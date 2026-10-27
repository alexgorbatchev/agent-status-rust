use super::*;

impl Context {
    pub(super) fn subagent(
        &mut self,
        frame: &TelemetryFrame,
        now: i64,
        clock: i64,
        config: &Config,
    ) {
        let Some(id) = frame.subagent_id.as_ref().filter(|id| !id.is_empty()) else {
            return;
        };
        self.seen.insert(id.clone());
        let sub = self
            .subagents
            .entry(id.clone())
            .or_insert_with(|| Subagent {
                summary: SubagentSummary {
                    id: id.clone(),
                    activity: ActivityState::Busy,
                    started_at: now,
                    status: SubagentStatus::Running,
                    ..Default::default()
                },
                question: None,
                reset: None,
                retry: None,
            });
        if sub.summary.started_at == 0 {
            sub.summary.started_at = now;
        }
        if !matches!(frame.event_type.as_str(), "turn_end" | "run_exited") {
            sub.summary.completed_at = None;
            sub.summary.status = SubagentStatus::Running;
        }
        let data = &frame.data;
        let mut limited = false;
        match frame.event_type.as_str() {
            "tool_call" => {
                sub.summary.completed_at = None;
                sub.summary.status = SubagentStatus::Running;
                sub.summary.last_error = None;
                sub.reset = None;
                sub.retry = None;
                if data.tool_name.as_deref() == Some("AskUserQuestion") {
                    sub.summary.activity = ActivityState::WaitingQuestion;
                    sub.summary.active_tool = None;
                    sub.question = question(data).or_else(|| {
                        data.tool_input.as_ref().and_then(|input| {
                            ["question", "prompt"].into_iter().find_map(|key| {
                                nonempty(input.get(key).and_then(serde_json::Value::as_str))
                                    .map(str::to_owned)
                            })
                        })
                    });
                } else {
                    sub.summary.activity = ActivityState::Busy;
                    sub.summary.active_tool = data.tool_name.clone();
                    sub.question = None;
                    if let Some(tool) = &data.tool_name {
                        sub.summary.action = format!("Running {tool}");
                    }
                }
            }
            "tool_result" => {
                sub.summary.activity = ActivityState::Busy;
                sub.summary.active_tool = None;
                sub.summary.last_error = None;
                sub.reset = None;
                sub.retry = None;
            }
            "error" => {
                limited = is_rate_limited(data);
                sub.summary.activity = if limited {
                    ActivityState::RateLimited
                } else {
                    ActivityState::Error
                };
                sub.reset = if limited {
                    data.rate_limit_reset_at
                } else {
                    None
                };
                sub.retry = if limited {
                    data.retry_after_seconds
                } else {
                    None
                };
                if let Some(content) = data.content.as_ref().or(data.tool_output.as_ref()) {
                    sub.summary.last_error = Some(content.chars().take(2000).collect());
                }
                sub.summary.active_tool = None;
            }
            "question_prompt" | "waiting_question" => {
                sub.summary.activity = ActivityState::WaitingQuestion;
                sub.reset = None;
                sub.retry = None;
                if let Some(question) = question(data) {
                    sub.question = Some(question);
                }
                sub.summary.active_tool = None;
            }
            "turn_start" => {
                sub.summary.activity = ActivityState::Busy;
                sub.summary.active_tool = None;
                sub.summary.last_error = None;
                sub.reset = None;
                sub.retry = None;
                sub.summary.completed_at = None;
                sub.summary.status = SubagentStatus::Running;
            }
            "turn_end" | "run_exited" => {
                sub.summary.activity = ActivityState::Idle;
                sub.reset = None;
                sub.retry = None;
                sub.summary.active_tool = None;
                sub.summary.completed_at.get_or_insert(now);
                sub.summary.status = SubagentStatus::Completed;
            }
            _ => {}
        }
        if limited {
            self.schedule_silence(
                clock,
                rate_limit_silence(
                    config,
                    data.rate_limit_reset_at,
                    data.retry_after_seconds,
                    now,
                ),
                config,
            );
        }
    }

    pub(super) fn rollup(&self, activity: &mut AgentActivity, now: i64) {
        activity.active_subagents = self.subagents.values().map(|s| s.summary.clone()).collect();
        let was_busy = self.aggregate == ActivityState::Busy;
        // BTreeMap iteration makes the chosen attention source deterministic by subagent ID.
        let find = |state: &ActivityState| {
            self.subagents
                .values()
                .find(|sub| &sub.summary.activity == state)
        };
        let chosen = if self.root.activity == ActivityState::Error {
            None
        } else {
            find(&ActivityState::Error)
        };
        let state = if self.root.activity == ActivityState::Error || chosen.is_some() {
            ActivityState::Error
        } else if self.root.activity == ActivityState::WaitingQuestion
            || find(&ActivityState::WaitingQuestion).is_some()
        {
            ActivityState::WaitingQuestion
        } else if self.root.activity == ActivityState::RateLimited
            || find(&ActivityState::RateLimited).is_some()
        {
            ActivityState::RateLimited
        } else if self.root.activity == ActivityState::Busy || find(&ActivityState::Busy).is_some()
        {
            ActivityState::Busy
        } else {
            ActivityState::Idle
        };
        activity.activity = state.clone();
        activity.last_error = self.root.last_error.clone();
        activity.question = self.root.question.clone();
        if state != ActivityState::Idle {
            activity.has_unseen_completion = None;
            activity.completed_at = None;
        }
        activity.rate_limit_reset_at = None;
        activity.retry_after_seconds = None;
        match state {
            ActivityState::Error => {
                if let Some(sub) = chosen {
                    activity.last_error = sub
                        .summary
                        .last_error
                        .as_ref()
                        .map(|error| format!("[Subagent {}] {error}", sub.summary.id));
                }
            }
            ActivityState::WaitingQuestion => {
                if self.root.activity != ActivityState::WaitingQuestion
                    && let Some(sub) = find(&ActivityState::WaitingQuestion)
                {
                    activity.question = sub
                        .question
                        .as_ref()
                        .map(|question| format!("[Subagent {}] {question}", sub.summary.id));
                }
            }
            ActivityState::RateLimited => {
                if self.root.activity == ActivityState::RateLimited {
                    activity.rate_limit_reset_at = self.root.rate_limit_reset_at;
                    activity.retry_after_seconds = self.root.retry_after_seconds;
                } else if let Some(sub) = find(&ActivityState::RateLimited) {
                    activity.last_error = sub
                        .summary
                        .last_error
                        .as_ref()
                        .map(|error| format!("[Subagent {}] {error}", sub.summary.id));
                    activity.rate_limit_reset_at = sub.reset;
                    activity.retry_after_seconds = sub.retry;
                }
            }
            ActivityState::Busy => {
                if self.root.activity == ActivityState::Busy {
                    activity.action = self.root.action.clone();
                } else if let Some(sub) = find(&ActivityState::Busy) {
                    activity.action =
                        if let Some(tool) = nonempty(sub.summary.active_tool.as_deref()) {
                            format!("Running subagent {}: {tool}", sub.summary.id)
                        } else if !sub.summary.action.is_empty() {
                            format!(
                                "Running subagent {}: {}",
                                sub.summary.id, sub.summary.action
                            )
                        } else {
                            format!("Running subagent {} task", sub.summary.id)
                        };
                }
            }
            ActivityState::Idle => {
                if was_busy {
                    activity.has_unseen_completion = Some(true);
                    activity.completed_at = Some(now);
                }
                if !self.subagents.is_empty() {
                    activity.action =
                        if self.root.action.is_empty() || self.root.action == "Thinking" {
                            "Idle".into()
                        } else {
                            self.root.action.clone()
                        };
                }
            }
            ActivityState::Unknown(_) => {}
        }
    }
}
fn question(data: &EventData) -> Option<String> {
    nonempty(data.question.as_deref())
        .or_else(|| nonempty(data.content.as_deref()))
        .map(str::to_owned)
}
