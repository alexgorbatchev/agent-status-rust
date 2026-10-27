use super::SummaryInput;
use serde_json::Value;
use std::fmt::Write;

fn bounded(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push_str("...");
    }
    result
}

fn excluded(key: &str) -> bool {
    matches!(
        key.to_lowercase().as_str(),
        "content"
            | "file_text"
            | "replacement"
            | "patch"
            | "old_str"
            | "new_str"
            | "old_text"
            | "new_text"
            | "text"
            | "code"
            | "body"
            | "edits"
            | "raw"
            | "bytes"
            | "items"
            | "output"
            | "instructions"
            | "api_key"
            | "apikey"
            | "token"
            | "password"
            | "secret"
            | "auth"
            | "authorization"
            | "access_token"
            | "refresh_token"
            | "credential"
            | "credentials"
            | "private_key"
            | "key"
    )
}

fn scrub(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !excluded(key))
                .map(|(key, value)| (key.clone(), scrub(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(scrub).collect()),
        _ => value.clone(),
    }
}

pub fn sanitize_tool_input(input: &Value, max_len: usize) -> String {
    let Some(map) = input.as_object() else {
        return String::new();
    };
    let max_len = if max_len == 0 { 300 } else { max_len };
    let mut keys: Vec<_> = map.keys().filter(|key| !excluded(key)).collect();
    keys.sort();
    let parts = keys
        .into_iter()
        .filter_map(|key| {
            let value = &map[key];
            if value.is_null() {
                return None;
            }
            let value = scrub(value);
            let value = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
                .replace(['\r', '\n'], " ");
            let value = value.trim();
            (!value.is_empty()).then(|| format!("{key}: {}", bounded(value, 100)))
        })
        .collect::<Vec<_>>();
    bounded(&parts.join(", "), max_len)
}

pub fn build_user_prompt(input: &SummaryInput) -> String {
    let mut result = String::new();
    let has_thinking = !input.last_thinking_excerpt.trim().is_empty();
    let tool = input
        .active_tool
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if !input.last_prompt.trim().is_empty() {
        writeln!(
            result,
            "User Prompt: {}",
            bounded(input.last_prompt.trim(), 1000)
        )
        .expect("String writes succeed");
    }
    if has_thinking {
        writeln!(
            result,
            "Thinking: {}",
            bounded(input.last_thinking_excerpt.trim(), 1200)
        )
        .expect("String writes succeed");
    }
    if let Some(tool) = tool {
        write!(result, "Active Tool: {tool}").expect("String writes succeed");
        if let Some(target) = input
            .active_target
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            write!(result, " (Target: {target})").expect("String writes succeed");
        }
        result.push('\n');
        let args = input
            .active_tool_args
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| sanitize_tool_input(&input.active_tool_input, 300));
        if !args.is_empty() {
            writeln!(result, "Tool Arguments: {}", bounded(&args, 300))
                .expect("String writes succeed");
        }
    }
    if !has_thinking {
        if tool.is_none() {
            if !input.last_assistant_response.is_empty() {
                writeln!(
                    result,
                    "Assistant Response: {}",
                    bounded(input.last_assistant_response.trim(), 1000)
                )
                .expect("String writes succeed");
            }
            writeln!(
                result,
                "Current Activity State: {}",
                input.activity.as_str()
            )
            .expect("String writes succeed");
        }
        if !input.recent_tools.is_empty() {
            result.push_str("Recent Tool Calls:\n");
            for tool in input
                .recent_tools
                .iter()
                .skip(input.recent_tools.len().saturating_sub(3))
            {
                write!(result, "- {}", tool.tool_name).expect("String writes succeed");
                if !tool.target.is_empty() {
                    write!(result, " on {}", tool.target).expect("String writes succeed");
                }
                result.push('\n');
            }
        }
        if tool.is_none() {
            if let Some(question) = input
                .question
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                writeln!(result, "Question Asked: {question}").expect("String writes succeed");
            }
            if let Some(error) = input
                .last_error
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                writeln!(result, "Last Error: {}", bounded(error, 300))
                    .expect("String writes succeed");
            }
        }
    }
    result
}
