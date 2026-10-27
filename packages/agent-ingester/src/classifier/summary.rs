use super::*;
use crate::summarizer::{SummaryResult, fallback, is_near_duplicate};

impl Context {
    fn input(&self, id: &str) -> SummaryInput {
        SummaryInput {
            agent_id: id.into(),
            current_action: self.root.action.clone(),
            last_prompt: self.last_prompt.clone(),
            last_assistant_response: self.assistant.clone(),
            recent_tools: self.recent.clone(),
            last_thinking_excerpt: self.root.last_thinking_excerpt.clone().unwrap_or_default(),
            activity: self.root.activity.clone(),
            active_tool: self.root.active_tool.clone(),
            active_target: self.root.active_target.clone(),
            active_tool_args: (!self.tool_args.is_empty()).then(|| self.tool_args.clone()),
            last_error: self.root.last_error.clone(),
            question: self.root.question.clone(),
            ..Default::default()
        }
    }
    pub(super) fn summarize(
        &mut self,
        id: &str,
        force: bool,
        event_time: i64,
        clock: i64,
        config: &Config,
    ) -> Option<SummaryJob> {
        let input = self.input(id);
        let result = fallback(&input);
        apply(&mut self.root, result);
        if self.root.activity == ActivityState::RateLimited {
            self.root.action_stale = false;
            self.pending = None;
            self.trailing = None;
            return None;
        }
        if !config.summarize {
            self.root.action_stale = false;
            return None;
        }
        let key = self
            .root
            .active_tool
            .as_ref()
            .map(|tool| {
                format!(
                    "{tool}|{}|{}",
                    self.root.active_target.as_deref().unwrap_or_default(),
                    self.tool_args
                )
            })
            .unwrap_or_default();
        if !force {
            self.root.active_tool.as_ref()?;
            if !key.is_empty() && key == self.last_key {
                self.root.action_stale = false;
                return None;
            }
            let elapsed = self.last_dispatched.map(|at| event_time.saturating_sub(at));
            if let Some(elapsed) = elapsed
                && elapsed < millis(config.debounce)
            {
                self.pending = Some(Candidate {
                    input,
                    key,
                    at: event_time,
                    generation: self.generation,
                });
                self.trailing = Some(
                    clock.saturating_add(millis(config.debounce).saturating_sub(elapsed).max(1)),
                );
                self.root.action_stale = self.root.activity == ActivityState::Busy;
                return None;
            }
        }
        self.pending = None;
        self.trailing = None;
        self.last_dispatched = Some(event_time);
        self.last_key = key;
        self.root.action_stale = false;
        Some(SummaryJob {
            input,
            generation: self.generation,
            run_id: self.root.current_run.as_ref().map(|r| r.id.clone()),
        })
    }
}

impl Classifier {
    pub fn summary_is_current(&self, agent: &AgentState, job: &SummaryJob) -> bool {
        self.contexts
            .get(&agent.id)
            .is_some_and(|context| context.generation == job.generation)
            && agent
                .activity
                .current_run
                .as_ref()
                .map(|run| run.id.clone())
                == job.run_id
            && agent.activity.lifecycle == LifecycleState::Running
            && !agent
                .activity
                .current_run
                .as_ref()
                .is_some_and(|run| run.ended_at.is_some())
            && agent.activity.activity != ActivityState::RateLimited
            && agent.activity.activity == job.input.activity
            && agent.activity.active_tool == job.input.active_tool
    }
    /// Applies only to the same run and saved frame generation, using the caller's fresh row.
    pub fn apply_summary(
        &mut self,
        agent: &mut AgentState,
        job: &SummaryJob,
        result: SummaryResult,
        now: i64,
    ) -> bool {
        if result.action.is_empty() || !self.summary_is_current(agent, job) {
            return false;
        }
        let context = self
            .contexts
            .get_mut(&agent.id)
            .expect("current summary has a context");
        let duplicate = is_near_duplicate(&agent.activity.action, &result.action, 0.6)
            && agent.activity.phase == result.phase
            && agent.activity.active_target == result.active_target;
        if duplicate && !agent.activity.action_stale {
            return false;
        }
        if !duplicate {
            apply(&mut agent.activity, result);
            agent.activity.updated_at = now;
        }
        agent.activity.action_stale = false;
        context.root.action = agent.activity.action.clone();
        context.root.phase = agent.activity.phase.clone();
        context.root.action_stale = false;
        context.root.active_target = agent.activity.active_target.clone();
        context.root.updated_at = agent.activity.updated_at;
        true
    }
}
fn apply(activity: &mut AgentActivity, result: SummaryResult) {
    activity.action = result.action;
    activity.phase = result.phase;
    if activity.active_tool.is_some()
        && result
            .active_target
            .as_ref()
            .is_some_and(|target| !target.is_empty())
    {
        activity.active_target = result.active_target;
    }
}
