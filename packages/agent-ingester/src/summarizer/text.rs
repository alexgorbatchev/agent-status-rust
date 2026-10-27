use super::SummaryResult;
use agent_common::types::AgentPhase;
use serde::Deserialize;
use std::collections::BTreeSet;

pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn similarity(a: &str, b: &str, overlap: bool) -> f64 {
    let a_tokens: BTreeSet<_> = tokenize(a).into_iter().collect();
    let b_tokens: BTreeSet<_> = tokenize(b).into_iter().collect();
    if a_tokens.is_empty() && b_tokens.is_empty() {
        return f64::from(a.trim().to_lowercase() == b.trim().to_lowercase());
    }
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }
    let shared = a_tokens.intersection(&b_tokens).count();
    let denominator = if overlap {
        a_tokens.len().min(b_tokens.len())
    } else {
        a_tokens.len() + b_tokens.len() - shared
    };
    shared as f64 / denominator as f64
}
pub fn jaccard_similarity(a: &str, b: &str) -> f64 {
    similarity(a, b, false)
}
pub fn word_overlap_similarity(a: &str, b: &str) -> f64 {
    similarity(a, b, true)
}
pub fn is_near_duplicate(a: &str, b: &str, threshold: f64) -> bool {
    let a = a.trim();
    let b = b.trim();
    let threshold = if threshold <= 0.0 { 0.6 } else { threshold };
    if a.is_empty() || b.is_empty() {
        return a == b;
    }
    a.to_lowercase() == b.to_lowercase()
        || jaccard_similarity(a, b) >= threshold
        || (tokenize(a).len().min(tokenize(b).len()) >= 3
            && word_overlap_similarity(a, b) >= threshold)
}

pub fn parse_output(raw: &str) -> Result<SummaryResult, serde_json::Error> {
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Output {
        action: String,
        phase: String,
        active_target: String,
    }
    let mut fenced = String::new();
    let mut inside = false;
    if raw.contains("```") {
        for line in raw.lines() {
            if line.trim_start().starts_with("```") {
                inside = !inside;
            } else if inside {
                fenced.push_str(line);
                fenced.push('\n');
            }
        }
    }
    let content = if fenced.trim().is_empty() {
        raw.trim()
    } else {
        fenced.trim()
    };
    let object = content
        .find('{')
        .zip(content.rfind('}'))
        .filter(|(start, end)| start < end)
        .map(|(start, end)| &content[start..=end])
        .unwrap_or("");
    let result: Output = serde_json::from_str(object)?;
    if result.action.trim().is_empty() {
        return Err(<serde_json::Error as serde::de::Error>::custom(
            "summary action is empty",
        ));
    }
    let phase = match result.phase.trim().to_lowercase().as_str() {
        "planning" | "plan" => Some(AgentPhase::Planning),
        "editing" | "edit" | "writing" | "write" => Some(AgentPhase::Editing),
        "testing" | "test" => Some(AgentPhase::Testing),
        "debugging" | "debug" | "fix" | "fixing" => Some(AgentPhase::Debugging),
        "investigating" | "investigate" | "reading" | "search" | "searching" => {
            Some(AgentPhase::Investigating)
        }
        _ => None,
    };
    Ok(SummaryResult {
        action: result.action.trim().into(),
        phase,
        active_target: (!result.active_target.trim().is_empty())
            .then(|| result.active_target.trim().into()),
        is_fallback: false,
    })
}
