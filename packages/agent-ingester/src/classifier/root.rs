use super::*;
use crate::summarizer::{ToolExecution, sanitize_tool_input};

impl Context {
    pub(super) fn schedule_silence(&mut self, clock: i64, timeout: Duration, config: &Config) {
        self.silence = (!config.synchronous && !config.silence_timeout.is_zero())
            .then(|| (clock.saturating_add(millis(timeout)), self.root_generation));
    }
    pub(super) fn root_event(
        &mut self,
        id: &str,
        frame: &TelemetryFrame,
        now: i64,
        clock: i64,
        config: &Config,
    ) -> Option<SummaryJob> {
        let data = &frame.data;
        match frame.event_type.as_str() {
            "summary" => {
                if nonempty(data.content.as_deref()).is_some()
                    && self
                        .root
                        .summary_updated_at
                        .is_none_or(|last| frame.timestamp >= last)
                {
                    self.root.summary = data.content.clone();
                    self.root.summary_updated_at = Some(frame.timestamp);
                }
            }
            "run_started" => {
                self.idle = None;
                self.silence = None;
                self.stop_turn();
                if let Some(run) = &mut self.root.current_run
                    && run.ended_at.is_none()
                    && self.root.lifecycle == LifecycleState::Running
                    && (run.pid == frame.pid || run.pid == 0 || frame.pid == 0)
                {
                    if run.pid == 0 && frame.pid != 0 {
                        run.pid = frame.pid;
                    }
                } else {
                    self.root.lifecycle = LifecycleState::Running;
                    self.root.activity = ActivityState::Idle;
                    self.clear_completion();
                    self.root.current_run = Some(run(frame, now));
                    if self.root.action.is_empty() {
                        return self.summarize(id, false, now, clock, config);
                    }
                }
            }
            "run_exited" => {
                self.idle = None;
                self.silence = None;
                self.trailing = None;
                self.pending = None;
                self.stop_turn();
                self.root.lifecycle = LifecycleState::Exited;
                self.clear_completion();
                self.clear_tool();
                self.clear_rate_limit();
                self.root.active_subagents.clear();
                if let Some(run) = &mut self.root.current_run {
                    run.ended_at = Some(now);
                }
            }
            "question_prompt" | "waiting_question" => {
                self.idle = None;
                self.silence = None;
                self.in_flight = false;
                self.root.activity = ActivityState::WaitingQuestion;
                self.clear_completion();
                self.clear_tool();
                self.clear_rate_limit();
                if data.question.is_some() {
                    self.root.question = data.question.clone();
                }
                return self.summarize(id, true, now, clock, config);
            }
            "error" => {
                self.idle = None;
                self.in_flight = false;
                if is_rate_limited(data) {
                    self.root.activity = ActivityState::RateLimited;
                    self.root.rate_limit_reset_at = data.rate_limit_reset_at;
                    self.root.retry_after_seconds = data.retry_after_seconds;
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
                } else {
                    self.root.activity = ActivityState::Error;
                    self.clear_rate_limit();
                    self.silence = None;
                }
                self.clear_completion();
                self.clear_tool();
                if let Some(error) = &data.content {
                    self.root.last_error = Some(error.chars().take(2000).collect());
                }
                return self.summarize(id, true, now, clock, config);
            }
            "tool_result" => {
                self.idle = None;
                self.in_flight = false;
                self.schedule_silence(clock, config.silence_timeout, config);
                self.root.activity = ActivityState::Busy;
                self.clear_rate_limit();
                self.clear_completion();
                self.root.last_error = None;
                self.clear_tool();
                return self.summarize(id, false, now, clock, config);
            }
            "tool_call" => {
                self.idle = None;
                self.in_flight = true;
                self.floor = 0;
                self.schedule_silence(clock, config.silence_timeout, config);
                self.root.activity = ActivityState::Busy;
                self.clear_completion();
                self.clear_rate_limit();
                self.root.last_error = None;
                self.root.active_tool_started_at = Some(now);
                if let Some(tool) = &data.tool_name {
                    self.root.active_tool = Some(tool.clone());
                    let target = data
                        .file_modification
                        .as_ref()
                        .map(|m| m.file_path.as_str())
                        .filter(|p| !p.is_empty())
                        .map(str::to_owned)
                        .or_else(|| extract_target(data));
                    // A target belongs to this call; a targetless call cannot retain the preceding call's path.
                    self.root.active_target = target.clone();
                    self.tool_args = sanitize_tool_input(
                        &data
                            .tool_input
                            .clone()
                            .map(serde_json::Value::Object)
                            .unwrap_or_default(),
                        300,
                    );
                    self.recent.push(ToolExecution {
                        tool_name: tool.clone(),
                        target: target.unwrap_or_default(),
                    });
                    if self.recent.len() > 3 {
                        self.recent.remove(0);
                    }
                    if tool == "AskUserQuestion" {
                        self.root.activity = ActivityState::WaitingQuestion;
                        if data.question.is_some() {
                            self.root.question = data.question.clone();
                        }
                        return self.summarize(id, true, now, clock, config);
                    }
                    return self.summarize(id, false, now, clock, config);
                }
            }
            "turn_start" => {
                self.idle = None;
                self.trailing = None;
                self.pending = None;
                self.assistant.clear();
                self.tool_args.clear();
                self.recent.clear();
                self.last_key.clear();
                self.in_flight = false;
                self.in_turn = true;
                self.floor = now.saturating_add(millis(config.turn_floor));
                self.schedule_silence(clock, config.silence_timeout, config);
                self.root.activity = ActivityState::Busy;
                self.clear_completion();
                self.root.last_error = None;
                self.clear_rate_limit();
                self.clear_tool();
                self.root.action = "Thinking".into();
                self.root.phase = Some(AgentPhase::Planning);
            }
            "thinking" => {
                self.idle = None;
                self.schedule_silence(clock, config.silence_timeout, config);
                self.root.activity = ActivityState::Busy;
                self.clear_completion();
                self.root.last_error = None;
                self.clear_rate_limit();
                if data.content.is_some() {
                    self.root.last_thinking_excerpt = data.content.clone();
                }
                if self.recent.is_empty()
                    && self.root.active_tool.is_none()
                    && (self.root.action.is_empty() || self.root.action == "Thinking")
                {
                    self.root.action = "Thinking".into();
                    self.root.phase = Some(AgentPhase::Planning);
                }
            }
            "turn_end" => return self.turn_end(id, data, now, clock, config),
            _ => {}
        }
        None
    }
    fn turn_end(
        &mut self,
        id: &str,
        data: &EventData,
        now: i64,
        clock: i64,
        config: &Config,
    ) -> Option<SummaryJob> {
        if let Some(content) = nonempty(data.content.as_deref()) {
            self.assistant = content.into();
        }
        let tool_use = if let Some(reason) = data.stop_reason.as_deref().filter(|v| !v.is_empty()) {
            matches!(reason, "tool_use" | "toolUse" | "tool_calls" | "tool_call")
        } else {
            self.in_flight || self.root.active_tool.is_some()
        };
        let terminal = matches!(data.stop_reason.as_deref(), Some("end_turn" | "stop"));
        let content = nonempty(data.content.as_deref()).is_some();
        if tool_use {
            self.idle = None;
            self.trailing = None;
            self.pending = None;
            self.schedule_silence(clock, config.silence_timeout, config);
            if self.root.activity != ActivityState::WaitingQuestion {
                self.root.activity = ActivityState::Busy;
                self.clear_completion();
            }
            return self.summarize(id, true, now, clock, config);
        }
        if matches!(
            self.root.activity,
            ActivityState::WaitingQuestion | ActivityState::Error
        ) {
            self.idle = None;
            self.trailing = None;
            self.pending = None;
            self.silence = None;
            return self.summarize(id, true, now, clock, config);
        }
        if self.root.activity == ActivityState::RateLimited && !terminal && !content {
            if data.rate_limit_reset_at.is_some() {
                self.root.rate_limit_reset_at = data.rate_limit_reset_at;
            }
            if data.retry_after_seconds.is_some() {
                self.root.retry_after_seconds = data.retry_after_seconds;
            }
            self.idle = None;
            self.trailing = None;
            self.pending = None;
            self.schedule_silence(
                clock,
                rate_limit_silence(
                    config,
                    self.root.rate_limit_reset_at,
                    self.root.retry_after_seconds,
                    now,
                ),
                config,
            );
            return self.summarize(id, true, now, clock, config);
        }
        if self.in_turn && now < self.floor && !content && !terminal && self.recent.is_empty() {
            self.root.activity = ActivityState::Busy;
            self.clear_completion();
            self.root.action = "Thinking".into();
            self.root.phase = Some(AgentPhase::Planning);
            self.idle = None;
            self.schedule_silence(clock, config.silence_timeout, config);
            return self.summarize(id, false, now, clock, config);
        }
        self.clear_tool();
        self.root.question = None;
        self.in_flight = false;
        self.in_turn = false;
        self.floor = 0;
        self.tool_args.clear();
        self.recent.clear();
        self.last_key.clear();
        self.pending = None;
        self.trailing = None;
        self.silence = None;
        self.turn_ended = now;
        if config.idle_grace.is_zero()
            || config.synchronous
            || self.root.activity != ActivityState::Busy
        {
            self.idle = None;
            if self.root.activity == ActivityState::Busy {
                self.root.has_unseen_completion = Some(true);
                self.root.completed_at = Some(now);
            }
            self.root.activity = ActivityState::Idle;
            self.clear_rate_limit();
            self.summarize(id, true, now, clock, config)
        } else {
            self.root.activity = ActivityState::Busy;
            self.clear_rate_limit();
            self.idle = Some((
                clock.saturating_add(millis(config.idle_grace)),
                self.root_generation,
            ));
            self.summarize(id, false, now, clock, config)
        }
    }
}

fn extract_target(data: &EventData) -> Option<String> {
    let input = data.tool_input.as_ref()?;
    [
        "file_path",
        "path",
        "notebook_path",
        "description",
        "command",
        "url",
        "query",
        "skill",
        "pattern",
        "prompt",
    ]
    .into_iter()
    .find_map(|key| nonempty(input.get(key).and_then(serde_json::Value::as_str)).map(str::to_owned))
}
