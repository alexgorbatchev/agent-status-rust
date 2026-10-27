//! Native process/session inventory and deterministic, identity-aware pairing.
use crate::{Result, check_cancel};
use agent_common::types::HarnessType;
use agent_watcher::scanner::{self, ProcessMeta, ProcessSnapshot};
use path_clean::PathClean;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PairingStatus {
    Paired,
    OrphanProcess,
    OrphanSession,
}
impl PairingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Paired => "PAIRED",
            Self::OrphanProcess => "ORPHAN_PROCESS",
            Self::OrphanSession => "ORPHAN_SESSION",
        }
    }
    fn priority(self) -> u8 {
        match self {
            Self::Paired => 0,
            Self::OrphanProcess => 1,
            Self::OrphanSession => 2,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: i32,
    pub created_at: i64,
    pub cwd: PathBuf,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cmdline: String,
    #[serde(skip_serializing_if = "zero")]
    pub memory_mb: f64,
    #[serde(skip_serializing_if = "zero")]
    pub cpu_percent: f64,
}
fn zero(value: &f64) -> bool {
    *value == 0.0
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionInfo {
    pub harness: HarnessType,
    pub session_id: String,
    pub cwd: PathBuf,
    pub file_path: PathBuf,
    pub header_time: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub harness_version: String,
    #[serde(skip_serializing_if = "pid_zero")]
    pub pid: i32,
}
/// A file that could not contribute a session, without discarding valid peers.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDiagnostic {
    pub path: PathBuf,
    pub error: String,
}
#[derive(Clone, Debug, Default)]
pub struct SessionInventory {
    pub sessions: Vec<SessionInfo>,
    pub diagnostics: Vec<SessionDiagnostic>,
}
impl SessionInventory {
    fn read(&mut self, path: PathBuf, read: impl FnOnce(&Path) -> Result<SessionInfo>) {
        match read(&path) {
            Ok(session) => self.sessions.push(session),
            Err(error) => self.diagnostics.push(SessionDiagnostic {
                path,
                error: error.to_string(),
            }),
        }
    }
}
fn pid_zero(value: &i32) -> bool {
    *value == 0
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRecord {
    pub harness: HarnessType,
    pub pid: i32,
    pub created_at: i64,
    pub cwd: PathBuf,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cmdline: String,
    #[serde(skip_serializing_if = "zero")]
    pub memory_mb: f64,
    #[serde(skip_serializing_if = "zero")]
    pub cpu_percent: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(skip_serializing_if = "empty_path")]
    pub session_file: PathBuf,
    #[serde(skip_serializing_if = "time_zero")]
    pub header_time: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_ms: Option<i64>,
    pub status: PairingStatus,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status_details: String,
}
fn empty_path(path: &Path) -> bool {
    path.as_os_str().is_empty()
}
fn time_zero(value: &i64) -> bool {
    *value == 0
}
#[derive(Clone, Debug, Default)]
pub struct AuditFilter {
    pub harness: Option<HarnessType>,
    pub cwd: Option<PathBuf>,
}
pub fn match_cwd(target: &Path, filter: Option<&Path>) -> bool {
    filter.is_none_or(|filter| target.clean().starts_with(filter.clean()))
}
fn status_details(time: i64, now: i64) -> String {
    let age = now.saturating_sub(time);
    if time > 0 && age >= 3_600_000 {
        format!("IDLE {}h", age / 3_600_000)
    } else {
        "ACTIVE".into()
    }
}
fn record(
    harness: &HarnessType,
    process: Option<&ProcessInfo>,
    session: Option<&SessionInfo>,
    now: i64,
) -> AuditRecord {
    AuditRecord {
        harness: harness.clone(),
        pid: process.map_or_else(|| session.map_or(0, |s| s.pid), |p| p.pid),
        created_at: process.map_or_else(|| session.map_or(0, |s| s.header_time), |p| p.created_at),
        cwd: process.map_or_else(
            || session.map_or_else(PathBuf::new, |s| s.cwd.clone()),
            |p| p.cwd.clone(),
        ),
        cmdline: process.map_or_else(String::new, |p| p.cmdline.clone()),
        memory_mb: process.map_or(0.0, |p| p.memory_mb),
        cpu_percent: process.map_or(0.0, |p| p.cpu_percent),
        session_id: session.map_or_else(String::new, |s| s.session_id.clone()),
        session_file: session.map_or_else(PathBuf::new, |s| s.file_path.clone()),
        header_time: session.map_or(0, |s| s.header_time),
        delta_ms: process
            .zip(session)
            .map(|(p, s)| s.header_time.saturating_sub(p.created_at)),
        status: if process.is_some() && session.is_some() {
            PairingStatus::Paired
        } else if process.is_some() {
            PairingStatus::OrphanProcess
        } else {
            PairingStatus::OrphanSession
        },
        status_details: process
            .zip(session)
            .map_or_else(String::new, |(_, s)| status_details(s.header_time, now)),
    }
}
pub fn audit(
    groups: &[(HarnessType, Vec<ProcessInfo>)],
    sessions: &[SessionInfo],
    filter: &AuditFilter,
    now: i64,
) -> Vec<AuditRecord> {
    let mut records = Vec::new();
    let mut harnesses = Vec::new();
    for harness in groups
        .iter()
        .map(|(h, _)| h)
        .chain(sessions.iter().map(|s| &s.harness))
    {
        if filter
            .harness
            .as_ref()
            .is_none_or(|selected| selected == harness)
            && !harnesses.contains(&harness)
        {
            harnesses.push(harness);
        }
    }
    for harness in harnesses {
        let processes = groups
            .iter()
            .filter(|(found, _)| found == harness)
            .flat_map(|(_, processes)| processes)
            .filter(|p| match_cwd(&p.cwd, filter.cwd.as_deref()))
            .collect::<Vec<_>>();
        let mut sessions = sessions
            .iter()
            .filter(|s| &s.harness == harness && match_cwd(&s.cwd, filter.cwd.as_deref()))
            .collect::<Vec<_>>();
        let mut used = HashSet::new();
        if harness == &HarnessType::ClaudeCode {
            sessions.sort_by_key(|s| std::cmp::Reverse(s.header_time));
            for session in sessions {
                let process = processes.iter().copied().find(|p| {
                    p.pid > 0
                        && p.pid == session.pid
                        && !used.contains(&p.pid)
                        && (p.created_at <= 0
                            || session.header_time <= 0
                            || p.created_at.abs_diff(session.header_time) < 60_000)
                });
                if let Some(p) = process {
                    used.insert(p.pid);
                }
                records.push(record(harness, process, Some(session), now));
            }
            records.extend(
                processes
                    .into_iter()
                    .filter(|p| !used.contains(&p.pid))
                    .map(|p| record(harness, Some(p), None, now)),
            );
        } else {
            let mut used_sessions = HashSet::new();
            for p in processes {
                let best = sessions
                    .iter()
                    .copied()
                    .filter(|s| {
                        !used_sessions.contains(&(s.file_path.clone(), s.session_id.clone()))
                            && s.cwd.clean() == p.cwd.clean()
                            && i128::from(s.header_time) - i128::from(p.created_at) >= -5000
                    })
                    .min_by_key(|s| s.header_time.abs_diff(p.created_at));
                if let Some(s) = best {
                    used_sessions.insert((s.file_path.clone(), s.session_id.clone()));
                }
                records.push(record(harness, Some(p), best, now));
            }
            records.extend(
                sessions
                    .into_iter()
                    .filter(|s| {
                        !used_sessions.contains(&(s.file_path.clone(), s.session_id.clone()))
                    })
                    .map(|s| record(harness, None, Some(s), now)),
            );
        }
    }
    records.sort_by_key(|r| (r.status.priority(), std::cmp::Reverse(r.created_at)));
    records
}

/// One native capture supplies both scanner identity checks and audit metadata.
pub struct LiveProcesses {
    pub snapshot: ProcessSnapshot,
    pub groups: Vec<(HarnessType, Vec<ProcessInfo>)>,
}
pub fn capture(cancel: &CancellationToken, now: i64) -> Result<LiveProcesses> {
    use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};
    check_cancel(cancel)?;
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cpu()
            .with_memory()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always),
    );
    let mut metadata = Vec::new();
    let mut enrichment = HashMap::new();
    for (pid, p) in system.processes() {
        check_cancel(cancel)?;
        let Ok(pid) = i32::try_from(pid.as_u32()) else {
            continue;
        };
        let cwd = p.cwd().map_or_else(PathBuf::new, |v| v.clean());
        let created = i64::try_from(p.start_time())
            .unwrap_or(i64::MAX)
            .saturating_mul(1000);
        let cmdline = p
            .cmd()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        enrichment.insert(
            pid,
            ProcessInfo {
                pid,
                created_at: created,
                cwd: cwd.clone(),
                cmdline: cmdline.join(" "),
                memory_mb: p.memory() as f64 / 1_048_576.0,
                cpu_percent: if created > 0 && now > created {
                    p.accumulated_cpu_time() as f64 / now.saturating_sub(created) as f64 * 100.0
                } else {
                    0.0
                },
            },
        );
        metadata.push(ProcessMeta {
            pid,
            name: p.name().to_string_lossy().into_owned(),
            create_time: created,
            alive: !matches!(p.status(), ProcessStatus::Zombie | ProcessStatus::Dead),
            cwd,
            cmdline,
        });
    }
    let snapshot = ProcessSnapshot::from_processes(metadata);
    let mut groups = Vec::new();
    for (harness, processes) in [
        (HarnessType::Pi, snapshot.pi_processes()),
        (HarnessType::Codex, snapshot.codex_processes()),
        (HarnessType::Opencode, snapshot.opencode_processes()),
    ] {
        groups.push((
            harness,
            processes
                .iter()
                .filter_map(|p| enrichment.get(&p.pid).cloned())
                .collect(),
        ));
    }
    let basename = |text: &str| {
        text.rsplit(['/', '\\'])
            .next()
            .unwrap_or(text)
            .to_ascii_lowercase()
    };
    groups.push((
        HarnessType::ClaudeCode,
        snapshot
            .processes()
            .filter(|p| {
                p.alive
                    && !p.cwd.as_os_str().is_empty()
                    && (matches!(basename(&p.name).as_str(), "claude" | "claude.exe")
                        || p.cmdline.iter().any(|arg| basename(arg) == "claude"))
            })
            .filter_map(|p| enrichment.get(&p.pid).cloned())
            .collect(),
    ));
    check_cancel(cancel)?;
    Ok(LiveProcesses { snapshot, groups })
}

#[derive(Clone, Debug)]
pub struct SessionPaths {
    pub pi: PathBuf,
    pub claude: Vec<PathBuf>,
    pub codex: PathBuf,
    pub opencode: PathBuf,
}
impl Default for SessionPaths {
    fn default() -> Self {
        Self {
            pi: scanner::default_pi_sessions_dir(),
            claude: scanner::default_claude_session_dirs(),
            codex: scanner::default_codex_sessions_dir(),
            opencode: scanner::default_opencode_db_path(),
        }
    }
}
fn entries(path: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let read = match fs::read_dir(path) {
        Ok(read) => read,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut entries = read.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    Ok(entries)
}
fn jsonl_files(
    cancel: &CancellationToken,
    root: &Path,
    recursive: bool,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in entries(root)? {
        check_cancel(cancel)?;
        let kind = entry.file_type()?;
        if recursive && kind.is_dir() {
            jsonl_files(cancel, &entry.path(), true, out)?;
        } else if kind.is_file() && entry.path().extension().is_some_and(|v| v == "jsonl") {
            out.push(entry.path());
        }
    }
    Ok(())
}
pub fn sessions(
    cancel: &CancellationToken,
    paths: &SessionPaths,
    harness: Option<&HarnessType>,
) -> Result<SessionInventory> {
    check_cancel(cancel)?;
    let mut result = SessionInventory::default();
    if harness.is_none_or(|h| h == &HarnessType::Pi) {
        let mut files = Vec::new();
        for dir in entries(&paths.pi)? {
            check_cancel(cancel)?;
            if dir.file_type()?.is_dir() {
                jsonl_files(cancel, &dir.path(), false, &mut files)?;
            }
        }
        for path in files {
            check_cancel(cancel)?;
            result.read(path, |path| {
                let header = scanner::read_pi_session_header(path)?;
                let timestamp = pi_timestamp(&header.timestamp).filter(|t| *t != 0);
                let timestamp = match timestamp {
                    Some(time) => time,
                    None => {
                        let time = fs::metadata(path)?.modified()?;
                        let milliseconds = match time.duration_since(std::time::UNIX_EPOCH) {
                            Ok(duration) => i128::try_from(duration.as_millis()).ok(),
                            Err(error) => i128::try_from(error.duration().as_millis())
                                .ok()
                                .map(|v| -v),
                        };
                        milliseconds
                            .and_then(|v| i64::try_from(v).ok())
                            .ok_or_else(|| {
                                crate::Error::InvalidInput(
                                    "session modification time is out of range".into(),
                                )
                            })?
                    }
                };
                Ok(SessionInfo {
                    harness: HarnessType::Pi,
                    session_id: header.id,
                    cwd: header.cwd.into(),
                    file_path: path.to_owned(),
                    header_time: timestamp,
                    ..Default::default()
                })
            });
        }
    }
    if harness.is_none_or(|h| h == &HarnessType::ClaudeCode) {
        let mut used = HashSet::new();
        for root in &paths.claude {
            for entry in entries(root)? {
                check_cancel(cancel)?;
                let path = entry.path();
                if !entry.file_type()?.is_file()
                    || path.extension().is_none_or(|s| s != "json")
                    || !used.insert(path.clone())
                {
                    continue;
                }
                result.read(path, |path| {
                    let raw: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
                    let descriptor: scanner::SessionDescriptor =
                        serde_json::from_value(raw.clone())?;
                    Ok(SessionInfo {
                        harness: HarnessType::ClaudeCode,
                        session_id: descriptor.session_id,
                        cwd: descriptor.cwd,
                        file_path: path.to_owned(),
                        header_time: descriptor.started_at,
                        pid: descriptor.pid,
                        harness_version: raw["version"]
                            .as_str()
                            .unwrap_or(&descriptor.harness_version)
                            .into(),
                    })
                });
            }
        }
    }
    if harness.is_none_or(|h| h == &HarnessType::Codex) {
        let mut files = Vec::new();
        jsonl_files(cancel, &paths.codex, true, &mut files)?;
        for path in files {
            check_cancel(cancel)?;
            result.read(path, |path| {
                let header = scanner::read_codex_session_header(path)?;
                Ok(SessionInfo {
                    harness: HarnessType::Codex,
                    session_id: header.session_id,
                    cwd: header.cwd,
                    file_path: path.to_owned(),
                    header_time: header.timestamp,
                    harness_version: header.cli_version,
                    ..Default::default()
                })
            });
        }
    }
    if harness.is_none_or(|h| h == &HarnessType::Opencode) && paths.opencode.try_exists()? {
        let db = agent_parser::OpencodeDb::open(&paths.opencode)?;
        for session in crate::opencode::query(cancel, &db, |db| db.get_sessions())? {
            check_cancel(cancel)?;
            if session.time_archived.is_some_and(|t| t > 0) {
                continue;
            }
            result.sessions.push(SessionInfo {
                harness: HarnessType::Opencode,
                session_id: session.id,
                cwd: session.directory.into(),
                file_path: paths.opencode.clone(),
                header_time: session.time_created,
                ..Default::default()
            });
        }
    }
    check_cancel(cancel)?;
    Ok(result)
}

fn pi_timestamp(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| {
            value
                .as_f64()
                .filter(|v| *v >= i64::MIN as f64 && *v < -(i64::MIN as f64))
                .map(|v| v.trunc() as i64)
        })
        .or_else(|| {
            value
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.timestamp_millis())
        })
}
