use agent_common::types::{
    ActivityState, AgentState, CostInfo, HarnessType, LifecycleState, SubagentSummary,
};
use agent_debug::output::{Format, activity_status, completed, sanitize, stale_idle, visual_width};
use cli_command_kit::Mode;
use std::{io, path::PathBuf};

fn agent(title: &str, activity: ActivityState, updated: i64) -> AgentState {
    let mut agent = AgentState {
        harness: HarnessType::Codex,
        ..Default::default()
    };
    agent.project.id = "project".into();
    agent.project.name = "Project".into();
    agent.activity.title = title.into();
    agent.activity.lifecycle = LifecycleState::Running;
    agent.activity.activity = activity;
    agent.activity.updated_at = updated;
    agent.workspace.branch = "main".into();
    agent
}

fn format(mode: Mode) -> Format {
    Format {
        mode,
        width: 100,
        color: false,
        home: Some(PathBuf::from("/home/fixture")),
    }
}

fn card(format: &Format, agent: &AgentState, now: i64) -> String {
    let mut bytes = Vec::new();
    format.card(&mut bytes, agent, now).unwrap();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn completion_and_idle_expiry_follow_lifecycle_and_exact_time_boundaries() {
    let mut agent = agent("Session", ActivityState::Idle, 1000);
    assert!(!completed(&agent));
    assert!(!stale_idle(&agent, 7_201_000));
    assert!(stale_idle(&agent, 7_201_001));
    assert!(!stale_idle(&agent, -1));
    agent.activity.has_unseen_completion = Some(true);
    assert!(completed(&agent));
    assert_eq!(activity_status(&agent), "Completed");
    assert!(!stale_idle(&agent, i64::MAX));
    agent.activity.lifecycle = LifecycleState::Exited;
    assert!(!completed(&agent));
    assert!(!stale_idle(&agent, i64::MAX));
    assert_eq!(activity_status(&agent), "Finished");
    agent.activity.lifecycle = LifecycleState::Running;
    agent.activity.has_unseen_completion = Some(false);
    agent.activity.updated_at = 0;
    assert!(stale_idle(&agent, 0));
    agent.activity.updated_at = i64::MIN;
    assert!(stale_idle(&agent, i64::MAX));
    for (activity, status) in [
        (ActivityState::Busy, "Working"),
        (ActivityState::WaitingQuestion, "Waiting on Question"),
        (ActivityState::RateLimited, "Rate Limited"),
        (ActivityState::Error, "Error"),
        (ActivityState::Unknown("new-state".into()), "Idle"),
    ] {
        agent.activity.activity = activity;
        assert_eq!(activity_status(&agent), status);
        assert!(!stale_idle(&agent, i64::MAX));
    }
}

#[test]
fn cards_render_harnesses_lifecycle_visibility_and_completion_without_terminal_injection() {
    let mut f = format(Mode::Human);
    f.color = true;
    for (harness, label, code) in [
        (HarnessType::ClaudeCode, "Claude Code", "35"),
        (HarnessType::Pi, "Pi", "1;35"),
        (HarnessType::Codex, "Codex", "36"),
        (HarnessType::Opencode, "OpenCode", "1;36"),
        (HarnessType::Unknown(String::new()), "Unknown", "37"),
        (HarnessType::Unknown("custom\x1b[2J".into()), "custom", "37"),
    ] {
        let mut a = agent("title\x1b]52;c;secret\x07", ActivityState::Busy, 1);
        a.harness = harness;
        let text = card(&f, &a, 2);
        assert!(
            text.contains(&format!("\x1b[{code}m({label})\x1b[0m")),
            "{text:?}"
        );
        assert!(text.contains("[Focus] [Hide]"));
        assert!(!text.contains("secret") && !text.contains("\x1b[2J"));
        a.is_hidden = true;
        a.activity.lifecycle = LifecycleState::Exited;
        let text = card(&f, &a, 2);
        assert!(text.contains("[Unhide]"));
        assert!(!text.contains("[Focus]"));
        assert!(text.contains("EXITED"));
    }
    let mut a = agent("", ActivityState::Idle, 1);
    a.activity.has_unseen_completion = Some(true);
    let text = card(&f, &a, 2);
    assert!(text.contains("Untitled Agent Session"));
    assert!(text.contains("\x1b[32mCompleted\x1b[0m"));
    a.activity.has_unseen_completion = None;
    let text = card(&f, &a, 8_000_000);
    assert!(text.contains("\x1b[2m"));
    f.color = false;
    assert!(!card(&f, &a, 8_000_000).contains('\x1b'));
}

#[test]
fn question_error_and_rate_limit_callouts_preserve_meaning_in_both_modes() {
    for mode in [Mode::Human, Mode::Agent] {
        let f = format(mode);
        let mut a = agent("Session", ActivityState::WaitingQuestion, 1000);
        a.activity.question = Some("May I inspect /home/fixture/repo?\x1b[2J".into());
        let text = card(&f, &a, 1000);
        assert!(text.contains("May I inspect ~/repo?"));
        assert!(!text.contains('\x1b'));
        a.activity.question = Some(String::new());
        assert!(!card(&f, &a, 1000).contains("May I inspect"));
        a.activity.activity = ActivityState::Error;
        a.activity.last_error = Some("Cannot read /home/fixture/config".into());
        assert!(card(&f, &a, 1000).contains("Cannot read ~/config"));
        a.activity.activity = ActivityState::RateLimited;
        a.activity.last_error = None;
        for (seconds, duration) in [
            (1, "1s"),
            (120, "120s"),
            (121, "2m 1s"),
            (180, "3m"),
            (3600, "1h"),
            (3660, "1h 1m"),
        ] {
            a.activity.retry_after_seconds = Some(seconds);
            assert!(card(&f, &a, 1000).contains(&format!("retry in {duration}")));
        }
        a.activity.rate_limit_reset_at = Some(2001);
        assert!(card(&f, &a, 1000).contains("retry in 2s"));
        a.activity.last_error = Some("Provider unavailable".into());
        assert!(card(&f, &a, 1000).contains("Provider unavailable (retry in 2s)"));
        a.activity.last_error = Some("Provider: retry in 2s".into());
        assert_eq!(card(&f, &a, 1000).matches("retry in 2s").count(), 1);
        a.activity.rate_limit_reset_at = Some(999);
        a.activity.last_error = Some("Provider unavailable".into());
        assert!(!card(&f, &a, 1000).contains("retry in"));
        a.activity.last_error = None;
        assert!(card(&f, &a, 1000).contains("backoff/retry in progress"));
    }
}

#[test]
fn active_work_metadata_distinguishes_tool_action_and_subagent_details() {
    let mut a = agent("Session", ActivityState::Busy, 1000);
    a.environment.id = "environment-id".into();
    a.workspace.is_worktree = true;
    a.workspace.rel_path = Some("/home/fixture/repo/.workspaces/fix".into());
    a.activity.active_tool = Some("Read\x1b[2J".into());
    a.activity.cost = Some(CostInfo {
        total: 1.25,
        ..Default::default()
    });
    a.activity.usage.total = -1234;
    a.activity.subagent_count = 1;
    a.activity.active_subagents = vec![
        SubagentSummary {
            id: "reader".into(),
            active_tool: Some("Read".into()),
            ..Default::default()
        },
        SubagentSummary {
            id: "writer".into(),
            action: "Editing".into(),
            ..Default::default()
        },
        SubagentSummary {
            id: "busy".into(),
            activity: ActivityState::Busy,
            ..Default::default()
        },
    ];
    let text = card(&format(Mode::Human), &a, 1000);
    for expected in [
        "Executing Read",
        "environment-id",
        "(branch: main)",
        "~/repo/.workspaces/fix",
        "(1 subagent)",
        "(subagent reader: Read)",
        "(subagent writer: Editing)",
        "(subagent busy: BUSY)",
        "$1.2500 USD",
        "-1,234 tokens",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    a.activity.action = "Reviewing".into();
    a.activity.subagent_count = 2;
    a.environment.label = "labeled".into();
    let text = card(&format(Mode::Human), &a, 1000);
    assert!(text.contains("Reviewing") && !text.contains("Executing Read"));
    assert!(text.contains("(2 subagents)") && text.contains("(labeled)"));
    let text = card(&format(Mode::Agent), &a, 1000);
    for expected in [
        "env:labeled",
        "action:\"Reviewing\"",
        "worktree:~/repo/.workspaces/fix",
        "subagents:2",
        "cost_usd:1.2500",
        "tokens:-1234",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}

#[test]
fn activity_age_handles_missing_future_and_all_display_units_without_overflow() {
    let mut a = agent("Session", ActivityState::Busy, 1000);
    for (now, expected) in [
        (1000, "just now"),
        (6000, "5s ago"),
        (61_000, "1m ago"),
        (3_601_000, "1h ago"),
        (86_401_000, "1d ago"),
        (i64::MIN, "just now"),
    ] {
        assert!(card(&format(Mode::Human), &a, now).contains(&format!("(active: {expected})")));
    }
    a.activity.updated_at = 0;
    assert!(card(&format(Mode::Human), &a, i64::MAX).contains("(active: never)"));
    a.activity.activity = ActivityState::RateLimited;
    a.activity.rate_limit_reset_at = Some(i64::MAX);
    assert!(card(&format(Mode::Agent), &a, i64::MIN).contains("retry in"));
}

#[test]
fn fleet_groups_projects_and_worktrees_then_ranks_active_recent_idle_and_exited_sessions() {
    let mut active_old = agent("Project: active-old", ActivityState::Busy, 1000);
    active_old.workspace.branch = "main".into();
    let active_new = agent("Project: active-new", ActivityState::Error, 2000);
    let idle_fresh = agent("Project: idle-fresh", ActivityState::Idle, 8_000_000);
    let idle_stale = agent("Project: idle-stale", ActivityState::Idle, 1);
    let mut exited = agent("Project: exited", ActivityState::Busy, 9_000_000);
    exited.activity.lifecycle = LifecycleState::Exited;
    let mut worktree = agent("Project: worktree", ActivityState::Busy, 5000);
    worktree.workspace.is_worktree = true;
    worktree.workspace.branch = "fix".into();
    worktree.workspace.rel_path = Some(".workspaces/fix".into());
    let mut worktree_two = worktree.clone();
    worktree_two.activity.title = "Project: worktree-two".into();
    let mut newest_project = agent("other", ActivityState::Idle, 10_000_000);
    newest_project.project.id = "other".into();
    newest_project.project.name = "Other".into();
    let agents = vec![
        idle_stale,
        exited,
        worktree,
        worktree_two,
        active_old,
        idle_fresh,
        active_new,
        newest_project,
    ];
    let mut bytes = Vec::new();
    format(Mode::Human)
        .fleet(&mut bytes, &agents, 10_000_000)
        .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let position = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("missing {needle}: {text}"))
    };
    assert!(position("v Other (other)") < position("v Project (project)"));
    assert!(position("active-new") < position("active-old"));
    assert!(position("active-old") < position("idle-fresh"));
    assert!(position("idle-fresh") < position("idle-stale"));
    assert!(position("idle-stale") < position("exited"));
    assert!(text.contains("[main] (5 sessions)"));
    assert!(text.contains("[worktree: fix] (.workspaces/fix) (2 sessions)"));
    assert!(!text.contains("Project: active"));
    assert!(text.lines().all(|line| visual_width(line) <= 100));
    let mut bytes = Vec::new();
    format(Mode::Agent)
        .fleet(&mut bytes, &agents, 10_000_000)
        .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text.lines().count(), 8);
    assert!(text.contains("project:project is_worktree:true harness:codex"));
    assert!(!text.contains('\x1b'));
}

#[test]
fn empty_and_fallback_fleets_remain_useful_and_every_renderer_propagates_write_errors() {
    let mut bytes = Vec::new();
    format(Mode::Human).fleet(&mut bytes, &[], 0).unwrap();
    assert_eq!(bytes, b"No active or recent agent runs detected.\n");
    bytes.clear();
    format(Mode::Agent).fleet(&mut bytes, &[], 0).unwrap();
    assert!(bytes.is_empty());
    let mut a = AgentState::default();
    a.workspace.is_worktree = true;
    let mut b = a.clone();
    b.workspace.branch = "branch-only".into();
    let mut c = a.clone();
    c.workspace.cwd = "/home/fixture/cwd-only".into();
    format(Mode::Human)
        .fleet(&mut bytes, &[a.clone(), b, c], 0)
        .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("v unknown (unknown)"));
    assert!(text.contains("[worktree: HEAD]"));
    assert!(text.contains("[worktree: branch-only]"));
    assert_eq!(text.matches("Untitled Agent Session").count(), 3);
    struct Broken;
    impl io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    for mode in [Mode::Human, Mode::Agent] {
        assert_eq!(
            format(mode).card(&mut Broken, &a, 0).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            format(mode)
                .fleet(&mut Broken, &[a.clone()], 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }
    assert_eq!(sanitize("\x1b[31mplain\x1b[0m"), "plain");
}
