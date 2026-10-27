//! Environment-backed defaults resolved only after offline command handling.
use std::{path::PathBuf, time::Duration};

pub struct Paths {
    home: Option<PathBuf>,
}
impl Paths {
    pub fn new(home: Option<PathBuf>) -> Self {
        Self { home }
    }
    pub fn capture() -> Self {
        Self::new(dirs::home_dir())
    }
    pub fn database(&self, lookup: &impl Fn(&str) -> Option<String>) -> PathBuf {
        nonempty(lookup, "XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.clone().unwrap_or_default().join(".local/share"))
            .join("agent-status/agent-status.db")
    }
    pub fn notify_socket(&self, lookup: &impl Fn(&str) -> Option<String>) -> PathBuf {
        nonempty(lookup, "XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("agent-status/notify.sock")
    }
    pub fn herdr_socket(&self, lookup: &impl Fn(&str) -> Option<String>) -> PathBuf {
        if let Some(path) = nonempty(lookup, "HERDR_SOCKET_PATH") {
            return path.into();
        }
        nonempty(lookup, "XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.clone().unwrap_or_default().join(".config"))
            .join("herdr/herdr.sock")
    }
}
pub fn nonempty(lookup: &impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    lookup(name).filter(|value| !value.is_empty())
}
pub fn duration(value: &str) -> Result<Duration, String> {
    if matches!(value, "0" | "+0" | "-0") {
        return Ok(Duration::ZERO);
    }
    let nanos = value
        .parse::<go_duration::GoDuration>()
        .map_err(|_| "expected a duration such as 5s, 250ms, or 1h".to_string())?
        .nanoseconds();
    u64::try_from(nanos)
        .map(Duration::from_nanos)
        .map_err(|_| "duration must be nonnegative".into())
}
