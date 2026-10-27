//! Context capacity estimates; explicit 1m suffixes represent caller configuration.
pub const DEFAULT_CONTEXT_WINDOW: i64 = 128_000;

const CAPACITIES: &[(&str, i64)] = &[
    ("claude-3-7-sonnet", 200000),
    ("claude-3-7-sonnet-20250219", 200000),
    ("claude-3.7-sonnet", 200000),
    ("claude-3-7-sonnet-1m", 1000000),
    ("claude-3-7-sonnet:1m", 1000000),
    ("claude-3.7-sonnet-1m", 1000000),
    ("claude-3.7-sonnet:1m", 1000000),
    ("claude-3-5-sonnet", 200000),
    ("claude-3-5-sonnet-20241022", 200000),
    ("claude-3-5-sonnet-20240620", 200000),
    ("claude-3.5-sonnet", 200000),
    ("claude-3-5-haiku", 200000),
    ("claude-3-5-haiku-20241022", 200000),
    ("claude-3.5-haiku", 200000),
    ("claude-3-opus", 200000),
    ("claude-3-opus-20240229", 200000),
    ("claude-3.0-opus", 200000),
    ("claude-3-sonnet", 200000),
    ("claude-3-sonnet-20240229", 200000),
    ("claude-3-haiku", 200000),
    ("claude-3-haiku-20240307", 200000),
    ("claude-haiku-4-5", 200000),
    ("claude-haiku-4-5-20251001", 200000),
    ("claude-sonnet-4-5", 200000),
    ("claude-opus-5", 1000000),
    ("claude-sonnet-5", 1000000),
    ("claude-fable-5", 1000000),
    ("claude-fable-5-1", 1000000),
    ("gemini-2.5-pro", 1048576),
    ("gemini-2.5-pro-preview", 1048576),
    ("gemini-1.5-pro", 2000000),
    ("gemini-1.5-pro-latest", 2000000),
    ("gemini-1.5-pro-001", 2000000),
    ("gemini-1.5-pro-002", 2000000),
    ("gemini-1.5-flash", 1000000),
    ("gemini-1.5-flash-latest", 1000000),
    ("gemini-1.5-flash-8b", 1000000),
    ("gemini-2.0-flash", 1000000),
    ("gemini-2.0-flash-exp", 1000000),
    ("gemini-2.0-pro-exp", 1000000),
    ("gemini-2.5-flash", 1000000),
    ("gemini-3.7-flash", 1000000),
    ("gpt-4o", 128000),
    ("gpt-4o-2024-05-13", 128000),
    ("gpt-4o-2024-08-06", 128000),
    ("gpt-4o-2024-11-20", 128000),
    ("gpt-4o-mini", 128000),
    ("gpt-4o-mini-2024-07-18", 128000),
    ("gpt-4-turbo", 128000),
    ("gpt-4-turbo-2024-04-09", 128000),
    ("gpt-4-turbo-preview", 128000),
    ("gpt-4-0125-preview", 128000),
    ("gpt-4-1106-preview", 128000),
    ("gpt-4", 8192),
    ("gpt-4-0613", 8192),
    ("gpt-4-32k", 32768),
    ("gpt-3.5-turbo", 16385),
    ("gpt-3.5-turbo-0125", 16385),
    ("gpt-3.5-turbo-16k", 16385),
    ("o1", 200000),
    ("o1-2024-12-17", 200000),
    ("o1-preview", 128000),
    ("o1-mini", 128000),
    ("o3", 200000),
    ("o3-mini", 200000),
    ("o3-mini-2025-01-31", 200000),
    ("gpt-5", 400000),
    ("gpt-5.4", 1050000),
    ("gpt-5.5", 1050000),
];

pub fn resolve_context_window(model_id: &str) -> i64 {
    let normalized = model_id.trim().to_ascii_lowercase();
    let name = normalized.rsplit('/').next().unwrap_or("");
    if name.contains("-1m") || name.contains(":1m") {
        return 1_000_000;
    }
    // Longest matching catalog entry wins before family estimates, including dated aliases.
    if let Some((_, capacity)) = CAPACITIES
        .iter()
        .filter(|(prefix, _)| name.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
    {
        return *capacity;
    }
    if name.starts_with("gemini") {
        return 1_000_000;
    }
    if name.starts_with("claude") {
        return 200_000;
    }
    if name.starts_with("gpt-4o") || name.starts_with("gpt-4-turbo") {
        return 128_000;
    }
    if name.starts_with("o1") || name.starts_with("o3") {
        return 200_000;
    }
    if name.starts_with("gpt-4") {
        return 8_192;
    }
    if name.starts_with("gpt-3.5") {
        return 16_385;
    }
    DEFAULT_CONTEXT_WINDOW
}

pub fn calculate_context_fullness(used_tokens: i64, context_window: i64) -> f64 {
    if context_window <= 0 {
        0.0
    } else {
        used_tokens as f64 / context_window as f64
    }
}
