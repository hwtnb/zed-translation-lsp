use serde::Deserialize;

/// `hover`: how Hover shows a translation.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct HoverSettings {
    /// Whether the title, the provider's note, and the rule above the
    /// translation show.
    pub header: bool,
    pub format: HoverFormat,
}

impl Default for HoverSettings {
    fn default() -> Self {
        Self {
            header: true,
            format: HoverFormat::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum HoverFormat {
    /// Markdown that keeps the formatting the translation writes and shows
    /// every other character as written.
    #[default]
    Markdown,
    /// A code block of the translation as written.
    Text,
}
