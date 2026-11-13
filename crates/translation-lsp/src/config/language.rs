use std::borrow::Cow;

use isolang::Language;

use super::error::ConfigError;

/// A language setting as the user and the provider read it. A value that reads
/// as a language tag gets the English name of its language, as `Japanese (ja)`;
/// any other value, such as `Japanese` or `auto`, is used as written.
pub(crate) fn display(value: &str) -> Cow<'_, str> {
    match name(value) {
        Some(name) => format!("{name} ({value})").into(),
        None => value.into(),
    }
}

/// Whether `value` reads as a language tag, such as `ja` or `pt-BR`.
pub(crate) fn is_tag(value: &str) -> bool {
    name(value).is_some()
}

/// Whether `code` names a script, such as `Latn` in `sr-Latn` or `sr_Latn_RS`:
/// a part after the first of four ASCII letters, in any case.
pub(crate) fn names_script(code: &str) -> bool {
    code.split(['-', '_'])
        .skip(1)
        .any(|part| part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
}

/// Checks that the language settings are codes, for a provider that takes no
/// language names: `target_language` a language tag, and `source_language`
/// one too, or `auto`.
pub(crate) fn check_codes(
    source: &str,
    target: &str,
    provider: &'static str,
) -> Result<(), ConfigError> {
    for (setting, value) in [("target_language", target), ("source_language", source)] {
        if !(is_tag(value) || setting == "source_language" && value == "auto") {
            return Err(ConfigError::LanguageCode { setting, provider });
        }
    }
    Ok(())
}

/// The language of a tag such as `ja`, `yue`, `zh-Hans`, or `pt_BR`, named by
/// its first part. The other parts are only checked to be subtags.
fn name(value: &str) -> Option<&'static str> {
    let mut parts = value.split(['-', '_']);
    let primary = parts.next()?.to_ascii_lowercase();
    let subtags = parts.all(|part| {
        (1..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
    });
    if !subtags || !primary.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return None;
    }
    let language = match primary.len() {
        2 => Language::from_639_1(&primary),
        3 => Language::from_639_3(&primary),
        _ => None,
    }?;
    Some(language.to_name())
}

#[cfg(test)]
mod tests {
    use super::display;

    #[test]
    fn a_language_tag_is_named_and_any_other_value_is_kept() {
        for (value, shown) in [
            ("ja", "Japanese (ja)"),
            ("en", "English (en)"),
            ("JA", "Japanese (JA)"),
            ("zh-Hans", "Chinese (zh-Hans)"),
            ("pt_BR", "Portuguese (pt_BR)"),
            ("yue", "Yue Chinese (yue)"),
            ("auto", "auto"),
            ("Japanese", "Japanese"),
            ("xx", "xx"),
            ("ja-", "ja-"),
            ("ja JP", "ja JP"),
            ("", ""),
        ] {
            assert_eq!(display(value), shown, "{value}");
        }
    }
}
