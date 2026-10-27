use std::{
    io,
    path::{Component, PathBuf},
};
use tracing::level_filters::LevelFilter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Human,
    Agent,
}
impl Mode {
    pub fn from_env() -> Self {
        if is_agent_mode(&std::env::var("AGENT").unwrap_or_default()) {
            Self::Agent
        } else {
            Self::Human
        }
    }
}
pub fn is_agent_mode(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}
pub fn parse_level(value: &str) -> io::Result<LevelFilter> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "info" => Ok(LevelFilter::INFO),
        "debug" => Ok(LevelFilter::DEBUG),
        "warn" | "warning" => Ok(LevelFilter::WARN),
        "error" => Ok(LevelFilter::ERROR),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid log level {value:?} (valid: debug, info, warn, error)"),
        )),
    }
}
#[derive(Clone, Debug)]
pub struct Options {
    pub service: String,
    pub level: String,
    pub mode: Mode,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            service: String::new(),
            level: "info".into(),
            mode: Mode::from_env(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileConfig {
    pub filename: Option<PathBuf>,
    pub dir: PathBuf,
    pub max_size_bytes: u64,
    /// Zero means unlimited, independently of the age limit.
    pub max_backups: usize,
    /// Zero means unlimited, independently of the backup count.
    pub max_age_days: u32,
    pub compress: bool,
    pub disabled: bool,
}
impl Default for FileConfig {
    fn default() -> Self {
        Self {
            filename: None,
            dir: default_log_dir(),
            max_size_bytes: 10 * 1024 * 1024,
            max_backups: 5,
            max_age_days: 14,
            compress: true,
            disabled: false,
        }
    }
}
pub fn default_log_dir() -> PathBuf {
    default_log_dir_with(|key| std::env::var(key).ok())
}
fn default_log_dir_with(get: impl Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(dir) = get("AGENT_LOG_DIR").filter(|v| !v.trim().is_empty()) {
        return PathBuf::from(dir.trim());
    }
    for key in ["XDG_STATE_HOME", "XDG_DATA_HOME"] {
        if let Some(dir) = get(key).filter(|v| !v.trim().is_empty()) {
            return PathBuf::from(dir.trim()).join("agent-status/logs");
        }
    }
    dirs::home_dir()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.join(".local/state/agent-status/logs"))
        .unwrap_or_else(|| std::env::temp_dir().join("agent-status/logs"))
}
fn disabled(value: Option<String>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}
impl FileConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }
    /// Inject a lookup to resolve configuration without mutating process environment.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let positive_size = get("AGENT_LOG_MAX_SIZE_MB")
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .and_then(|v| v.checked_mul(1024 * 1024));
        Self {
            filename: get("AGENT_LOG_FILE")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            dir: default_log_dir_with(&get),
            max_size_bytes: positive_size.unwrap_or(10 * 1024 * 1024),
            max_backups: get("AGENT_LOG_MAX_BACKUPS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            max_age_days: get("AGENT_LOG_MAX_AGE_DAYS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(14),
            compress: !disabled(get("AGENT_LOG_COMPRESS")),
            disabled: disabled(get("AGENT_LOG_TO_DISK")),
        }
    }
    pub fn path(&self, service: &str) -> io::Result<PathBuf> {
        if let Some(path) = &self.filename {
            return Ok(path.clone());
        }
        let name = if service.is_empty() {
            "agent-status"
        } else {
            service
        };
        let path = std::path::Path::new(name);
        if path.components().count() != 1
            || !matches!(path.components().next(), Some(Component::Normal(_)))
            || name.contains(['/', '\\'])
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log service must be a single file name",
            ));
        }
        Ok(self.dir.join(format!("{name}.log")))
    }
}
