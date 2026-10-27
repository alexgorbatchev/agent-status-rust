use agent_common::types::{ActivityState, AgentState, HarnessType, LifecycleState};
use agent_debug::output::{
    Format, sanitize, trim_project_prefix, truncate, truncate_left, visual_width,
};
use cli_command_kit::Mode;
use std::{io, path::PathBuf};

fn format(mode: Mode) -> Format {
    Format {
        mode,
        width: 60,
        color: false,
        home: Some(PathBuf::from("/home/fixture")),
    }
}
fn agent() -> AgentState {
    let mut a = AgentState {
        harness: HarnessType::Pi,
        session_id: "s".into(),
        ..Default::default()
    };
    a.project.id = "project".into();
    a.project.name = "project".into();
    a.activity.lifecycle = LifecycleState::Running;
    a.activity.activity = ActivityState::Idle;
    a.activity.title = "project: Repair 界界".into();
    a.activity.usage.total = 14520;
    a.activity.updated_at = 1000;
    a.workspace.cwd = "/home/fixture/repo".into();
    a
}

#[test]
fn sanitizes_complete_and_incomplete_terminal_sequences_and_preserves_formatting() {
    assert_eq!(
        sanitize(
            "a\x1b[31mred\x1b[0m\x1b]52;c;secret\x07b\u{009d}8;url\u{009c}c\x00\r\u{009f}hidden\x1b\\d\n\t界"
        ),
        "aredbcd\n\t界"
    );
    assert_eq!(sanitize("text\x1b]52;c;secret"), "text");
    assert_eq!(sanitize("\x1b[2Jbad\x1bPprivate\x1b\\ok"), "badok");
    assert_eq!(visual_width("\x1b[35m界·a\x1b[0m"), 4);
    assert_eq!(truncate("界界界", 5, "..."), "界...");
    assert_eq!(truncate("e\u{301}界abc", 4, "..."), "e\u{301}...");
    assert_eq!(truncate_left("界界abc", 6, "..."), "...abc");
    assert_eq!(truncate("hello", 2, "..."), "..");
    assert_eq!(truncate("hello", 0, "..."), "");
}

#[test]
fn strips_project_prefix_only_at_verified_boundaries() {
    assert_eq!(
        trim_project_prefix(" [ project ] — Repair", "/owner/project.git"),
        "Repair"
    );
    assert_eq!(trim_project_prefix("project: Repair", "project"), "Repair");
    for title in [
        "projector: Repair",
        "Project: Repair",
        "project - ",
        "project: ]",
        "project:Repair",
    ] {
        assert_eq!(trim_project_prefix(title, "project"), title);
    }
}

#[test]
fn cards_and_fleets_keep_completion_rate_limit_and_unicode_layout() {
    let mut a = agent();
    let mut out = Vec::new();
    format(Mode::Human).card(&mut out, &a, 9_000_000).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("project: Repair 界界"));
    assert!(text.contains("14,520 tokens"));
    assert!(!text.contains('\x1b'));
    assert!(text.lines().all(|line| visual_width(line) <= 60));
    a.activity.has_unseen_completion = Some(true);
    let mut out = Vec::new();
    format(Mode::Agent).card(&mut out, &a, 9_000_000).unwrap();
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("has_unseen_completion:true")
    );
    a.activity.activity = ActivityState::RateLimited;
    a.activity.retry_after_seconds = Some(180);
    a.activity.updated_at = 1000;
    let mut out = Vec::new();
    format(Mode::Agent).card(&mut out, &a, 2000).unwrap();
    assert!(String::from_utf8(out).unwrap().contains("retry in 2m 59s"));
    a.activity.activity = ActivityState::Idle;
    let mut b = a.clone();
    b.workspace.is_worktree = true;
    b.workspace.rel_path = Some(".workspaces/fix".into());
    b.workspace.branch = "fix".into();
    let mut out = Vec::new();
    format(Mode::Human).fleet(&mut out, &[a, b], 2000).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("v project (project)"));
    assert!(text.contains("[worktree: fix]"));
    assert!(text.contains("Repair 界界"));
    assert!(!text.contains("project: Repair"));
}

#[test]
fn folds_home_only_for_display_and_propagates_output_failures() {
    struct Broken;
    impl io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let f = format(Mode::Agent);
    assert_eq!(f.text("/home/fixture"), "~");
    assert_eq!(f.text("/home/fixture/repo"), "~/repo");
    assert_eq!(f.text("/home/fixtures/repo"), "/home/fixtures/repo");
    assert_eq!(f.text("/outside/repo"), "/outside/repo");
    assert!(f.card(&mut Broken, &agent(), 1000).is_err());
    let mut a = agent();
    a.activity.current_run = Some(agent_common::types::RunInfo {
        pid: 42,
        ..Default::default()
    });
    a.activity.title = "bad\n\ttitle\x1b[2J".into();
    let mut out = Vec::new();
    f.agent_row(&mut out, &a).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "42\tpi\ts\t\t~/repo\tbad  title\n"
    );
}

#[test]
fn colored_human_reports_sanitize_native_paths() {
    let mut f = format(Mode::Human);
    f.color = true;
    let path = PathBuf::from("/home/fixture/repo/\x1b[2Jbad");
    let report =
        agent_debug::validation::Report::failed(path.clone(), &agent_debug::Error::Cancelled);
    let comparison = agent_debug::comparison::Comparison {
        db_path: path,
        session_id: "s".into(),
        agent_id: "a".into(),
        fields: vec![],
        warnings: vec![],
        has_drift: false,
    };
    let mut out = Vec::new();
    f.validation(&mut out, &report).unwrap();
    f.comparison(&mut out, &comparison).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(!text.contains('\x1b'), "{text:?}");
    assert!(text.contains("~/repo/bad"));
}
