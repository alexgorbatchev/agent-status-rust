//! Deterministic telemetry transitions. The ingestion owner commits outcomes and cursors atomically.
mod root;
mod subagent;
mod summary;
mod timers;

use crate::{pricing::Engine, summarizer::SummaryInput};
use agent_common::{models, types::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct Config {
    pub idle_grace: Duration,
    pub turn_floor: Duration,
    pub silence_timeout: Duration,
    pub rate_limit_buffer: Duration,
    pub subagent_dwell: Duration,
    pub debounce: Duration,
    pub synchronous: bool,
    pub summarize: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            idle_grace: Duration::from_secs(5),
            turn_floor: Duration::from_secs(20),
            silence_timeout: Duration::from_secs(1800),
            rate_limit_buffer: Duration::from_secs(120),
            subagent_dwell: Duration::from_secs(30),
            debounce: Duration::from_secs(10),
            synchronous: false,
            summarize: false,
        }
    }
}
#[derive(Clone, Debug)]
pub struct SummaryJob {
    pub input: SummaryInput,
    pub generation: u64,
    pub run_id: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Outcome {
    pub agent: AgentState,
    pub summary: Option<SummaryJob>,
}
#[derive(Clone, Debug)]
struct Candidate {
    input: SummaryInput,
    key: String,
    at: i64,
    generation: u64,
}
#[derive(Clone, Debug)]
struct Subagent {
    summary: SubagentSummary,
    question: Option<String>,
    reset: Option<i64>,
    retry: Option<i64>,
}
#[derive(Clone, Debug)]
struct Context {
    root: AgentActivity,
    aggregate: ActivityState,
    generation: u64,
    root_generation: u64,
    seen: BTreeSet<String>,
    subagents: BTreeMap<String, Subagent>,
    last_prompt: String,
    assistant: String,
    tool_args: String,
    recent: Vec<crate::summarizer::ToolExecution>,
    in_flight: bool,
    in_turn: bool,
    floor: i64,
    idle: Option<(i64, u64)>,
    silence: Option<(i64, u64)>,
    turn_ended: i64,
    last_event: i64,
    last_dispatched: Option<i64>,
    last_key: String,
    pending: Option<Candidate>,
    trailing: Option<i64>,
}
impl Context {
    fn restore(agent: &AgentState, seen: &[String]) -> Self {
        let mut root = agent.activity.clone();
        let mut subagents = BTreeMap::new();
        let mut known: BTreeSet<_> = seen.iter().cloned().collect();
        for summary in &agent.activity.active_subagents {
            let mut summary = summary.clone();
            if summary.status.as_str().is_empty() {
                summary.status = if summary.activity == ActivityState::Busy {
                    SubagentStatus::Running
                } else {
                    SubagentStatus::Completed
                };
            }
            let limited = summary.activity == ActivityState::RateLimited
                && root.activity == ActivityState::RateLimited;
            known.insert(summary.id.clone());
            subagents.insert(
                summary.id.clone(),
                Subagent {
                    summary,
                    question: None,
                    reset: limited.then_some(root.rate_limit_reset_at).flatten(),
                    retry: limited.then_some(root.retry_after_seconds).flatten(),
                },
            );
        }
        if root.active_tool.is_none()
            && root.activity == ActivityState::Busy
            && subagents
                .values()
                .any(|s| s.summary.activity == ActivityState::Busy)
        {
            root.activity = ActivityState::Idle;
            if root.action.starts_with("Running subagent ") {
                root.action = "Idle".into();
            }
        }
        Self {
            aggregate: agent.activity.activity.clone(),
            root,
            generation: 0,
            root_generation: 0,
            seen: known,
            subagents,
            last_prompt: String::new(),
            assistant: String::new(),
            tool_args: String::new(),
            recent: Vec::new(),
            in_flight: false,
            in_turn: false,
            floor: 0,
            idle: None,
            silence: None,
            turn_ended: 0,
            last_event: 0,
            last_dispatched: None,
            last_key: String::new(),
            pending: None,
            trailing: None,
        }
    }
    fn clear_completion(&mut self) {
        self.root.has_unseen_completion = None;
        self.root.completed_at = None;
    }
    fn clear_rate_limit(&mut self) {
        self.root.rate_limit_reset_at = None;
        self.root.retry_after_seconds = None;
    }
    fn clear_tool(&mut self) {
        self.root.active_tool = None;
        self.root.active_target = None;
        self.root.active_tool_started_at = None;
    }
    fn stop_turn(&mut self) {
        self.in_flight = false;
        self.in_turn = false;
        self.floor = 0;
    }
    fn prune(&mut self, now: i64, dwell: Duration) {
        let dwell = millis(dwell);
        self.subagents.retain(|_, s| {
            s.summary.completed_at.is_none()
                || (dwell > 0 && now < s.summary.started_at.saturating_add(dwell))
        });
    }
}

pub struct Classifier {
    pub config: Config,
    contexts: BTreeMap<String, Context>,
    pricing: Engine,
}
impl Classifier {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            contexts: BTreeMap::new(),
            pricing: Engine::default(),
        }
    }
    pub fn pricing_mut(&mut self) -> &mut Engine {
        &mut self.pricing
    }
    /// Consumes the caller's fresh database row; it never persists or acknowledges a frame.
    pub fn process(
        &mut self,
        existing: Option<AgentState>,
        frame: &TelemetryFrame,
        seen: &[String],
        clock: i64,
    ) -> Outcome {
        let now = if frame.timestamp == 0 {
            clock
        } else {
            frame.timestamp
        };
        let mut agent = existing.unwrap_or_else(|| new_agent(frame, now));
        agent.environment = frame.environment.clone();
        agent.workspace = frame.workspace.clone();
        agent.project = frame.project.clone();
        if !matches!(frame.event_type.as_str(), "run_exited" | "summary") {
            agent.activity.updated_at = now;
        }
        if agent.is_hidden
            && agent.hidden_at.is_none_or(|at| now > at)
            && root_state_event(&frame.event_type)
            && frame.event_type != "run_exited"
        {
            agent.is_hidden = false;
            agent.hidden_at = None;
        }
        let is_sub = frame.subagent_id.as_ref().is_some_and(|id| !id.is_empty());
        update_metrics(&mut agent, frame, is_sub, &self.pricing);
        let mut context = self
            .contexts
            .remove(&agent.id)
            .unwrap_or_else(|| Context::restore(&agent, seen));
        context.generation = context.generation.wrapping_add(1);
        if frame.event_type != "summary" {
            context.last_event = now;
        }
        if !is_sub && root_state_event(&frame.event_type) {
            context.root_generation = context.root_generation.wrapping_add(1);
        }
        // Metadata and token accumulation belong to the row; root state stays independent of subagent rollups.
        let latest = &agent.activity;
        context.root.updated_at = latest.updated_at;
        context.root.usage = latest.usage.clone();
        context.root.cost = latest.cost.clone();
        context.root.title = latest.title.clone();
        context.root.model = latest.model.clone();
        context.root.context_window = latest.context_window;
        context.root.context_fullness = latest.context_fullness;
        context.root.current_run = latest.current_run.clone();
        if let Some(prompt) = nonempty(frame.data.last_prompt.as_deref())
            && prompt != context.last_prompt
        {
            context.last_prompt = prompt.into();
            context.assistant.clear();
            context.tool_args.clear();
            context.recent.clear();
        }
        let summary = if is_sub {
            if frame.event_type != "summary" {
                context.subagent(frame, now, clock, &self.config);
            }
            None
        } else {
            context.root_event(&agent.id, frame, now, clock, &self.config)
        };
        context.prune(now, self.config.subagent_dwell);
        agent.activity = context.root.clone();
        agent.activity.subagent_count = agent
            .activity
            .subagent_count
            .max(context.seen.len().min(i64::MAX as usize) as i64);
        context.root.subagent_count = agent.activity.subagent_count;
        if agent.activity.lifecycle == LifecycleState::Exited {
            agent.activity.active_subagents.clear();
            agent.activity.rate_limit_reset_at = None;
            agent.activity.retry_after_seconds = None;
        } else {
            context.rollup(&mut agent.activity, now);
            context.sync_completion(&agent.activity);
            self.contexts.insert(agent.id.clone(), context);
        }
        Outcome { agent, summary }
    }
    /// Restores the classifier if the surrounding SQLite transaction fails.
    pub fn checkpoint(&self, id: &str) -> Checkpoint {
        Checkpoint(self.contexts.get(id).cloned())
    }
    pub fn rollback(&mut self, id: &str, checkpoint: Checkpoint) {
        match checkpoint.0 {
            Some(context) => {
                self.contexts.insert(id.into(), context);
            }
            None => {
                self.contexts.remove(id);
            }
        }
    }
    pub fn dismiss_completion(&mut self, id: &str) {
        if let Some(context) = self.contexts.get_mut(id) {
            context.clear_completion();
        }
    }
    pub fn remove(&mut self, id: &str) {
        self.contexts.remove(id);
    }
}
impl Context {
    fn sync_completion(&mut self, activity: &AgentActivity) {
        self.root.has_unseen_completion = activity.has_unseen_completion;
        self.root.completed_at = activity.completed_at;
        self.aggregate = activity.activity.clone();
    }
}
pub struct Checkpoint(Option<Context>);

fn new_agent(frame: &TelemetryFrame, now: i64) -> AgentState {
    AgentState {
        id: frame.agent_id.clone(),
        session_id: frame.session_id.clone(),
        harness: frame.harness.clone(),
        environment: frame.environment.clone(),
        project: frame.project.clone(),
        workspace: frame.workspace.clone(),
        activity: AgentActivity {
            lifecycle: if frame.event_type == "run_exited" {
                LifecycleState::Exited
            } else {
                LifecycleState::Running
            },
            activity: ActivityState::Idle,
            first_started_at: now,
            updated_at: now,
            current_run: (frame.event_type != "run_exited").then(|| run(frame, now)),
            ..Default::default()
        },
        ..Default::default()
    }
}
fn run(frame: &TelemetryFrame, now: i64) -> RunInfo {
    RunInfo {
        id: format!("{}:{}:{now}", frame.agent_id, frame.pid),
        pid: frame.pid,
        harness_version: frame.harness_version.clone(),
        started_at: now,
        ended_at: None,
    }
}
fn update_metrics(agent: &mut AgentState, frame: &TelemetryFrame, is_sub: bool, pricing: &Engine) {
    let data = &frame.data;
    let a = &mut agent.activity;
    if !is_sub {
        if let Some(model) = nonempty(data.model.as_deref()) {
            a.model = Some(model.into());
        }
        if let Some(window) = data.model_context_window.filter(|v| *v > 0) {
            a.context_window = Some(window);
        } else if let Some(model) = &a.model {
            a.context_window = Some(models::resolve_context_window(model));
        }
        if let Some(fullness) = data.context_fullness {
            a.context_fullness = Some(fullness);
        } else if let (Some(window), Some(usage)) =
            (a.context_window.filter(|v| *v > 0), data.usage.as_ref())
        {
            a.context_fullness = Some(models::calculate_context_fullness(usage.total, window));
        }
        if let Some(title) = nonempty(data.title.as_deref()) {
            a.title = title.into();
        }
    }
    if let Some(u) = &data.usage {
        a.usage.input = a.usage.input.saturating_add(u.input);
        a.usage.output = a.usage.output.saturating_add(u.output);
        a.usage.cache_read = a.usage.cache_read.saturating_add(u.cache_read);
        a.usage.cache_write = a.usage.cache_write.saturating_add(u.cache_write);
        a.usage.reasoning = a.usage.reasoning.saturating_add(u.reasoning);
        let total = if u.total > 0 {
            u.total
        } else {
            u.input
                .saturating_add(u.output)
                .saturating_add(u.cache_read)
                .saturating_add(u.cache_write)
        };
        a.usage.total = a.usage.total.saturating_add(total);
        if let Some(cost) = data
            .model
            .as_deref()
            .and_then(|model| pricing.calculate(model, u))
        {
            a.cost
                .get_or_insert_with(|| CostInfo {
                    currency: "USD".into(),
                    ..Default::default()
                })
                .total += cost;
        }
    }
}
fn millis(duration: Duration) -> i64 {
    duration.as_millis().min(i64::MAX as u128) as i64
}
fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
pub fn is_rate_limited(data: &EventData) -> bool {
    data.is_rate_limit
        .unwrap_or(data.rate_limit_reset_at.is_some() || data.retry_after_seconds.is_some())
}
fn root_state_event(event: &str) -> bool {
    matches!(
        event,
        "run_started"
            | "run_exited"
            | "turn_start"
            | "thinking"
            | "tool_call"
            | "tool_result"
            | "turn_end"
            | "question_prompt"
            | "waiting_question"
            | "error"
    )
}
pub fn rate_limit_silence(
    config: &Config,
    reset: Option<i64>,
    retry: Option<i64>,
    now: i64,
) -> Duration {
    if config.silence_timeout.is_zero() {
        return Duration::ZERO;
    }
    let delay = reset
        .map(|at| at.saturating_sub(now).max(0))
        .unwrap_or(0)
        .max(retry.unwrap_or(0).max(0).saturating_mul(1000))
        .min(86_400_000);
    let wait = Duration::from_millis(delay as u64);
    if wait > config.silence_timeout {
        wait.saturating_add(config.rate_limit_buffer)
    } else {
        config.silence_timeout
    }
}
