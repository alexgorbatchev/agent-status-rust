use agent_common::types::AgentState;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct FieldMutation {
    pub field: String,
    pub from: String,
    pub to: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateDiff {
    pub step_index: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub line_number: usize,
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_synthetic: bool,
    pub mutations: Vec<FieldMutation>,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub input_delta: i64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub output_delta: i64,
    #[serde(skip_serializing_if = "is_zero_f64")]
    pub cost_delta: f64,
}
fn is_zero(value: &usize) -> bool {
    *value == 0
}
fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}
fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}
fn optional(value: &Option<String>) -> String {
    value
        .as_ref()
        .map_or_else(|| "nil".into(), |v| format!("{v:?}"))
}

pub fn compute(
    step_index: usize,
    event: &crate::transcript::ReplayEvent,
    previous: Option<&AgentState>,
    current: Option<&AgentState>,
) -> StateDiff {
    let mut diff = StateDiff {
        step_index,
        line_number: event.source_line,
        event_type: event.event.event_type.clone(),
        subagent_id: event.subagent_id.clone(),
        is_synthetic: event.is_synthetic,
        mutations: Vec::new(),
        input_delta: 0,
        output_delta: 0,
        cost_delta: 0.0,
    };
    let Some(current) = current else { return diff };
    let next = &current.activity;
    let mut add = |field: &str, from: String, to: String| {
        if from != to {
            diff.mutations.push(FieldMutation {
                field: field.into(),
                from,
                to,
            });
        }
    };
    if let Some(previous) = previous {
        let old = &previous.activity;
        add(
            "Lifecycle",
            old.lifecycle.as_str().into(),
            next.lifecycle.as_str().into(),
        );
        add(
            "Activity",
            old.activity.as_str().into(),
            next.activity.as_str().into(),
        );
        for (name, from, to) in [
            ("Title", &old.title, &next.title),
            ("Action", &old.action, &next.action),
        ] {
            add(name, format!("{from:?}"), format!("{to:?}"));
        }
        for (name, from, to) in [
            ("ActiveTool", &old.active_tool, &next.active_tool),
            ("ActiveTarget", &old.active_target, &next.active_target),
            ("Question", &old.question, &next.question),
            ("LastError", &old.last_error, &next.last_error),
        ] {
            add(name, optional(from), optional(to));
        }
        add(
            "SubagentCount",
            old.subagent_count.to_string(),
            next.subagent_count.to_string(),
        );
        diff.input_delta = next.usage.input.saturating_sub(old.usage.input);
        diff.output_delta = next.usage.output.saturating_sub(old.usage.output);
        diff.cost_delta = (next.cost.as_ref().map_or(0.0, |c| c.total)
            - old.cost.as_ref().map_or(0.0, |c| c.total))
        .max(0.0);
    } else {
        add("Lifecycle", "none".into(), next.lifecycle.as_str().into());
        add("Activity", "none".into(), next.activity.as_str().into());
        if !next.title.is_empty() {
            add("Title", "\"\"".into(), format!("{:?}", next.title));
        }
        diff.input_delta = next.usage.input;
        diff.output_delta = next.usage.output;
        diff.cost_delta = next.cost.as_ref().map_or(0.0, |c| c.total);
    }
    diff
}
