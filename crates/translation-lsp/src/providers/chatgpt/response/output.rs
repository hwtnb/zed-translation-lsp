use super::{Error, SizeLimit, SizeSetting};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    mem,
};

#[derive(Default)]
pub(super) struct Output {
    completed: Option<String>,
    parts: BTreeMap<(u64, u64), TextPart>,
    done_items: BTreeMap<u64, OutputItem>,
    streamed_bytes: usize,
    item_bytes: usize,
}

impl Output {
    pub fn accept(&mut self, kind: &str, value: &Value, limit: SizeLimit) -> Result<(), Error> {
        match kind {
            "response.output_text.delta" => self.text(value, false, limit),
            "response.output_text.done" => self.text(value, true, limit),
            "response.output_item.done" => self.item(value, limit),
            "response.completed" => self.complete(value, limit),
            _ => Ok(()),
        }
    }

    /// Only the transport reader may call this, after EOF and truncation checks.
    pub fn finish(self) -> Result<String, Error> {
        self.completed
            .ok_or_else(|| Error::failed("OpenAI stream ended without response.completed"))
    }

    fn text(&mut self, value: &Value, done: bool, output_limit: SizeLimit) -> Result<(), Error> {
        let output_index = value["output_index"]
            .as_u64()
            .ok_or_else(|| Error::failed("OpenAI text output index is invalid"))?;
        let content_index = value["content_index"]
            .as_u64()
            .ok_or_else(|| Error::failed("OpenAI text content index is invalid"))?;
        let item_id = value["item_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Error::failed("OpenAI text item ID is invalid"))?;

        let text = value
            .get(if done { "text" } else { "delta" })
            .and_then(Value::as_str)
            .ok_or_else(|| Error::failed("OpenAI streamed text is invalid"))?;
        if self.completed.is_some() || self.done_items.contains_key(&output_index) {
            return Err(Error::failed("OpenAI text follows completion"));
        }
        let key = (output_index, content_index);
        if let Some(part) = self.parts.get_mut(&key) {
            if part.item_id != item_id || part.done {
                return Err(Error::failed("OpenAI text item is inconsistent"));
            }
            if done {
                if part.text != text {
                    return Err(Error::failed("OpenAI text completion is inconsistent"));
                }
                part.done = true;
            } else {
                self.streamed_bytes = self.streamed_bytes.saturating_add(text.len());
                output_limit.check(self.streamed_bytes, SizeSetting::Output)?;
                part.text.push_str(text);
            }
        } else {
            self.streamed_bytes = self.streamed_bytes.saturating_add(text.len());
            output_limit.check(self.streamed_bytes, SizeSetting::Output)?;
            self.parts.insert(
                key,
                TextPart {
                    item_id: item_id.into(),
                    text: text.into(),
                    done,
                },
            );
        }
        Ok(())
    }

    fn item(&mut self, value: &Value, output_limit: SizeLimit) -> Result<(), Error> {
        let index = value["output_index"]
            .as_u64()
            .ok_or_else(|| Error::failed("OpenAI output item index is invalid"))?;
        let item = output_item(&value["item"], output_limit)?;
        if self.completed.is_some() || self.done_items.contains_key(&index) || item.id.is_none() {
            return Err(Error::failed("OpenAI output item completion is invalid"));
        }
        self.item_bytes = item.texts.iter().fold(self.item_bytes, |bytes, text| {
            bytes.saturating_add(text.len())
        });
        output_limit.check(self.item_bytes, SizeSetting::Output)?;
        self.done_items.insert(index, item);
        Ok(())
    }

    fn complete(&mut self, value: &Value, output_limit: SizeLimit) -> Result<(), Error> {
        if self.completed.is_some()
            || value["response"]["status"] != "completed"
            || !value["response"]["error"].is_null()
        {
            return Err(Error::failed("OpenAI completion status is invalid"));
        }
        let output = value["response"]["output"]
            .as_array()
            .ok_or_else(|| Error::failed("OpenAI completion has no output"))?;
        let mut items = BTreeMap::new();
        for (index, value) in output.iter().enumerate() {
            items.insert(index as u64, output_item(value, output_limit)?);
        }
        for output in [&self.done_items, &items] {
            let mut ids = HashSet::new();
            for item in output.values() {
                if item.id.as_ref().is_some_and(|id| !ids.insert(id)) {
                    return Err(Error::failed("OpenAI output item ID is duplicated"));
                }
            }
        }
        if items.is_empty() {
            items = mem::take(&mut self.done_items);
        } else {
            for (index, done) in &self.done_items {
                let Some(item) = items.get(index) else {
                    return Err(Error::failed("OpenAI completed output omits an item"));
                };
                if item.texts != done.texts
                    || item
                        .id
                        .as_ref()
                        .is_some_and(|id| Some(id) != done.id.as_ref())
                {
                    return Err(Error::failed("OpenAI completed output is inconsistent"));
                }
            }
        }
        for ((index, content_index), part) in &self.parts {
            let Some(item) = items.get(index) else {
                return Err(Error::failed("OpenAI completed output omits streamed text"));
            };
            let item_id = item
                .id
                .as_ref()
                .or_else(|| self.done_items.get(index).and_then(|done| done.id.as_ref()));
            if item_id.is_some_and(|id| id != &part.item_id)
                || usize::try_from(*content_index)
                    .ok()
                    .and_then(|index| item.texts.get(index))
                    != Some(&part.text)
            {
                return Err(Error::failed(
                    "OpenAI streamed text does not match its completed output",
                ));
            }
        }
        let mut text = String::new();
        for item in items.into_values() {
            for part in item.texts {
                output_limit.check(text.len().saturating_add(part.len()), SizeSetting::Output)?;
                text.push_str(&part);
            }
        }
        if text.is_empty() {
            return Err(Error::failed("OpenAI completed without translation text"));
        }
        self.completed = Some(text);
        Ok(())
    }
}

struct TextPart {
    item_id: String,
    text: String,
    done: bool,
}

struct OutputItem {
    id: Option<String>,
    texts: Vec<String>,
}

fn output_item(value: &Value, output_limit: SizeLimit) -> Result<OutputItem, Error> {
    let mut texts = Vec::new();
    match value["type"].as_str() {
        Some("reasoning") => (),
        Some("message") => {
            if value["status"] != "completed" || value["role"] != "assistant" {
                return Err(Error::failed("OpenAI output message is incomplete"));
            }
            let content = value["content"]
                .as_array()
                .ok_or_else(|| Error::failed("OpenAI output content is invalid"))?;
            let mut bytes = 0usize;
            for part in content {
                if part["type"] != "output_text" {
                    return Err(Error::failed("ChatGPT did not return translation text"));
                }
                let text = part["text"]
                    .as_str()
                    .ok_or_else(|| Error::failed("OpenAI output text is invalid"))?;
                bytes = bytes.saturating_add(text.len());
                output_limit.check(bytes, SizeSetting::Output)?;
                texts.push(text.to_owned());
            }
        }
        _ => return Err(Error::failed("OpenAI returned unexpected output")),
    }
    let id = match value.get("id") {
        None => None,
        Some(Value::String(id)) if !id.is_empty() => Some(id.clone()),
        _ => return Err(Error::failed("OpenAI output item ID is invalid")),
    };
    Ok(OutputItem { id, texts })
}
