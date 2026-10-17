use std::collections::HashMap;

use serde::Deserialize;

use crate::language::Language;

/// What starts a translation.
#[derive(Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Feature {
    CommentHover,
    AutomaticSelection,
    SelectionAction,
    DocumentAction,
}

impl Feature {
    /// Whether the feature runs in a registered language.
    pub(super) fn applies_to(self, language: Language) -> bool {
        match self {
            Self::CommentHover => !language.is_prose(),
            Self::AutomaticSelection => language == Language::Markdown,
            Self::SelectionAction => true,
            Self::DocumentAction => language.is_prose(),
        }
    }

    pub(super) fn language_ids(self) -> Vec<&'static str> {
        Language::ALL
            .into_iter()
            .filter(|language| self.applies_to(*language))
            .map(Language::id)
            .collect()
    }
}

impl std::fmt::Display for Feature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::CommentHover => "comment_hover",
            Self::AutomaticSelection => "automatic_selection",
            Self::SelectionAction => "selection_action",
            Self::DocumentAction => "document_action",
        })
    }
}

type Switches = HashMap<Feature, bool>;

/// `features`: the switches written for every language, and those written for
/// one language under `languages`. An omitted switch leaves the feature on.
#[derive(Clone, Default, Deserialize)]
pub(crate) struct Features {
    #[serde(default)]
    languages: HashMap<String, Switches>,
    /// Every other key is a feature name.
    #[serde(flatten)]
    switches: Switches,
}

impl Features {
    /// `language` is `None` for a language ID that `extension.toml` does not
    /// register, which only the switches for every language apply to.
    pub fn enables(&self, feature: Feature, language: Option<Language>) -> bool {
        language
            .and_then(|language| self.languages.get(language.id()))
            .and_then(|switches| switches.get(&feature))
            .or_else(|| self.switches.get(&feature))
            .copied()
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        super::{Settings, error::ConfigError},
        *,
    };

    const LANGUAGE_IDS: [&str; 18] = [
        "rust",
        "javascript",
        "typescript",
        "typescriptreact",
        "csharp",
        "c",
        "cpp",
        "python",
        "json",
        "jsonc",
        "yaml",
        "toml",
        "shellscript",
        "powershell",
        "dockerfile",
        "dockercompose",
        "markdown",
        "plaintext",
    ];

    const GENERIC: &str = "Invalid translation settings: check field names and value types";
    const FEATURES: [(Feature, &str); 4] = [
        (Feature::CommentHover, "comment_hover"),
        (Feature::AutomaticSelection, "automatic_selection"),
        (Feature::SelectionAction, "selection_action"),
        (Feature::DocumentAction, "document_action"),
    ];

    fn features(features: Value) -> Result<Features, ConfigError> {
        Settings::from_options(Some(json!({"features": features})))
            .map(|settings| settings.features)
    }

    fn message(features: Value) -> String {
        self::features(features.clone())
            .err()
            .unwrap_or_else(|| panic!("Accepted {features}"))
            .to_string()
    }

    #[test]
    fn language_ids_match_the_extension_manifest() {
        let manifest = include_str!("../../../zed-extension/extension.toml");
        let registered: Vec<_> = manifest
            .lines()
            .skip_while(|line| *line != "[language_servers.translation-lsp.language_ids]")
            .skip(1)
            .take_while(|line| !line.starts_with('['))
            .filter_map(|line| line.split_once(" = "))
            .map(|(_, language_id)| language_id.trim_matches('"'))
            .collect();
        assert_eq!(registered, LANGUAGE_IDS);
        assert_eq!(Language::ALL.map(Language::id), LANGUAGE_IDS);
    }

    #[test]
    fn every_feature_is_on_until_a_switch_turns_it_off() {
        let omitted = [
            Settings::from_options(None).unwrap().features,
            Settings::from_options(Some(json!({}))).unwrap().features,
            features(json!({})).unwrap(),
            features(json!({"languages": {}})).unwrap(),
            features(json!({"languages": {"yaml": {}}})).unwrap(),
        ];
        for features in &omitted {
            for (feature, name) in FEATURES {
                for language_id in LANGUAGE_IDS {
                    assert!(
                        features.enables(feature, Language::from_id(language_id)),
                        "{name} {language_id}"
                    );
                }
            }
        }
        for (off, name) in FEATURES {
            let features = features(json!({(name): false})).unwrap();
            for (feature, _) in FEATURES {
                for language_id in LANGUAGE_IDS {
                    assert_eq!(
                        features.enables(feature, Language::from_id(language_id)),
                        feature != off,
                        "{name} {language_id}"
                    );
                }
            }
        }
        let written = features(json!({
            "comment_hover": true,
            "automatic_selection": true,
            "selection_action": true,
            "document_action": false,
            "languages": {"yaml": {"comment_hover": false}, "toml": {"comment_hover": false}}
        }))
        .unwrap();
        assert!(written.enables(Feature::CommentHover, Language::from_id("rust")));
        assert!(!written.enables(Feature::CommentHover, Language::from_id("yaml")));
        assert!(!written.enables(Feature::CommentHover, Language::from_id("toml")));
        assert!(written.enables(Feature::SelectionAction, Language::from_id("yaml")));
        assert!(!written.enables(Feature::DocumentAction, Language::from_id("markdown")));
    }

    #[test]
    fn a_language_switch_overrides_the_switch_for_every_language() {
        // Each feature is switched in the first language it runs in.
        for (feature, name) in FEATURES {
            let language = feature.language_ids()[0];
            for (everywhere, own) in [(None, false), (Some(true), false), (Some(false), true)] {
                let mut written = json!({"languages": {(language): {(name): own}}});
                if let Some(everywhere) = everywhere {
                    written[name] = json!(everywhere);
                }
                let features = features(written).unwrap();
                for language_id in LANGUAGE_IDS {
                    assert_eq!(
                        features.enables(feature, Language::from_id(language_id)),
                        if language_id == language { own } else { !own },
                        "{name} {language_id}"
                    );
                }
                // The other features keep their own switches.
                for (other, _) in FEATURES {
                    if other != feature {
                        assert!(features.enables(other, Language::from_id(language)));
                    }
                }
            }
        }
        // A language outside the registered ones follows the switch for every language.
        let features = features(json!({
            "selection_action": false,
            "languages": {"javascript": {"selection_action": true}}
        }))
        .unwrap();
        assert!(features.enables(Feature::SelectionAction, Language::from_id("javascript")));
        assert!(!features.enables(
            Feature::SelectionAction,
            Language::from_id("javascriptreact")
        ));
    }

    #[test]
    fn unknown_names_and_values_other_than_booleans_are_generic_errors() {
        for features in [
            Value::Null,
            json!(true),
            json!([]),
            json!({"unknown": true}),
            json!({"markdown_auto_translate": false}),
            json!({"comment_hover": null}),
            json!({"comment_hover": 0}),
            json!({"comment_hover": "false"}),
            json!({"comment_hover": [false]}),
            json!({"comment_hover": {"yaml": false}}),
            json!({"languages": null}),
            json!({"languages": true}),
            json!({"languages": []}),
            json!({"languages": {"yaml": null}}),
            json!({"languages": {"yaml": false}}),
            json!({"languages": {"yaml": []}}),
            json!({"languages": {"yaml": {"unknown": false}}}),
            json!({"languages": {"yaml": {"languages": {}}}}),
            json!({"languages": {"yaml": {"comment_hover": null}}}),
            json!({"languages": {"yaml": {"comment_hover": "false"}}}),
            json!({"languages": {"yaml": {"comment_hover": [false]}}}),
        ] {
            assert_eq!(message(features), GENERIC);
        }
        for options in [
            json!({"markdown_auto_translate": false}),
            json!({"markdown_auto_translate": true}),
            json!({"comment_hover": false}),
        ] {
            let error = Settings::from_options(Some(options)).err().unwrap();
            assert_eq!(error.to_string(), GENERIC);
        }
    }

    #[test]
    fn language_ids_outside_the_registered_ones_are_named_without_the_rejected_key() {
        for language_id in [
            "javascriptreact",
            "tsx",
            "YAML",
            "Rust",
            "c++",
            "plain text",
            "",
            "synthetic-secret",
        ] {
            for switches in [json!({}), json!({"comment_hover": false}), Value::Null] {
                let error = features(json!({"languages": {(language_id): switches}}))
                    .err()
                    .unwrap();
                assert!(matches!(error, ConfigError::FeatureLanguage));
                assert_eq!(
                    error.to_string(),
                    "features.languages keys must be one of rust, javascript, typescript, \
                     typescriptreact, csharp, c, cpp, python, json, jsonc, yaml, toml, \
                     shellscript, powershell, dockerfile, dockercompose, markdown, plaintext"
                );
                assert_eq!(format!("{error:?}"), error.to_string());
            }
        }
        // The named check reports before the generic error.
        let error = Settings::from_options(Some(json!({
            "features": {"languages": {"go": {}}, "unknown": true},
            "translation_limits": []
        })))
        .err()
        .unwrap();
        assert!(matches!(error, ConfigError::FeatureLanguage));
    }

    #[test]
    fn a_feature_is_accepted_only_for_the_languages_it_runs_in() {
        let comments = &LANGUAGE_IDS[..16];
        for (feature, name, language_ids) in [
            (Feature::CommentHover, "comment_hover", comments),
            (
                Feature::AutomaticSelection,
                "automatic_selection",
                &["markdown"][..],
            ),
            (
                Feature::SelectionAction,
                "selection_action",
                &LANGUAGE_IDS[..],
            ),
            (
                Feature::DocumentAction,
                "document_action",
                &["markdown", "plaintext"][..],
            ),
        ] {
            assert_eq!(feature.language_ids(), language_ids);
            for language_id in LANGUAGE_IDS {
                for value in [json!(false), json!(true), json!("synthetic-secret")] {
                    let written = json!({"languages": {(language_id): {(name): value}}});
                    if language_ids.contains(&language_id) {
                        assert_eq!(features(written).is_ok(), value.is_boolean());
                        continue;
                    }
                    assert_eq!(
                        message(written),
                        format!(
                            "features.languages.{language_id}.{name} is not available; {name} \
                             applies to {}",
                            language_ids.join(", ")
                        )
                    );
                }
            }
        }
        assert_eq!(
            message(json!({"languages": {"rust": {"document_action": false}}})),
            "features.languages.rust.document_action is not available; document_action applies \
             to markdown, plaintext"
        );
        assert_eq!(
            message(json!({"languages": {"plaintext": {"automatic_selection": false}}})),
            "features.languages.plaintext.automatic_selection is not available; \
             automatic_selection applies to markdown"
        );
        assert_eq!(
            message(json!({"languages": {"markdown": {"comment_hover": false}}})),
            "features.languages.markdown.comment_hover is not available; comment_hover applies \
             to rust, javascript, typescript, typescriptreact, csharp, c, cpp, python, json, \
             jsonc, yaml, toml, shellscript, powershell, dockerfile, dockercompose"
        );
    }
}
