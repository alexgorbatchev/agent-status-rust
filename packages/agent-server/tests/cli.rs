use agent_server::cli::{self, Action, Paths};
use cli_command_kit::Mode;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
};

fn parse(
    args: &[&str],
    mode: Mode,
    env: &[(&str, &str)],
    home: Option<&str>,
    output: &mut impl Write,
) -> Result<Action, cli::Error> {
    cli::parse_with_env(
        args,
        mode,
        output,
        |name| {
            env.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        },
        Paths {
            home: home.map(PathBuf::from),
        },
    )
}

#[test]
fn native_flags_and_environment_precedence_preserve_paths_csv_and_sensitive_configuration() {
    let mut output = Vec::new();
    let env = [
        ("XDG_DATA_HOME", "/data"),
        ("XDG_RUNTIME_DIR", "/runtime"),
        ("AGENT_STATUS_TOKEN", "configured"),
        ("AGENT_INGESTER_HTTP_URL", "http://ingester:4501"),
        ("AGENT_LOG_DIR", "/logs"),
        ("AGENT_LOG_FILE", "/logs/server.log"),
    ];
    let Action::Serve(options) = parse(
        &["agent-server"],
        Mode::Human,
        &env,
        Some("/home/test"),
        &mut output,
    )
    .unwrap() else {
        panic!("expected daemon")
    };
    assert_eq!(options.addr, ":4502");
    assert_eq!(
        options.database,
        Path::new("/data/agent-status/agent-status.db")
    );
    assert_eq!(
        options.socket,
        Path::new("/runtime/agent-status/notify.sock")
    );
    assert_eq!(options.api.auth_token, "configured");
    assert_eq!(options.api.ingester_url, "http://ingester:4501");
    assert_eq!(options.log_file, Path::new("/logs/server.log"));
    assert!(output.is_empty());
    let Action::Serve(options) = parse(
        &[
            "agent-server",
            "-a",
            "127.0.0.1:0",
            "--db",
            "custom.db",
            "--socket",
            "custom.sock",
            "--token=",
            "--ingester-url",
            "http://local:99",
            "--cors-origin",
            "http://one,http://two",
            "--cors-origin",
            "\"http://comma,inside\"",
            "--log-level",
            "debug",
            "--log-dir",
            "custom-logs",
            "--log-file",
            "custom.log",
        ],
        Mode::Agent,
        &env,
        Some("/home/test"),
        &mut output,
    )
    .unwrap() else {
        panic!("expected daemon")
    };
    assert_eq!(options.addr, "127.0.0.1:0");
    assert_eq!(options.database, Path::new("custom.db"));
    assert_eq!(options.socket, Path::new("custom.sock"));
    assert!(options.api.auth_token.is_empty());
    assert_eq!(options.api.ingester_url, "http://local:99");
    assert_eq!(
        options.api.cors_origins,
        ["http://one", "http://two", "http://comma,inside"]
    );
    assert_eq!(options.log_level, "debug");
    assert_eq!(options.log_dir, Path::new("custom-logs"));
    let (logging, file) = cli::logging_options(&options, Mode::Agent);
    assert_eq!(logging.service, "server");
    assert_eq!(logging.level, "debug");
    assert_eq!(file.filename.as_deref(), Some(Path::new("custom.log")));
}

#[test]
fn default_paths_handle_absent_and_empty_xdg_home_without_touching_files() {
    for (env, home, database, socket) in [
        (
            vec![],
            Some("/home/test"),
            "/home/test/.local/share/agent-status/agent-status.db",
            "/tmp/agent-status/notify.sock",
        ),
        (
            vec![("XDG_DATA_HOME", ""), ("XDG_RUNTIME_DIR", "")],
            None,
            ".local/share/agent-status/agent-status.db",
            "/tmp/agent-status/notify.sock",
        ),
    ] {
        let Action::Serve(options) =
            parse(&["agent-server"], Mode::Human, &env, home, &mut Vec::new()).unwrap()
        else {
            panic!("expected daemon")
        };
        assert_eq!(options.database, Path::new(database));
        assert_eq!(options.socket, Path::new(socket));
    }
}

#[test]
fn skill_help_version_completion_and_invalid_values_use_native_offline_paths() {
    for mode in [Mode::Human, Mode::Agent] {
        for args in [
            vec!["agent-server", "skill"],
            vec!["agent-server", "--version"],
            vec!["agent-server", "--help"],
            vec!["agent-server", "help", "completion"],
            vec!["agent-server", "completion"],
            vec!["agent-server", "completion", "bash"],
            vec!["agent-server", "completion", "zsh"],
            vec!["agent-server", "completion", "fish"],
            vec!["agent-server", "completion", "powershell"],
        ] {
            let mut output = Vec::new();
            assert!(matches!(
                parse(&args, mode, &[], None, &mut output).unwrap(),
                Action::Printed
            ));
            let text = String::from_utf8(output).unwrap();
            if args.contains(&"skill") {
                assert_eq!(text, cli::SKILL_POINTER);
            } else if args.contains(&"--version") {
                assert_eq!(text, concat!(env!("CARGO_PKG_VERSION"), "\n"));
            } else if args.contains(&"--help") || args.contains(&"help") || args.len() == 2 {
                assert_eq!(
                    text.starts_with("ALERT: Agents must read `AGENT=1 agent-server skill`"),
                    mode == Mode::Agent
                );
            } else {
                assert!(text.contains("agent-server"));
            }
        }
    }
    for args in [
        vec!["agent-server", "--unknown"],
        vec!["agent-server", "extra"],
        vec!["agent-server", "--log-level", "invalid"],
        vec!["agent-server", "skill", "extra"],
        vec!["agent-server", "skill", "--db", "x"],
    ] {
        assert!(
            parse(&args, Mode::Agent, &[], None, &mut Vec::new()).is_err(),
            "{args:?}"
        );
    }
}

struct Broken;
impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("closed output"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("closed output"))
    }
}
#[test]
fn metadata_write_failure_propagates_and_skill_covers_the_live_interface() {
    for args in [
        vec!["agent-server", "skill"],
        vec!["agent-server", "--version"],
        vec!["agent-server", "--help"],
        vec!["agent-server", "completion", "fish"],
    ] {
        assert!(parse(&args, Mode::Agent, &[], None, &mut Broken).is_err());
    }
    let text = include_str!("../../../skills/agent-server/SKILL.md");
    assert!(text.contains("name: agent-server"));
    let omissions =
        cli_command_kit::skill_omissions(&cli::command(Mode::Agent), &cli::help_options(), text);
    assert!(omissions.is_empty(), "{omissions:?}");
    assert!(cli::SKILL_POINTER.contains("/skills/agent-server"));
}

#[cfg(unix)]
#[test]
fn non_unicode_auth_environment_is_rejected_rather_than_silently_disabling_auth() {
    use std::os::unix::ffi::OsStringExt;
    let result = cli::parse_with_env(
        ["agent-server"],
        Mode::Human,
        &mut Vec::new(),
        |name| (name == "AGENT_STATUS_TOKEN").then(|| OsString::from_vec(vec![255])),
        Paths { home: None },
    );
    assert!(matches!(
        result,
        Err(cli::Error::InvalidEnvironment("AGENT_STATUS_TOKEN"))
    ));
}

#[test]
fn origin_csv_rejects_malformed_quoting_and_extra_records_before_service_access() {
    for value in ["a\"b", "\"unclosed", "\"closed\"junk", "a\nb", "a\r"] {
        assert!(
            parse(
                &["agent-server", "--cors-origin", value],
                Mode::Agent,
                &[],
                None,
                &mut Vec::new()
            )
            .is_err(),
            "accepted {value:?}"
        );
    }
    for (value, expected) in [
        ("", vec![]),
        ("a,", vec!["a", ""]),
        ("\"a\"\"b\",c", vec!["a\"b", "c"]),
        ("a,b\r\n", vec!["a", "b"]),
    ] {
        let Action::Serve(options) = parse(
            &["agent-server", "--cors-origin", value],
            Mode::Human,
            &[],
            None,
            &mut Vec::new(),
        )
        .unwrap() else {
            panic!("expected daemon")
        };
        assert_eq!(options.api.cors_origins, expected);
    }
}

#[test]
fn origin_csv_does_not_impose_document_limits_on_a_valid_single_flag_record() {
    let value = "a".repeat(70_000);
    let Action::Serve(options) = parse(
        &["agent-server", "--cors-origin", &value],
        Mode::Agent,
        &[],
        None,
        &mut Vec::new(),
    )
    .unwrap() else {
        panic!("expected daemon")
    };
    assert_eq!(options.api.cors_origins, [value]);
    let value = vec!["http://localhost"; 1100].join(",");
    let Action::Serve(options) = parse(
        &["agent-server", "--cors-origin", &value],
        Mode::Agent,
        &[],
        None,
        &mut Vec::new(),
    )
    .unwrap() else {
        panic!("expected daemon")
    };
    assert_eq!(options.api.cors_origins.len(), 1100);
}
