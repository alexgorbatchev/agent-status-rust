use super::{Format, number, quote, sanitize, trim_project_prefix, truncate, visual_width};
use agent_common::types::{ActivityState, AgentState, HarnessType, LifecycleState};
use cli_command_kit::Mode;
use std::io::{self, Write};

pub fn completed(agent: &AgentState) -> bool {
    agent.activity.lifecycle == LifecycleState::Running
        && agent.activity.activity == ActivityState::Idle
        && agent.activity.has_unseen_completion == Some(true)
}
pub fn stale_idle(agent: &AgentState, now: i64) -> bool {
    agent.activity.lifecycle == LifecycleState::Running
        && agent.activity.activity == ActivityState::Idle
        && !completed(agent)
        && (agent.activity.updated_at <= 0
            || i128::from(now) - i128::from(agent.activity.updated_at) > 7_200_000)
}
pub fn activity_status(agent: &AgentState) -> &'static str {
    if completed(agent) {
        return "Completed";
    }
    if agent.activity.lifecycle == LifecycleState::Exited {
        return "Finished";
    }
    match agent.activity.activity {
        ActivityState::Busy => "Working",
        ActivityState::WaitingQuestion => "Waiting on Question",
        ActivityState::RateLimited => "Rate Limited",
        ActivityState::Error => "Error",
        _ => "Idle",
    }
}
fn fallback<'a>(value: &'a str, default: &'a str) -> &'a str {
    if value.is_empty() { default } else { value }
}
fn label(harness: &HarnessType) -> &str {
    match harness {
        HarnessType::ClaudeCode => "Claude Code",
        HarnessType::Pi => "Pi",
        HarnessType::Codex => "Codex",
        HarnessType::Opencode => "OpenCode",
        _ => fallback(harness.as_str(), "Unknown"),
    }
}
fn badge(agent: &AgentState) -> String {
    let code = match agent.activity.activity {
        ActivityState::Error => "31",
        ActivityState::WaitingQuestion | ActivityState::RateLimited => "33",
        _ => match agent.harness {
            HarnessType::ClaudeCode => "35",
            HarnessType::Pi => "1;35",
            HarnessType::Codex => "36",
            HarnessType::Opencode => "1;36",
            _ => "37",
        },
    };
    format!("\x1b[{code}m({})\x1b[0m", sanitize(label(&agent.harness)))
}
fn relative(timestamp: i64, now: i64) -> String {
    if timestamp <= 0 {
        return "never".into();
    }
    let delta = i128::from(now) - i128::from(timestamp);
    if delta < 5_000 {
        "just now".into()
    } else if delta < 60_000 {
        format!("{}s ago", delta / 1000)
    } else if delta < 3_600_000 {
        format!("{}m ago", delta / 60_000)
    } else if delta < 86_400_000 {
        format!("{}h ago", delta / 3_600_000)
    } else {
        format!("{}d ago", delta / 86_400_000)
    }
}
fn wait(agent: &AgentState, now: i64) -> String {
    let activity = &agent.activity;
    let seconds = if let Some(reset) = activity.rate_limit_reset_at {
        (i128::from(reset) - i128::from(now) + 999) / 1000
    } else {
        i128::from(activity.retry_after_seconds.unwrap_or_default())
            - (i128::from(now) - i128::from(activity.updated_at)) / 1000
    };
    if seconds <= 0 {
        return String::new();
    }
    let duration = if seconds <= 120 {
        format!("{seconds}s")
    } else if seconds >= 3600 {
        if seconds / 60 % 60 > 0 {
            format!("{}h {}m", seconds / 3600, seconds / 60 % 60)
        } else {
            format!("{}h", seconds / 3600)
        }
    } else if seconds % 60 > 0 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}m", seconds / 60)
    };
    format!("retry in {duration}")
}
fn callout(agent: &AgentState, now: i64) -> Option<(&'static str, &'static str, String)> {
    let activity = &agent.activity;
    let present = |v: &Option<String>| v.as_ref().filter(|s| !s.is_empty()).map(|s| sanitize(s));
    match activity.activity {
        ActivityState::WaitingQuestion => {
            present(&activity.question).map(|s| ("question", "[?]", s))
        }
        ActivityState::Error => present(&activity.last_error).map(|s| ("error", "[!]", s)),
        ActivityState::RateLimited => {
            let error = present(&activity.last_error);
            let wait = wait(agent, now);
            let message = match error {
                Some(message) if !wait.is_empty() && !message.contains(&wait) => {
                    format!("{message} ({wait})")
                }
                Some(message) => message,
                None if !wait.is_empty() => format!("Rate limit reached — {wait}"),
                None => "Rate limit reached — backoff/retry in progress".into(),
            };
            Some(("rate_limit", "[WARN]", message))
        }
        _ => None,
    }
}
fn action(agent: &AgentState) -> String {
    if !agent.activity.action.is_empty() {
        sanitize(&agent.activity.action)
    } else if let Some(tool) = agent
        .activity
        .active_tool
        .as_ref()
        .filter(|s| !s.is_empty())
    {
        format!("Executing {}", sanitize(tool))
    } else {
        "Idle".into()
    }
}
fn environment(agent: &AgentState) -> &str {
    fallback(
        &agent.environment.label,
        fallback(&agent.environment.id, "local-dev"),
    )
}
fn align(left: &str, right: &str, width: usize) -> String {
    let left = truncate(left, width.saturating_sub(visual_width(right) + 1), "...");
    let spaces = width
        .saturating_sub(visual_width(&left) + visual_width(right))
        .max(1);
    truncate(
        &format!("{left}{}{right}", " ".repeat(spaces)),
        width,
        "...",
    )
}
impl Format {
    pub fn card(&self, out: &mut impl Write, agent: &AgentState, now: i64) -> io::Result<()> {
        let a = &agent.activity;
        let title = self.text(fallback(&a.title, "Untitled Agent Session"));
        let branch = self.text(fallback(&agent.workspace.branch, "main"));
        let cost = a.cost.as_ref().map_or(0.0, |c| c.total);
        if self.mode == Mode::Agent {
            let mut parts = vec![
                format!(
                    "harness:{}",
                    self.text(fallback(agent.harness.as_str(), "unknown"))
                ),
                format!("env:{}", self.text(environment(agent))),
                format!("title:{}", quote(&title)),
                format!("action:{}", quote(&self.text(&action(agent)))),
                format!("activity:{}", fallback(a.activity.as_str(), "IDLE")),
            ];
            if completed(agent) {
                parts.push("has_unseen_completion:true".into());
            }
            if let Some((kind, _, message)) = callout(agent, now) {
                parts.push(format!("{kind}:{}", quote(&self.text(&message))));
            }
            parts.extend([
                format!("lifecycle:{}", fallback(a.lifecycle.as_str(), "RUNNING")),
                format!("branch:{branch}"),
            ]);
            if agent.workspace.is_worktree
                && let Some(path) = agent.workspace.rel_path.as_ref().filter(|s| !s.is_empty())
            {
                parts.push(format!("worktree:{}", self.text(path)));
            }
            if a.subagent_count > 0 {
                parts.push(format!("subagents:{}", a.subagent_count));
            }
            parts.extend([
                format!("cost_usd:{cost:.4}"),
                format!("tokens:{}", a.usage.total),
            ]);
            return self.line(out, &parts.join(" "));
        }
        let width = self.width.max(40);
        let divider = "-".repeat(width);
        let hide = if agent.is_hidden {
            "[Unhide]"
        } else {
            "[Hide]"
        };
        let focus = if a.lifecycle == LifecycleState::Running || a.lifecycle.as_str().is_empty() {
            "[Focus] "
        } else {
            ""
        };
        let state = if completed(agent) {
            "\x1b[32mCompleted\x1b[0m"
        } else if a.activity == ActivityState::RateLimited {
            "\x1b[33mRate Limited\x1b[0m"
        } else {
            fallback(a.lifecycle.as_str(), "RUNNING")
        };
        let header = align(
            &format!("{} {focus}{hide}", badge(agent)),
            &format!("({state})"),
            width,
        );
        let mut lines = vec![
            divider.clone(),
            header,
            divider.clone(),
            title.replace('\n', " "),
            self.text(&action(agent)).replace('\n', " "),
        ];
        if let Some((kind, glyph, message)) = callout(agent, now) {
            lines.push(String::new());
            lines.push(format!(
                "\x1b[{}m{glyph} {}\x1b[0m",
                if kind == "error" { 31 } else { 33 },
                self.text(&message).replace('\n', " ")
            ));
        }
        lines.push(String::new());
        let mut badges = vec![
            format!("({})", self.text(environment(agent)).replace('\n', " ")),
            format!("(branch: {branch})"),
        ];
        if agent.workspace.is_worktree
            && let Some(path) = agent.workspace.rel_path.as_ref().filter(|s| !s.is_empty())
        {
            badges.push(format!(
                "(worktree: {})",
                self.text(path).replace('\n', " ")
            ));
        }
        if a.subagent_count > 0 {
            badges.push(format!(
                "({} subagent{})",
                a.subagent_count,
                if a.subagent_count == 1 { "" } else { "s" }
            ));
        }
        for sub in &a.active_subagents {
            let detail = sub
                .active_tool
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| fallback(&sub.action, sub.activity.as_str()));
            badges.push(
                format!("(subagent {}: {})", self.text(&sub.id), self.text(detail))
                    .replace('\n', " "),
            );
        }
        badges.push(format!("(active: {})", relative(a.updated_at, now)));
        let mut row = String::new();
        for badge in badges {
            if !row.is_empty() && visual_width(&row) + 2 + visual_width(&badge) > width {
                lines.push(std::mem::take(&mut row));
            }
            if !row.is_empty() {
                row.push_str("  ");
            }
            row.push_str(&badge);
        }
        if !row.is_empty() {
            lines.push(row);
        }
        lines.extend([
            divider.clone(),
            align(
                &format!("${cost:.4} USD"),
                &format!("{} tokens", number(a.usage.total)),
                width,
            ),
            divider,
        ]);
        for line in lines {
            let line = truncate(&line, width, "...");
            if !line.is_empty() && stale_idle(agent, now) {
                self.line(
                    out,
                    &format!(
                        "\x1b[2m{}\x1b[0m",
                        line.replace("\x1b[0m", "\x1b[0m\x1b[2m")
                    ),
                )?;
            } else {
                self.line(out, &line)?;
            }
        }
        Ok(())
    }

    pub fn fleet(&self, out: &mut impl Write, agents: &[AgentState], now: i64) -> io::Result<()> {
        if agents.is_empty() {
            return if self.mode == Mode::Human {
                self.line(out, "No active or recent agent runs detected.")
            } else {
                Ok(())
            };
        }
        if self.mode == Mode::Agent {
            for agent in agents {
                write!(
                    out,
                    "project:{} is_worktree:{} ",
                    self.text(fallback(&agent.project.id, "unknown")),
                    agent.workspace.is_worktree
                )?;
                self.card(out, agent, now)?;
            }
            return Ok(());
        }
        let mut projects: Vec<(String, String, i64, Vec<&AgentState>)> = Vec::new();
        for agent in agents {
            let id = fallback(&agent.project.id, "unknown");
            if let Some(group) = projects.iter_mut().find(|g| g.0 == id) {
                group.2 = group.2.max(agent.activity.updated_at);
                group.3.push(agent);
            } else {
                projects.push((
                    id.into(),
                    fallback(&agent.project.name, id).into(),
                    agent.activity.updated_at,
                    vec![agent],
                ));
            }
        }
        projects.sort_by_key(|group| std::cmp::Reverse(group.2));
        for (index, (id, name, _, project)) in projects.into_iter().enumerate() {
            if index > 0 {
                self.line(out, "")?;
            }
            self.line(
                out,
                &truncate(&self.text(&format!("v {name} ({id})")), self.width, "..."),
            )?;
            let mut roots = Vec::new();
            let mut worktrees: Vec<(String, Vec<&AgentState>)> = Vec::new();
            for agent in project {
                if !agent.workspace.is_worktree {
                    roots.push(agent);
                    continue;
                }
                let key = agent
                    .workspace
                    .rel_path
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        fallback(
                            &agent.workspace.branch,
                            fallback(&agent.workspace.cwd, "unknown"),
                        )
                    });
                if let Some(group) = worktrees.iter_mut().find(|g| g.0 == key) {
                    group.1.push(agent);
                } else {
                    worktrees.push((key.into(), vec![agent]));
                }
            }
            sort_agents(&mut roots, now);
            if worktrees.is_empty() {
                for agent in roots {
                    self.fleet_line(out, agent, &name, "  ", now)?;
                }
                continue;
            }
            let mut branches = Vec::new();
            if !roots.is_empty() {
                branches.push((
                    format!("[{}]", fallback(&roots[0].workspace.branch, "HEAD")),
                    roots,
                ));
            }
            for (_, mut agents) in worktrees {
                sort_agents(&mut agents, now);
                let workspace = &agents[0].workspace;
                let mut header = format!("[worktree: {}]", fallback(&workspace.branch, "HEAD"));
                if let Some(path) = workspace.rel_path.as_ref().filter(|s| !s.is_empty()) {
                    header.push_str(&format!(" ({path})"));
                }
                branches.push((header, agents));
            }
            let total = branches.len();
            for (index, (header, agents)) in branches.into_iter().enumerate() {
                let last = index + 1 == total;
                let text = format!(
                    "{}{} ({} session{})",
                    if last { "  +- " } else { "  |- " },
                    header,
                    agents.len(),
                    if agents.len() == 1 { "" } else { "s" }
                );
                self.line(out, &truncate(&self.text(&text), self.width, "..."))?;
                for agent in agents {
                    self.fleet_line(
                        out,
                        agent,
                        &name,
                        if last { "       " } else { "  |    " },
                        now,
                    )?;
                }
            }
        }
        Ok(())
    }
    fn fleet_line(
        &self,
        out: &mut impl Write,
        agent: &AgentState,
        project: &str,
        indent: &str,
        now: i64,
    ) -> io::Result<()> {
        let title = self.text(fallback(&agent.activity.title, "Untitled Agent Session"));
        let title = trim_project_prefix(&title, &sanitize(project)).replace('\n', " ");
        let line = truncate(
            &format!(
                "{indent}{} {title} ({})",
                badge(agent),
                activity_status(agent)
            ),
            self.width,
            "...",
        );
        if stale_idle(agent, now) {
            self.line(
                out,
                &format!(
                    "\x1b[2m{}\x1b[0m",
                    line.replace("\x1b[0m", "\x1b[0m\x1b[2m")
                ),
            )
        } else {
            self.line(out, &line)
        }
    }
    pub fn agent_row(&self, out: &mut impl Write, agent: &AgentState) -> io::Result<()> {
        let clean = |s: &str| self.text(s).replace(['\n', '\t'], " ");
        self.line(
            out,
            &format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                agent.activity.current_run.as_ref().map_or(0, |run| run.pid),
                clean(agent.harness.as_str()),
                clean(&agent.session_id),
                clean(&agent.environment.hostname),
                clean(&agent.workspace.cwd),
                clean(&agent.activity.title)
            ),
        )
    }
}
fn sort_agents(agents: &mut [&AgentState], now: i64) {
    let rank = |a: &AgentState| {
        if a.activity.lifecycle == LifecycleState::Exited {
            2
        } else if a.activity.activity == ActivityState::Idle {
            1
        } else {
            0
        }
    };
    agents.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| {
            if rank(a) == 1 {
                stale_idle(a, now).cmp(&stale_idle(b, now))
            } else {
                b.activity.updated_at.cmp(&a.activity.updated_at)
            }
        })
    });
}
