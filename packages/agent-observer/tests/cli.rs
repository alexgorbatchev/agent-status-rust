use agent_observer::cli::{self, Action};
use cli_command_kit::Mode;
#[test]
fn logging_values_use_the_shared_case_insensitive_parser() {
    for value in ["DEBUG", " warning ", "", "Info", "eRrOr"] {
        let (Action::Observe(options), _) =
            parse(&["agent-observer", "--log-level", value], Mode::Agent, &[])
        else {
            panic!("expected daemon options")
        };
        assert!(agent_common::logging::parse_level(&options.log_level).is_ok());
    }
}
use std::{collections::HashMap, io, time::Duration};

fn parse(args: &[&str], mode: Mode, env: &[(&str, &str)]) -> (Action, String) {
    let env: HashMap<_, _> = env.iter().copied().collect();
    let mut output = Vec::new();
    let action = cli::parse_with_env(args.iter().copied(), mode, &mut output, |key| {
        env.get(key).map(|v| v.to_string())
    })
    .unwrap();
    (action, String::from_utf8(output).unwrap())
}

#[test]
fn offline_metadata_and_all_help_paths() {
    for mode in [Mode::Human, Mode::Agent] {
        assert_eq!(
            parse(&["agent-observer", "--version"], mode, &[]).1,
            format!("{}\n", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            parse(&["agent-observer", "skill"], mode, &[]).1,
            cli::SKILL_POINTER
        );
        for path in [
            vec!["--help"],
            vec!["skill", "--help"],
            vec!["completion", "--help"],
            vec!["completion", "bash", "--help"],
            vec!["help"],
            vec!["help", "skill"],
        ] {
            let mut args = vec!["agent-observer"];
            args.extend(path);
            let text = parse(&args, mode, &[]).1;
            if mode == Mode::Agent {
                assert!(
                    text.starts_with("ALERT: Agents must read `AGENT=1 agent-observer skill`"),
                    "{text}"
                );
            } else {
                assert!(!text.starts_with("ALERT:"));
            }
        }
        for shell in ["bash", "zsh", "fish", "powershell"] {
            assert!(
                !parse(&["agent-observer", "completion", shell], mode, &[])
                    .1
                    .is_empty()
            );
        }
    }
}

#[test]
fn operational_skill_matches_live_command_tree_and_pointer_location() {
    let skill = std::fs::read_to_string("../../skills/agent-observer/SKILL.md").unwrap();
    assert!(skill.contains("name: agent-observer\n"));
    assert!(cli::SKILL_POINTER.contains("--skill agent-observer"));
    assert!(cli::SKILL_POINTER.contains("/skills/agent-observer\n"));
    let omissions =
        cli_command_kit::skill_omissions(&cli::command(Mode::Agent), &cli::help_options(), &skill);
    assert!(omissions.is_empty(), "{omissions:#?}");
}

#[test]
fn environment_defaults_and_explicit_options_map_to_native_configuration() {
    let (Action::Observe(options), text) = parse(
        &[
            "agent-observer",
            "--env",
            "office",
            "--scan-interval",
            "125ms",
            "--cursor-path",
            "cursor",
            "--claude-projects-dir",
            "projects",
            "--pi-sessions-dir",
            "pi",
            "--codex-sessions-dir",
            "codex",
            "--opencode-db-path",
            "db",
            "--log-file",
            "observer.log",
            "--log-dir",
            "logs",
        ],
        Mode::Agent,
        &[
            ("AGENT_INGESTER_WS_URL", "ws://localhost:1234/ws/ingest"),
            ("AGENT_STATUS_TOKEN", "fixture"),
            ("AGENT_SESSION_STATE_PATH", "sessions"),
            ("CLAUDE_SESSIONS_DIR", "claude"),
            ("AGENT_CURSOR_RETENTION", "48h"),
            ("AGENT_CURSOR_PRUNE_INTERVAL", "bad"),
            ("AGENT_CURSOR_FLUSH_INTERVAL", "250ms"),
        ],
    ) else {
        panic!("expected observer configuration")
    };
    assert!(text.is_empty());
    assert_eq!(
        options.ingester_url.as_str(),
        "ws://localhost:1234/ws/ingest"
    );
    assert_eq!(options.token, "fixture");
    assert_eq!(options.environment_label, "office");
    assert_eq!(options.watcher.scan_interval, Duration::from_millis(125));
    assert_eq!(options.watcher.cursor_path.to_str(), Some("cursor"));
    assert_eq!(
        options.watcher.session_state_path.to_str(),
        Some("sessions")
    );
    assert_eq!(
        options.watcher.claude_sessions_dirs,
        vec![std::path::PathBuf::from("claude")]
    );
    assert_eq!(
        options.watcher.claude_projects_dir.to_str(),
        Some("projects")
    );
    assert_eq!(options.watcher.pi_sessions_dir.to_str(), Some("pi"));
    assert_eq!(options.watcher.codex_sessions_dir.to_str(), Some("codex"));
    assert_eq!(options.watcher.opencode_db_path.to_str(), Some("db"));
    assert_eq!(
        options.watcher.cursor_retention,
        Duration::from_secs(172800)
    );
    assert_eq!(
        options.watcher.cursor_prune_interval,
        Duration::from_secs(3600)
    );
    assert_eq!(
        options.watcher.cursor_flush_interval,
        Duration::from_millis(250)
    );
    assert_eq!(options.log_file.to_str(), Some("observer.log"));
    assert_eq!(options.log_dir.to_str(), Some("logs"));
    let (Action::Observe(defaults), _) = parse(
        &[
            "agent-observer",
            "--token",
            "override",
            "--cursor-retention",
            "0",
        ],
        Mode::Human,
        &[("AGENT_STATUS_TOKEN", "fixture")],
    ) else {
        panic!()
    };
    assert_eq!(defaults.token, "override");
    assert_eq!(defaults.watcher.cursor_retention, Duration::ZERO);
}

#[test]
fn invalid_arguments_and_writer_failures_are_typed() {
    for args in [
        vec!["unexpected"],
        vec!["skill", "extra"],
        vec!["--scan-interval", "bad"],
        vec!["--scan-interval", "-1s"],
        vec!["--log-level", "verbose"],
        vec!["--ingester-url", "https://localhost"],
        vec!["--ingester-url", "bad"],
    ] {
        let mut argv = vec!["agent-observer"];
        argv.extend(args);
        assert!(matches!(
            cli::parse_with_env(argv, Mode::Agent, &mut Vec::new(), |_| None),
            Err(cli::Error::Arguments(_))
        ));
    }
    struct Broken;
    impl io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec!["skill"],
        vec!["completion", "bash"],
        vec!["completion"],
    ] {
        let mut argv = vec!["agent-observer"];
        argv.extend(args);
        assert!(matches!(
            cli::parse_with_env(argv, Mode::Human, &mut Broken, |_| None),
            Err(cli::Error::Io(_))
        ));
    }
}

#[test]
fn unicode_microseconds_are_accepted_without_panicking() {
    for text in ["1µs", "1μs", "1us"] {
        let (Action::Observe(options), _) = parse(
            &["agent-observer", "--scan-interval", text],
            Mode::Agent,
            &[],
        ) else {
            panic!()
        };
        assert_eq!(options.watcher.scan_interval, Duration::from_micros(1));
    }
    let (Action::Observe(options), _) = parse(
        &[
            "agent-observer",
            "--scan-interval",
            "0.123456789012345678901234567890123456789s",
        ],
        Mode::Agent,
        &[],
    ) else {
        panic!()
    };
    assert_eq!(
        options.watcher.scan_interval,
        Duration::from_nanos(123456789)
    );
}

#[test]
fn log_configuration_and_console_fold_only_displayed_paths() {
    use std::io::Write;
    let (Action::Observe(options), _) = parse(
        &[
            "agent-observer",
            "--log-file",
            "chosen.log",
            "--log-dir",
            "chosen-dir",
            "--log-level",
            "debug",
        ],
        Mode::Human,
        &[],
    ) else {
        panic!()
    };
    let (logging, file) = cli::logging_options(&options, Mode::Human);
    assert_eq!(logging.level, "debug");
    assert_eq!(logging.mode, agent_common::logging::Mode::Human);
    assert_eq!(
        file.filename.unwrap(),
        std::path::PathBuf::from("chosen.log")
    );
    assert_eq!(file.dir, std::path::PathBuf::from("chosen-dir"));
    let home = dirs::home_dir().unwrap();
    let text = format!(
        "path={} child={} sibling={} other=/outside\n",
        home.display(),
        home.join("project").display(),
        format_args!("{}-sibling", home.display())
    );
    let mut console = cli::Console(Vec::new());
    console.write_all(text.as_bytes()).unwrap();
    console.flush().unwrap();
    let actual = String::from_utf8(console.0).unwrap();
    assert!(actual.contains("path=~ child=~/project"));
    assert!(actual.contains(&format!("sibling={}-sibling", home.display())));
    assert!(actual.contains("other=/outside"));
    assert!(cli::Console(Vec::new()).write(&[255]).is_err());
    let (Action::Observe(defaults), _) = parse(&["agent-observer"], Mode::Agent, &[]) else {
        panic!()
    };
    assert_eq!(
        cli::logging_options(&defaults, Mode::Agent).0.mode,
        agent_common::logging::Mode::Agent
    );
    assert!(matches!(
        cli::parse_with_env(["agent-observer"], Mode::Agent, &mut Vec::new(), |key| (key
            == "AGENT_INGESTER_WS_URL")
            .then(|| "invalid".into())),
        Err(cli::Error::Arguments(_))
    ));
    assert!(matches!(
        cli::parse_with_env(
            ["agent-observer", "--ingester-url", "ws://localhost:99"],
            Mode::Agent,
            &mut Vec::new(),
            |key| (key == "AGENT_INGESTER_WS_URL").then(|| "invalid".into())
        ),
        Ok(Action::Observe(_))
    ));
}

#[tokio::test]
async fn observation_with_cancelled_token_resolves_identity_without_state_writes() {
    let (Action::Observe(options), _) = parse(&["agent-observer"], Mode::Agent, &[]) else {
        panic!()
    };
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    cli::observe(&token, *options).await.unwrap();
}
