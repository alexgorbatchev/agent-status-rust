use agent_ingester::{
    cli::{self, Action},
    config::Paths,
};
use cli_command_kit::Mode;
use std::io::{self, Write};
fn parse(args: &[&str], mode: Mode) -> Result<(Action, String), cli::Error> {
    let mut output = Vec::new();
    let action = cli::parse_with_env(
        args.iter().copied(),
        mode,
        &mut output,
        |_| panic!("offline metadata must not read configuration"),
        Paths::new(None),
    )?;
    Ok((action, String::from_utf8(output).unwrap()))
}
#[test]
fn offline_help_version_completion_and_skill_use_the_live_native_command_tree() {
    for mode in [Mode::Human, Mode::Agent] {
        assert_eq!(
            parse(&["agent-ingester", "--version"], mode).unwrap().1,
            format!("{}\n", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            parse(&["agent-ingester", "skill"], mode).unwrap().1,
            cli::SKILL_POINTER
        );
        for path in [
            vec!["--help"],
            vec!["help"],
            vec!["help", "skill"],
            vec!["skill", "--help"],
            vec!["completion", "--help"],
            vec!["completion", "bash", "--help"],
        ] {
            let mut args = vec!["agent-ingester"];
            args.extend(path);
            let text = parse(&args, mode).unwrap().1;
            assert_eq!(
                text.starts_with("ALERT: Agents must read `AGENT=1 agent-ingester skill`"),
                mode == Mode::Agent
            );
        }
        for shell in ["bash", "zsh", "fish", "powershell"] {
            assert!(
                !parse(&["agent-ingester", "completion", shell], mode)
                    .unwrap()
                    .1
                    .is_empty()
            );
        }
        assert!(
            !parse(&["agent-ingester", "completion"], mode)
                .unwrap()
                .1
                .is_empty()
        );
    }
}
#[test]
fn invalid_values_and_write_failures_remain_typed_offline() {
    for path in [
        vec!["unknown"],
        vec!["skill", "extra"],
        vec!["--no-summarize=wrong"],
        vec!["--idle-grace-period", "bad"],
        vec!["--idle-grace-period", "-1s"],
        vec!["--log-level", "verbose"],
    ] {
        let mut args = vec!["agent-ingester"];
        args.extend(path);
        assert!(matches!(
            parse(&args, Mode::Agent),
            Err(cli::Error::Arguments(_))
        ));
    }
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    for path in [
        vec!["--help"],
        vec!["--version"],
        vec!["skill"],
        vec!["completion", "bash"],
        vec!["completion"],
    ] {
        let mut args = vec!["agent-ingester"];
        args.extend(path);
        assert!(matches!(
            cli::parse_with_env(args, Mode::Human, &mut Broken, |_| None, Paths::new(None)),
            Err(cli::Error::Io(_))
        ));
    }
}
#[test]
fn logging_and_console_fold_display_paths_without_changing_filesystem_options() {
    let Action::Serve(options) = cli::parse_with_env(
        [
            "agent-ingester",
            "--log-dir",
            "logs",
            "--log-file",
            "chosen.log",
            "--log-level",
            "DEBUG",
        ],
        Mode::Agent,
        &mut Vec::new(),
        |_| None,
        Paths::new(None),
    )
    .unwrap() else {
        panic!()
    };
    for mode in [Mode::Human, Mode::Agent] {
        let (logging, file) = cli::logging_options(&options, mode);
        assert_eq!(logging.level, "DEBUG");
        assert_eq!(
            logging.mode,
            if mode == Mode::Agent {
                agent_common::logging::Mode::Agent
            } else {
                agent_common::logging::Mode::Human
            }
        );
        assert_eq!(file.dir, std::path::PathBuf::from("logs"));
        assert_eq!(
            file.filename.unwrap(),
            std::path::PathBuf::from("chosen.log")
        );
    }
    let home = dirs::home_dir().unwrap();
    let text = format!(
        "home={} child={} sibling={}-sibling other=/elsewhere\n",
        home.display(),
        home.join("project").display(),
        home.display()
    );
    let mut console = cli::Console(Vec::new());
    console.write_all(text.as_bytes()).unwrap();
    console.flush().unwrap();
    let result = String::from_utf8(console.0).unwrap();
    assert!(result.contains("home=~ child=~/project"));
    assert!(result.contains(&format!("sibling={}-sibling", home.display())));
    assert!(result.contains("other=/elsewhere"));
    assert!(cli::Console(Vec::new()).write(&[255]).is_err());
}
#[test]
fn operational_skill_matches_every_live_command_and_option() {
    let skill = include_str!("../../../skills/agent-ingester/SKILL.md");
    assert!(skill.contains("name: agent-ingester\n"));
    assert!(cli::SKILL_POINTER.contains("--skill agent-ingester"));
    assert!(cli::SKILL_POINTER.contains("/skills/agent-ingester\n"));
    let omissions =
        cli_command_kit::skill_omissions(&cli::command(Mode::Agent), &cli::help_options(), skill);
    assert!(omissions.is_empty(), "skill omissions: {omissions:?}");
}
