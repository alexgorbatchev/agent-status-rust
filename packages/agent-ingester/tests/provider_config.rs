use agent_ingester::summarizer::{Config, DisableSource};
use std::collections::HashMap;

#[test]
fn explicit_lookup_constructs_native_providers_in_precedence_order() {
    let vars = HashMap::from([
        ("LITELLM_BASE_URL", "http://127.0.0.1:1/v1"),
        ("OPENAI_BASE_URL", "http://127.0.0.1:2/v1"),
        ("LITELLM_API_KEY", "first"),
        ("LITELLM_KEY", "second"),
        ("OPENAI_API_KEY", "third"),
        ("GEMINI_API_KEY", "fourth"),
        ("ANTHROPIC_API_KEY", "fifth"),
        ("AGENT_SUMMARIZER_MODEL", "chosen"),
    ]);
    let config = Config::from_lookup(None, |key| vars.get(key).map(|value| (*value).into()));
    assert_eq!(config.providers.len(), 3);
    assert_eq!(
        config
            .providers
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["openai", "gemini", "anthropic"]
    );
    assert_eq!(config.providers[0].api_key, "first");
    assert_eq!(
        config.providers[0].endpoint.as_deref(),
        Some("http://127.0.0.1:1/v1")
    );
    assert!(config.providers.iter().all(|p| p.model == "chosen"));
    let fallback = Config::from_lookup(None, |key| {
        (key == "OPENAI_API_KEY").then(|| "fallback".into())
    });
    assert_eq!(fallback.providers[0].api_key, "fallback");
    assert_eq!(fallback.providers[0].model, "gpt-4o-mini");
}

#[test]
fn disabling_configuration_never_reads_provider_credentials() {
    for source in [DisableSource::Flag, DisableSource::Environment] {
        let config =
            Config::from_lookup(Some(source), |_| panic!("disabled config read credentials"));
        assert_eq!(config.disabled, Some(source));
        assert!(config.providers.is_empty());
    }
    let config = Config::from_lookup(None, |_| None);
    assert!(config.providers.is_empty());
}

#[test]
fn explicit_empty_cli_settings_suppress_all_environment_fallbacks() {
    let action = agent_ingester::cli::parse_with_env(
        [
            "agent-ingester",
            "--litellm-url",
            "",
            "--litellm-key",
            "",
            "--summarizer-model",
            "",
        ],
        cli_command_kit::Mode::Agent,
        &mut Vec::new(),
        |key| match key {
            "OPENAI_API_KEY" | "LITELLM_KEY" => Some("hidden".into()),
            "OPENAI_BASE_URL" => Some("http://127.0.0.1:1".into()),
            "GEMINI_API_KEY" => Some("gemini".into()),
            "AGENT_SUMMARIZER_MODEL" => Some("hidden-model".into()),
            _ => None,
        },
        agent_ingester::config::Paths::new(Some("/fixture".into())),
    )
    .unwrap();
    let agent_ingester::cli::Action::Serve(options) = action else {
        panic!("expected serve")
    };
    assert_eq!(options.summarizer.providers.len(), 1);
    assert_eq!(options.summarizer.providers[0].name, "gemini");
    assert_eq!(options.summarizer.providers[0].model, "gemini-2.0-flash");
}
