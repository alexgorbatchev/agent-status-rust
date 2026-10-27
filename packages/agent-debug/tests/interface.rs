use agent_debug::{cli, output::Format};
use cli_command_kit::Mode;

#[test]
fn operational_skill_tracks_live_commands_flags_and_installation_pointer() {
    let text = concat!(
        include_str!("../../../skills/agent-debug/SKILL.md"),
        "\n",
        include_str!("../../../skills/agent-debug/references/help.md")
    );
    assert!(text.starts_with("---\nname: agent-debug\n"));
    let omissions =
        cli_command_kit::skill_omissions(&cli::command(Mode::Agent), &cli::help_options(), text);
    assert!(omissions.is_empty(), "{omissions:?}");
    assert!(std::path::Path::new("../../skills/agent-debug/references/setup.md").is_file());
    for mode in [Mode::Human, Mode::Agent] {
        let mut output = Vec::new();
        let format = Format {
            mode,
            width: 80,
            color: false,
            home: None,
        };
        assert!(
            cli::parse(["agent-debug", "skill"], &format, &mut output)
                .unwrap()
                .is_none()
        );
        assert_eq!(output, cli::SKILL_POINTER.as_bytes());
        assert!(cli::parse(["agent-debug", "skill", "extra"], &format, &mut Vec::new()).is_err());
    }
}

#[tokio::test]
async fn every_help_path_and_bare_subject_uses_the_native_agent_alert() {
    fn paths(command: &clap::Command, path: Vec<String>, found: &mut Vec<Vec<String>>) {
        found.push(path.clone());
        for child in command.get_subcommands() {
            let mut path = path.clone();
            path.push(child.get_name().into());
            paths(child, path, found);
        }
    }
    let mut all = Vec::new();
    paths(
        &cli::command(Mode::Agent),
        vec!["agent-debug".into()],
        &mut all,
    );
    let format = Format {
        mode: Mode::Agent,
        width: 80,
        color: false,
        home: None,
    };
    for mut path in all {
        // Clap's generated help subtree is a command path, rather than an
        // executable domain command accepting another --help option.
        if !path.iter().any(|segment| segment == "help") {
            path.push("--help".into());
        }
        let mut out = Vec::new();
        assert!(
            cli::parse(path.clone(), &format, &mut out)
                .unwrap()
                .is_none(),
            "{path:?}"
        );
        assert!(
            out.starts_with(b"ALERT: Agents must read `AGENT=1 agent-debug skill`"),
            "{path:?}"
        );
    }
    for subject in [
        "",
        "session",
        "fleet",
        "event",
        "db",
        "scanner",
        "process",
        "agent",
        "completion",
    ] {
        let mut args = vec!["agent-debug"];
        if !subject.is_empty() {
            args.push(subject);
        }
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            cli::run(
                args,
                &tokio_util::sync::CancellationToken::new(),
                &format,
                &mut out,
                &mut err
            )
            .await,
            0
        );
        assert!(
            out.starts_with(b"ALERT: Agents must read `AGENT=1 agent-debug skill`"),
            "{subject}"
        );
        assert!(err.is_empty());
    }
}
