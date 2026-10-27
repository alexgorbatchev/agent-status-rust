use super::*;

impl Classifier {
    /// The runtime schedules one native Tokio deadline for the earliest pending transition.
    pub fn next_deadline(&self) -> Option<i64> {
        self.contexts
            .values()
            .flat_map(|context| deadlines(context, &self.config))
            .min()
    }
    pub fn due_agents(&self, now: i64) -> Vec<String> {
        self.contexts
            .iter()
            .filter(|(_, context)| {
                deadlines(context, &self.config)
                    .into_iter()
                    .any(|at| at <= now)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }
    /// Runs due work against a fresh row. Serial ingestion eliminates cross-query save races.
    pub fn tick(&mut self, mut agent: AgentState, now: i64) -> Option<Outcome> {
        if agent.activity.lifecycle != LifecycleState::Running {
            self.contexts.remove(&agent.id);
            return None;
        }
        let context = self.contexts.get_mut(&agent.id)?;
        let mut changed = false;
        let mut summary = None;
        if context.idle.is_some_and(|(at, _)| at <= now) {
            let (_, generation) = context.idle.take()?;
            if generation == context.root_generation
                && context.root.activity == ActivityState::Busy
                && !context.in_flight
                && (!context.in_turn || now >= context.floor)
            {
                context.silence = None;
                context.stop_turn();
                context.root.has_unseen_completion = Some(true);
                context.root.completed_at = Some(if context.turn_ended != 0 {
                    context.turn_ended
                } else if context.last_event != 0 {
                    context.last_event
                } else {
                    now
                });
                context.root.activity = ActivityState::Idle;
                context.clear_tool();
                context.root.question = None;
                context.clear_rate_limit();
                context.root.updated_at = now;
                summary = context.summarize(&agent.id, true, now, now, &self.config);
                changed = true;
            }
        }
        if context.silence.is_some_and(|(at, _)| at <= now) {
            let (_, generation) = context.silence.take()?;
            if generation == context.root_generation
                && (matches!(
                    context.root.activity,
                    ActivityState::Busy | ActivityState::RateLimited
                ) || context
                    .subagents
                    .values()
                    .any(|sub| sub.summary.activity == ActivityState::RateLimited))
            {
                context.stop_turn();
                context.clear_completion();
                context.root.activity = ActivityState::Idle;
                context.clear_tool();
                context.root.question = None;
                context.clear_rate_limit();
                context.root.updated_at = now;
                for sub in context.subagents.values_mut() {
                    sub.summary.activity = ActivityState::Idle;
                    sub.summary.active_tool = None;
                    sub.reset = None;
                    sub.retry = None;
                    sub.summary.completed_at.get_or_insert(now);
                    sub.summary.status = SubagentStatus::Completed;
                }
                // Inactivity is not a completed user turn, even when the previous aggregate was busy.
                context.aggregate = ActivityState::Idle;
                summary = context.summarize(&agent.id, true, now, now, &self.config);
                changed = true;
            }
        }
        let previous = context.subagents.len();
        context.prune(now, self.config.subagent_dwell);
        changed |= previous != context.subagents.len();
        if context.trailing.is_some_and(|at| at <= now) {
            context.trailing = None;
            if let Some(candidate) = context.pending.take()
                && candidate.input.activity != ActivityState::RateLimited
                && context.root.activity != ActivityState::RateLimited
                && candidate.key != context.last_key
                && candidate.generation == context.generation
            {
                context.last_key = candidate.key;
                context.last_dispatched = Some(candidate.at);
                summary = Some(SummaryJob {
                    input: candidate.input,
                    generation: candidate.generation,
                    run_id: context.root.current_run.as_ref().map(|run| run.id.clone()),
                });
            }
        }
        if changed {
            agent.activity = context.root.clone();
            context.rollup(&mut agent.activity, now);
            context.sync_completion(&agent.activity);
        }
        (changed || summary.is_some()).then_some(Outcome { agent, summary })
    }
    /// Makes trailing work available immediately for an explicit replay/flush operation.
    pub fn flush_trailing(&mut self, id: &str, now: i64) {
        if let Some(context) = self.contexts.get_mut(id)
            && context.pending.is_some()
        {
            context.trailing = Some(now);
        }
    }
}
fn deadlines(context: &Context, config: &Config) -> Vec<i64> {
    let mut times = Vec::new();
    times.extend(context.idle.map(|(at, _)| at));
    times.extend(context.silence.map(|(at, _)| at));
    times.extend(context.trailing);
    if !config.synchronous {
        times.extend(
            context
                .subagents
                .values()
                .filter(|sub| sub.summary.completed_at.is_some())
                .map(|sub| {
                    sub.summary
                        .started_at
                        .saturating_add(millis(config.subagent_dwell))
                }),
        );
    }
    times
}
