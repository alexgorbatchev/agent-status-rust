use agent_common::types::EnvironmentInfo;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Agent-status paths, resolved independently of the reusable watcher defaults.
#[derive(Clone, Debug)]
pub struct Paths {
    config_home: Option<PathBuf>,
    cache_home: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl Paths {
    pub fn new(
        config_home: Option<PathBuf>,
        cache_home: Option<PathBuf>,
        home: Option<PathBuf>,
    ) -> Self {
        Self {
            config_home,
            cache_home,
            home,
        }
    }

    pub fn capture() -> Self {
        let variable = |name| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self::new(
            variable("XDG_CONFIG_HOME"),
            variable("XDG_CACHE_HOME"),
            dirs::home_dir(),
        )
    }

    pub fn telemetry_config(&self) -> Option<PathBuf> {
        self.config_home
            .clone()
            .or_else(|| self.home.as_ref().map(|h| h.join(".config")))
            .map(|p| p.join("ai-registry/telemetry.json"))
    }

    pub fn observer_cache(&self, name: impl AsRef<Path>) -> PathBuf {
        self.cache_home
            .clone()
            .or_else(|| self.home.as_ref().map(|h| h.join(".cache")))
            .unwrap_or_else(std::env::temp_dir)
            .join("agent-status")
            .join(name)
    }
}

/// Identity inputs are explicit so callers can resolve metadata without changing process globals.
#[derive(Clone, Debug)]
pub struct Identity {
    pub machine_id: String,
    pub hostname: String,
    pub uid: String,
    pub username: String,
}

impl Identity {
    pub fn capture() -> Self {
        let uid = uzers::get_current_uid();
        let user = uzers::get_user_by_uid(uid);
        Self {
            machine_id: platform_machine_id(),
            hostname: hostname::get()
                .map(|h| h.to_string_lossy().into_owned())
                .unwrap_or_default(),
            uid: user
                .as_ref()
                .map(|u| u.uid().to_string())
                .unwrap_or_else(|| {
                    std::env::var("UID")
                        .ok()
                        .filter(|u| !u.is_empty())
                        .unwrap_or_else(|| "0".into())
                }),
            username: user
                .map(|u| u.name().to_string_lossy().into_owned())
                .unwrap_or_else(|| std::env::var("USER").unwrap_or_default()),
        }
    }

    pub fn environment_id(&self) -> String {
        let machine = if !self.machine_id.is_empty() {
            &self.machine_id
        } else if !self.hostname.is_empty() {
            &self.hostname
        } else {
            "fallback-machine"
        };
        format!("{:x}", Sha256::digest(format!("{machine}:{}", self.uid)))[..16].to_owned()
    }

    pub fn environment(&self, label: impl Into<String>, now_ms: i64) -> EnvironmentInfo {
        EnvironmentInfo {
            id: self.environment_id(),
            label: label.into(),
            hostname: if self.hostname.is_empty() {
                "unknown".into()
            } else {
                self.hostname.clone()
            },
            platform: if cfg!(target_os = "macos") {
                "darwin"
            } else {
                "linux"
            }
            .into(),
            user: if self.username.is_empty() {
                "unknown".into()
            } else {
                self.username.clone()
            },
            is_stale: false,
            last_heartbeat_at: now_ms,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct TelemetryConfig {
    environment: String,
    env: String,
    label: String,
}

pub fn resolve_label(flag: &str, ai_env: &str, config: Option<&Path>, hostname: &str) -> String {
    for value in [flag.trim(), ai_env.trim()] {
        if !value.is_empty() {
            return value.to_owned();
        }
    }
    if let Some(config) = config
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<TelemetryConfig>(&bytes).ok())
    {
        for value in [config.environment, config.env, config.label] {
            if !value.is_empty() {
                return value;
            }
        }
    }
    if hostname.is_empty() {
        "unknown-host".into()
    } else {
        hostname.into()
    }
}

pub fn read_machine_id(paths: impl IntoIterator<Item = impl AsRef<Path>>) -> String {
    paths
        .into_iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_owned())
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn platform_machine_id() -> String {
    read_machine_id(["/etc/machine-id", "/var/lib/dbus/machine-id"])
}

#[cfg(target_os = "macos")]
fn platform_machine_id() -> String {
    use std::process::Command;
    if let Ok(output) = Command::new("sysctl").args(["-n", "kern.uuid"]).output() {
        let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if output.status.success() && !id.is_empty() {
            return id;
        }
    }
    Command::new("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find(|line| line.contains("IOPlatformUUID"))
                .and_then(|line| {
                    line.split_once('=')
                        .map(|(_, value)| value.trim().trim_matches('"').to_owned())
                })
        })
        .unwrap_or_default()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_machine_id() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default()
}
