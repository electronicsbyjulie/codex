use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

fn filter() -> OutputFilter {
    OutputFilter::compile(OutputFilterConfig {
        rules: vec![OutputFilterRule {
            pattern: r"(?i)\bsecret\w*\b".into(),
            exempt_pattern: Some("(?i)secretar(?:y|ies)".into()),
            replacement: "[filtered]".into(),
        }],
    }).unwrap()
}

#[test]
fn words_forms_case_and_exact_exceptions() {
    assert_eq!(filter().text("SECRET secrets secrecy secretary secretaries"),
        "[filtered] [filtered] secrecy secretary secretaries");
    assert_eq!(filter().text("secretarysecret"), "[filtered]");
    assert_eq!(filter().text("café secret 🦀"), "café [filtered] 🦀");
}

#[test]
fn exceptions_do_not_exempt_the_rest_of_the_message() {
    assert_eq!(filter().text("secretary secret"), "secretary [filtered]");
}

#[test]
fn nested_json_and_protocol_identity() {
    let mut value = json!({"type": "message", "id": "stable-id", "content": [{"text": "secret"}]});
    filter().json(&mut value).unwrap();
    assert_eq!(value, json!({"type": "message", "id": "stable-id", "content": [{"text": "[filtered]"}]}));
    assert!(filter().json(&mut json!({"id": "secret"})).is_err());
    assert!(filter().json(&mut json!({"secret": "value"})).is_err());
}

#[test]
fn parser_errors_and_replacements_do_not_reveal_source_patterns() {
    for pattern in ["(", "", ".*"] {
        let result = OutputFilter::compile(OutputFilterConfig {
            rules: vec![OutputFilterRule { pattern: pattern.into(), exempt_pattern: None, replacement: "".into() }],
        });
        assert_eq!(result.err().unwrap().to_string(), "Invalid global output filter configuration.");
    }
}

#[test]
fn literal_replacement_and_new_boundary_matches() {
    let filter = OutputFilter::compile(OutputFilterConfig {
        rules: vec![
            OutputFilterRule { pattern: "ab".into(), exempt_pattern: None, replacement: "$1".into() },
            OutputFilterRule { pattern: r"\$1c".into(), exempt_pattern: None, replacement: "ok".into() },
        ],
    }).unwrap();
    assert_eq!(filter.text("ab"), "$1");
    assert_eq!(filter.text("abc"), "ok");
}

#[test]
fn invalid_typed_shapes_are_suppressed() {
    #[derive(Serialize, Deserialize)]
    struct Typed { status: State }
    #[derive(Serialize, Deserialize)]
    enum State { Secret }
    assert!(filter().typed(&Typed { status: State::Secret }).is_none());
}

#[test]
fn base_user_policy_survives_project_profile_and_session_overrides() {
    use codex_utils_absolute_path::AbsolutePathBuf;
    let file = AbsolutePathBuf::from_absolute_path(std::env::temp_dir().join("filter-config.toml")).unwrap();
    let user_config: toml::Value = toml::from_str("[[output_filter.rules]]\npattern = 'secret'\n").unwrap();
    let override_config: toml::Value = toml::from_str("[output_filter]\nrules = []\n").unwrap();
    let layers = [
        ConfigLayerEntry::new(ConfigLayerSource::User { file: file.clone(), profile: None }, user_config),
        ConfigLayerEntry::new(ConfigLayerSource::User { file: file.clone(), profile: Some("other".into()) }, override_config.clone()),
        ConfigLayerEntry::new(ConfigLayerSource::Project { dot_codex_folder: file }, override_config.clone()),
        ConfigLayerEntry::new(ConfigLayerSource::SessionFlags, override_config),
    ];
    let policy = config_from_layers(layers.iter()).unwrap();
    assert_eq!(policy.rules.len(), 1);
    assert_eq!(policy.rules[0].pattern, "secret");
    assert!(!format!("{policy:?}").contains("secret"));
}

#[test]
fn unsafe_replacement_is_rejected() {
    let config = OutputFilterConfig { rules: vec![OutputFilterRule {
        pattern: "secret".into(), exempt_pattern: None, replacement: "secret".into(),
    }] };
    assert!(OutputFilter::compile(config).is_err());
}
