mod prompt;
mod provider;
mod text;

pub use prompt::{build_user_prompt, sanitize_tool_input};
pub use provider::{
    Config, DisableSource, ProviderInfo, ProviderInitError, ProviderSpec, Summarizer,
    sanitize_endpoint,
};
pub use text::{
    is_near_duplicate, jaccard_similarity, parse_output, tokenize, word_overlap_similarity,
};

use agent_common::types::{ActivityState, AgentPhase};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecution {
    pub tool_name: String,
    pub target: String,
}

#[derive(Clone, Debug, Default)]
pub struct SummaryInput {
    pub agent_id: String,
    pub current_action: String,
    pub last_prompt: String,
    pub last_assistant_response: String,
    pub recent_tools: Vec<ToolExecution>,
    pub last_thinking_excerpt: String,
    pub activity: ActivityState,
    pub active_tool: Option<String>,
    pub active_target: Option<String>,
    pub active_tool_args: Option<String>,
    pub active_tool_input: serde_json::Value,
    pub last_error: Option<String>,
    pub question: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryResult {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<AgentPhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_target: Option<String>,
    pub is_fallback: bool,
}

pub fn fallback(input: &SummaryInput) -> SummaryResult {
    use ActivityState as A;
    use AgentPhase as P;
    let active_target = input
        .active_target
        .clone()
        .filter(|value| !value.is_empty());
    let (action, phase) = if input.activity == A::RateLimited {
        (
            "Rate limit reached — backoff/retry in progress".into(),
            None,
        )
    } else if let Some(tool) = input.active_tool.as_ref().filter(|value| !value.is_empty()) {
        tool_summary(tool, active_target.as_deref().unwrap_or_default())
    } else {
        match input.activity {
            A::WaitingQuestion => (
                input
                    .question
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("Waiting for user input")
                    .into(),
                Some(P::Planning),
            ),
            A::Error => (
                input
                    .last_error
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("Encountered error")
                    .into(),
                Some(P::Debugging),
            ),
            A::Idle => (
                if !input.last_assistant_response.is_empty() && input.recent_tools.is_empty() {
                    "Responded to user"
                } else {
                    "Idle"
                }
                .into(),
                None,
            ),
            _ if !input.last_assistant_response.is_empty() && input.recent_tools.is_empty() => {
                ("Responded to user".into(), None)
            }
            _ if !input.current_action.is_empty() && input.current_action != "Thinking" => {
                (input.current_action.clone(), None)
            }
            _ if !input.recent_tools.is_empty() => {
                let last = input.recent_tools.last().expect("nonempty tools");
                tool_summary(&last.tool_name, &last.target)
            }
            _ if !input.last_thinking_excerpt.is_empty() || input.current_action == "Thinking" => {
                ("Thinking".into(), Some(P::Planning))
            }
            _ => ("Working".into(), None),
        }
    };
    SummaryResult {
        action,
        phase,
        active_target,
        is_fallback: true,
    }
}

fn tool_summary(tool: &str, target: &str) -> (String, Option<AgentPhase>) {
    use AgentPhase as P;
    let target_action = |verb: &str, fallback: &str| {
        if target.is_empty() {
            fallback.into()
        } else {
            format!("{verb} {target}")
        }
    };
    let (action, phase) = match tool.trim().to_lowercase().as_str() {
        "edit" | "notebookedit" | "notebook_edit" | "multiedit" | "multi_edit" => {
            (target_action("Editing", "Editing code"), P::Editing)
        }
        "write" | "writefile" | "write_file" => {
            (target_action("Writing", "Writing file"), P::Editing)
        }
        "read" | "readfile" | "read_file" => {
            (target_action("Reading", "Reading file"), P::Investigating)
        }
        "bash" | "sh" | "zsh" | "exec" | "exec_command" => {
            let lower = target.to_lowercase();
            if ["test", "jest", "pytest"]
                .iter()
                .any(|value| lower.contains(value))
            {
                ("Running tests".into(), P::Testing)
            } else {
                (
                    target_action("Running", "Running shell command"),
                    P::Editing,
                )
            }
        }
        "askuserquestion" | "ask_user_question" => ("Waiting for user input".into(), P::Planning),
        "websearch" | "web_search" | "webfetch" | "web_fetch" | "ast-grep" | "codegraph"
        | "grep" | "find" | "glob" => (
            target_action("Searching", "Investigating codebase"),
            P::Investigating,
        ),
        _ => (
            if target.is_empty() {
                format!("Using {tool}")
            } else {
                format!("Using {tool} on {target}")
            },
            P::Editing,
        ),
    };
    (action, Some(phase))
}
