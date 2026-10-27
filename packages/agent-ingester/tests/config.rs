use agent_ingester::{
    cli::{self, Action},
    config::{Paths, duration},
};
use cli_command_kit::Mode;
use std::{collections::HashMap, path::PathBuf, time::Duration};
fn parse_options(args: &[&str], env: &[(&str, &str)]) -> agent_ingester::cli::Options {
    let env: HashMap<_, _> = env.iter().copied().collect();
    let Action::Serve(options) = cli::parse_with_env(
        args.iter().copied(),
        Mode::Agent,
        &mut Vec::new(),
        |key| env.get(key).map(|value| value.to_string()),
        Paths::new(Some("/fixture/home".into())),
    )
    .unwrap() else {
        panic!("expected daemon options")
    };
    *options
}
#[test]
fn explicit_configuration_preserves_provider_precedence_and_paths() {
    let options = parse_options(
        &["agent-ingester"],
        &[
            ("AGENT_INGESTER_ADDR", "127.0.0.1:0"),
            ("AGENT_BEARER_TOKEN", "local"),
            ("XDG_DATA_HOME", "/data"),
            ("XDG_RUNTIME_DIR", "/runtime"),
            ("XDG_CONFIG_HOME", "/config"),
            ("LITELLM_BASE_URL", "http://localhost:5000/v1"),
            ("OPENAI_BASE_URL", "http://localhost:6000"),
            ("LITELLM_API_KEY", "one"),
            ("LITELLM_KEY", "two"),
            ("OPENAI_API_KEY", "three"),
            ("GEMINI_API_KEY", "four"),
            ("ANTHROPIC_API_KEY", "five"),
            ("AGENT_LOG_DIR", "logs"),
            ("AGENT_LOG_FILE", "custom.log"),
            ("AGENT_IDLE_GRACE_PERIOD", "125ms"),
        ],
    );
    assert_eq!(options.addr, "127.0.0.1:0");
    assert_eq!(options.token, "local");
    assert_eq!(
        options.database,
        PathBuf::from("/data/agent-status/agent-status.db")
    );
    assert_eq!(
        options.notify_socket,
        PathBuf::from("/runtime/agent-status/notify.sock")
    );
    assert_eq!(
        options.herdr_socket,
        PathBuf::from("/config/herdr/herdr.sock")
    );
    assert_eq!(options.classifier.idle_grace, Duration::from_millis(125));
    assert_eq!(options.summarizer.providers.len(), 3);
    assert_eq!(options.summarizer.providers[0].api_key, "one");
    assert_eq!(
        options.summarizer.providers[0].endpoint.as_deref(),
        Some("http://localhost:5000/v1")
    );
    assert_eq!(options.summarizer.providers[0].model, "gpt-4o-mini");
    assert_eq!(options.summarizer.providers[1].model, "gemini-2.0-flash");
    assert_eq!(
        options.summarizer.providers[2].model,
        "claude-3-5-haiku-latest"
    );
    assert_eq!(options.log_dir, PathBuf::from("logs"));
    assert_eq!(options.log_file, PathBuf::from("custom.log"));
}
#[test]
fn flags_override_environment_and_disable_source_without_hidden_sdk_environment_lookups() {
    let options = parse_options(
        &[
            "agent-ingester",
            "--no-summarize=false",
            "--litellm-url",
            "",
            "--litellm-key",
            "override",
            "--summarizer-model",
            "chosen",
            "--idle-grace-period",
            "0",
            "--addr",
            ":0",
            "--db",
            "chosen.db",
            "--notify-sock",
            "chosen.sock",
            "--token",
            "",
            "--log-dir",
            "chosen-logs",
            "--log-file",
            "chosen.log",
            "--log-level",
            "DEBUG",
        ],
        &[
            ("AGENT_NO_SUMMARIZE", "1"),
            ("OPENAI_API_KEY", "environment"),
            ("AGENT_IDLE_GRACE_PERIOD", "bad"),
            ("HERDR_SOCKET_PATH", "chosen-herdr"),
        ],
    );
    assert_eq!(options.summarizer.disabled, None);
    assert_eq!(options.summarizer.providers.len(), 1);
    assert_eq!(options.summarizer.providers[0].api_key, "override");
    assert_eq!(options.summarizer.providers[0].model, "chosen");
    assert_eq!(options.summarizer.providers[0].endpoint, None);
    assert_eq!(options.addr, ":0");
    assert!(options.token.is_empty());
    assert_eq!(options.classifier.idle_grace, Duration::ZERO);
    assert_eq!(options.database, PathBuf::from("chosen.db"));
    assert_eq!(options.notify_socket, PathBuf::from("chosen.sock"));
    assert_eq!(options.herdr_socket, PathBuf::from("chosen-herdr"));
    for (argv, env, source) in [
        (
            vec!["agent-ingester", "--no-summarize"],
            vec![],
            agent_ingester::summarizer::DisableSource::Flag,
        ),
        (
            vec!["agent-ingester"],
            vec![("AGENT_NO_SUMMARIZE", "1")],
            agent_ingester::summarizer::DisableSource::Environment,
        ),
    ] {
        let options = parse_options(&argv, &env);
        assert_eq!(options.summarizer.disabled, Some(source));
        assert!(options.summarizer.providers.is_empty());
    }
    let defaults = parse_options(&["agent-ingester"], &[]);
    assert_eq!(
        defaults.database,
        PathBuf::from("/fixture/home/.local/share/agent-status/agent-status.db")
    );
    assert_eq!(
        defaults.notify_socket,
        PathBuf::from("/tmp/agent-status/notify.sock")
    );
    assert_eq!(
        defaults.herdr_socket,
        PathBuf::from("/fixture/home/.config/herdr/herdr.sock")
    );
    assert_eq!(defaults.token, "dev-secret-token");
    for bad in ["bad", "-1s"] {
        assert_eq!(
            parse_options(&["agent-ingester"], &[("AGENT_IDLE_GRACE_PERIOD", bad)])
                .classifier
                .idle_grace,
            Duration::from_secs(5)
        );
    }
    assert_eq!(
        parse_options(&["agent-ingester"], &[("LITELLM_KEY", "alternate")])
            .summarizer
            .providers[0]
            .api_key,
        "alternate"
    );
    assert_eq!(
        parse_options(
            &["agent-ingester"],
            &[
                ("OPENAI_API_KEY", "fallback"),
                ("OPENAI_BASE_URL", "http://localhost")
            ]
        )
        .summarizer
        .providers[0]
            .api_key,
        "fallback"
    );
    assert_eq!(
        Paths::new(None).database(&|_| None),
        PathBuf::from(".local/share/agent-status/agent-status.db")
    );
}
#[test]
fn duration_parser_accepts_go_unicode_and_fractional_units_but_rejects_negative_values() {
    for value in ["1µs", "1μs", "1us"] {
        assert_eq!(duration(value).unwrap(), Duration::from_micros(1));
    }
    for value in ["0", "+0", "-0"] {
        assert_eq!(duration(value).unwrap(), Duration::ZERO);
    }
    assert_eq!(
        duration("0.123456789012345678901s").unwrap(),
        Duration::from_nanos(123456789)
    );
    assert!(duration("-1s").is_err());
    assert!(duration("broken").is_err());
}
