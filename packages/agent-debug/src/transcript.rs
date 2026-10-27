use crate::{Error, Result, check_cancel};
use agent_common::types::HarnessType;
use agent_parser::{ClaudeCodeParser, CodexParser, OpencodeDb, ParsedEvent};
use path_clean::PathClean;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

const MAX_LINE: u64 = 20 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ReplayEvent {
    pub event: ParsedEvent,
    pub source_line: usize,
    pub subagent_id: Option<String>,
    pub is_synthetic: bool,
}

#[derive(Clone, Debug)]
pub struct LoadedSession {
    pub harness: HarnessType,
    pub session_id: String,
    pub cwd: PathBuf,
    pub harness_version: String,
    pub events: Vec<ReplayEvent>,
    pub subagents: Vec<String>,
    /// Parsing failures retain their source location rather than disappearing from validation.
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LoadOptions {
    pub harness: Option<HarnessType>,
    pub session_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub strict_lifecycle: bool,
}

fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> Result<usize> {
    line.clear();
    let read = reader.take(MAX_LINE + 1).read_until(b'\n', line)?;
    if line.len() as u64 > MAX_LINE {
        return Err(Error::InvalidInput("transcript line exceeds 20 MiB".into()));
    }
    Ok(read)
}

pub fn detect_harness(path: &Path) -> Result<HarnessType> {
    if path.is_dir() {
        for entry in entries(path)? {
            if entry.file_type()?.is_file()
                && matches!(
                    entry.path().extension().and_then(|v| v.to_str()),
                    Some("jsonl" | "db")
                )
            {
                return detect_harness(&entry.path());
            }
        }
        return Err(Error::UnknownHarness(format!(
            "{} contains no recognized session files",
            path.display()
        )));
    }
    let mut reader = BufReader::new(File::open(path)?);
    let mut header = [0; 16];
    let count = reader.read(&mut header)?;
    if count == 16 && &header == b"SQLite format 3\0"
        || path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("db"))
    {
        return Ok(HarnessType::Opencode);
    }
    let mut reader = BufReader::new(File::open(path)?);
    let mut samples = Vec::new();
    let mut line = Vec::new();
    while samples.len() < 20 && read_line(&mut reader, &mut line)? > 0 {
        if !line.iter().all(u8::is_ascii_whitespace) {
            samples.push(line.clone());
        }
    }
    if samples.is_empty() {
        return Err(Error::UnknownHarness("file is empty".into()));
    }
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
    {
        return Ok(HarnessType::Codex);
    }
    for sample in &samples {
        let Ok(value) = serde_json::from_slice::<Value>(sample) else {
            continue;
        };
        let kind = value
            .get("type")
            .or_else(|| value.get("event_type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind == "session_meta" {
            return Ok(HarnessType::Codex);
        }
        if kind == "session"
            && ["cwd", "version", "parentId"]
                .iter()
                .any(|k| value.get(k).is_some())
            || matches!(
                kind,
                "session_info" | "model_change" | "thinking_level_change"
            )
        {
            return Ok(HarnessType::Pi);
        }
        if matches!(kind, "ai-title" | "progress" | "user" | "assistant")
            || ["costUSD", "toolsUse", "toolUse"]
                .iter()
                .any(|k| value.get(k).is_some())
        {
            return Ok(HarnessType::ClaudeCode);
        }
    }
    let first = &samples[0];
    if ClaudeCodeParser::new()
        .parse_line(first)
        .is_ok_and(|e| !e.is_empty())
    {
        return Ok(HarnessType::ClaudeCode);
    }
    if agent_parser::parse_pi_line(first, "").is_ok_and(|e| !e.is_empty()) {
        return Ok(HarnessType::Pi);
    }
    if CodexParser::new()
        .parse_line(first)
        .is_ok_and(|e| !e.is_empty())
    {
        return Ok(HarnessType::Codex);
    }
    Err(Error::UnknownHarness(path.display().to_string()))
}

fn entries(path: &Path) -> std::io::Result<Vec<fs::DirEntry>> {
    let mut result = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
    result.sort_by_key(fs::DirEntry::file_name);
    Ok(result)
}

fn jsonl(
    cancel: &CancellationToken,
    path: &Path,
    mut parser: impl FnMut(&[u8]) -> std::result::Result<Vec<ParsedEvent>, agent_parser::Error>,
    normalize_leading: bool,
) -> Result<(Vec<ReplayEvent>, Vec<String>)> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    let mut number = 0;
    let mut events = Vec::new();
    let mut diagnostics = Vec::new();
    let mut first_explicit = None;
    while read_line(&mut reader, &mut line)? > 0 {
        check_cancel(cancel)?;
        number += 1;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // Validate JSON even when a harness parser permissively ignores unknown records.
        let value = match serde_json::from_slice::<Value>(&line) {
            Ok(v) => v,
            Err(e) => {
                diagnostics.push(format!("{}:{number}: {e}", path.display()));
                continue;
            }
        };
        match parser(&line) {
            Ok(parsed) => {
                if normalize_leading && first_explicit.is_none() && value.get("timestamp").is_some()
                {
                    first_explicit = parsed
                        .iter()
                        .find(|e| e.timestamp > 0)
                        .map(|e| (events.len(), e.timestamp));
                }
                events.extend(parsed.into_iter().map(|event| ReplayEvent {
                    event,
                    source_line: number,
                    subagent_id: None,
                    is_synthetic: false,
                }));
            }
            Err(e) => diagnostics.push(format!("{}:{number}: {e}", path.display())),
        }
    }
    if let Some((count, timestamp)) = first_explicit {
        for event in &mut events[..count] {
            event.event.timestamp = timestamp;
        }
    }
    Ok((events, diagnostics))
}

/// Load a native transcript without modifying its source or OpenCode database.
pub fn load_session(
    cancel: &CancellationToken,
    path: &Path,
    options: &LoadOptions,
) -> Result<LoadedSession> {
    check_cancel(cancel)?;
    let harness = options
        .harness
        .clone()
        .map_or_else(|| detect_harness(path), Ok)?;
    if harness == HarnessType::Opencode {
        return load_opencode(cancel, path, options);
    }
    let mut claude = ClaudeCodeParser::new();
    let mut codex = CodexParser::new();
    let fallback = options
        .cwd
        .as_ref()
        .and_then(|p| p.to_str())
        .unwrap_or_default();
    let (mut events, mut diagnostics) = match harness {
        HarnessType::ClaudeCode => jsonl(cancel, path, |line| claude.parse_line(line), true)?,
        HarnessType::Codex => jsonl(cancel, path, |line| codex.parse_line(line), false)?,
        HarnessType::Pi => jsonl(
            cancel,
            path,
            |line| agent_parser::parse_pi_line(line, fallback),
            false,
        )?,
        _ => {
            return Err(Error::InvalidInput(format!(
                "unsupported harness: {}",
                harness.as_str()
            )));
        }
    };
    let session_id = options
        .session_id
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            (harness == HarnessType::Codex)
                .then(|| {
                    path.to_str()
                        .and_then(|s| agent_parser::parse_codex_rollout_path(s).ok())
                        .map(|r| r.session_id)
                })
                .flatten()
        })
        .map(Ok)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
                .ok_or_else(|| {
                    Error::InvalidInput("transcript name is not UTF-8; supply --session-id".into())
                })
        })?;
    let cwd = options.cwd.as_ref().map(|p| p.clean()).unwrap_or_else(|| {
        PathBuf::from(
            events
                .iter()
                .find(|e| !e.event.cwd.is_empty())
                .map(|e| e.event.cwd.as_str())
                .unwrap_or_default(),
        )
    });
    let mut subagents = Vec::new();
    if harness == HarnessType::ClaudeCode {
        let parent = path.parent().unwrap_or(Path::new("."));
        let mut seen = HashSet::new();
        for directory in [
            parent.join(&session_id).join("subagents"),
            parent.join("subagents"),
        ] {
            let listing = match entries(&directory) {
                Ok(e) => e,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in listing {
                check_cancel(cancel)?;
                if !entry.file_type()?.is_file()
                    || entry.path().extension().is_none_or(|e| e != "jsonl")
                {
                    continue;
                }
                let sub_id = entry
                    .path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        Error::InvalidInput("subagent transcript name is not UTF-8".into())
                    })?;
                if !seen.insert(sub_id.clone()) {
                    continue;
                }
                let mut parser = ClaudeCodeParser::new();
                let (mut sub_events, errors) =
                    jsonl(cancel, &entry.path(), |line| parser.parse_line(line), true)?;
                for event in &mut sub_events {
                    event.subagent_id = Some(sub_id.clone());
                }
                subagents.push(sub_id);
                events.extend(sub_events);
                diagnostics.extend(errors);
            }
        }
        events.sort_by_key(|e| e.event.timestamp);
    }
    if !cwd.as_os_str().is_empty() {
        let text = cwd.to_str().ok_or_else(|| {
            Error::InvalidInput(
                "working directory is not UTF-8 and cannot be represented in telemetry".into(),
            )
        })?;
        for event in &mut events {
            event.event.cwd = text.into();
        }
    }
    ensure_lifecycle(&mut events, 0, 0, &cwd, options.strict_lifecycle)?;
    Ok(LoadedSession {
        harness,
        session_id,
        cwd,
        harness_version: String::new(),
        events,
        subagents,
        diagnostics,
    })
}

fn load_opencode(
    cancel: &CancellationToken,
    path: &Path,
    options: &LoadOptions,
) -> Result<LoadedSession> {
    let db = OpencodeDb::open(path)?;
    check_cancel(cancel)?;
    let sessions = crate::opencode::query(cancel, &db, |db| db.get_sessions())?;
    let session = if let Some(id) = options.session_id.as_ref().filter(|s| !s.is_empty()) {
        sessions.into_iter().find(|s| &s.id == id).ok_or_else(|| {
            Error::InvalidInput(format!("session ID {id:?} not found in OpenCode database"))
        })?
    } else {
        if sessions.len() != 1 {
            return Err(Error::InvalidInput(format!(
                "expected one OpenCode session, found {}; specify --session-id ({})",
                sessions.len(),
                sessions
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        sessions
            .into_iter()
            .next()
            .ok_or_else(|| Error::InvalidInput("no OpenCode sessions".into()))?
    };
    let cwd = options
        .cwd
        .as_ref()
        .map(|p| p.clean())
        .unwrap_or_else(|| PathBuf::from(&session.directory));
    let cwd_text = cwd.to_str().ok_or_else(|| {
        Error::InvalidInput(
            "working directory is not UTF-8 and cannot be represented in telemetry".into(),
        )
    })?;
    check_cancel(cancel)?;
    let mut events =
        crate::opencode::query(cancel, &db, |db| db.reconstruct_session_events(&session.id))?
            .into_iter()
            .enumerate()
            .map(|(index, mut event)| {
                if options.cwd.is_some() || event.cwd.is_empty() {
                    event.cwd = cwd_text.into();
                }
                ReplayEvent {
                    event,
                    source_line: index + 1,
                    subagent_id: None,
                    is_synthetic: false,
                }
            })
            .collect::<Vec<_>>();
    check_cancel(cancel)?;
    ensure_lifecycle(
        &mut events,
        session.time_created,
        session.time_updated,
        &cwd,
        options.strict_lifecycle,
    )?;
    Ok(LoadedSession {
        harness: HarnessType::Opencode,
        session_id: session.id,
        cwd,
        harness_version: session.version,
        events,
        subagents: Vec::new(),
        diagnostics: Vec::new(),
    })
}

pub fn ensure_lifecycle(
    events: &mut Vec<ReplayEvent>,
    fallback_start: i64,
    fallback_end: i64,
    cwd: &Path,
    strict: bool,
) -> Result<()> {
    if strict || events.is_empty() {
        return Ok(());
    }
    let cwd = cwd.to_str().ok_or_else(|| {
        Error::InvalidInput(
            "working directory is not UTF-8 and cannot be represented in telemetry".into(),
        )
    })?;
    let native = |kind| {
        events.iter().any(|e| {
            e.subagent_id.as_ref().is_none_or(String::is_empty) && e.event.event_type == kind
        })
    };
    let has_start = native("run_started");
    let has_end = native("run_exited");
    let synthetic = |kind: &str, timestamp| ReplayEvent {
        event: ParsedEvent {
            event_type: kind.into(),
            timestamp,
            cwd: cwd.into(),
            ..Default::default()
        },
        source_line: 0,
        subagent_id: None,
        is_synthetic: true,
    };
    if !has_start {
        let timestamp = if fallback_start > 0 {
            fallback_start
        } else {
            events[0].event.timestamp
        };
        events.insert(0, synthetic("run_started", timestamp));
    }
    if !has_end {
        let last = events.last().map(|e| e.event.timestamp).unwrap_or_default();
        let timestamp = if fallback_end > last {
            fallback_end
        } else {
            last.checked_add(1)
                .ok_or_else(|| Error::InvalidInput("lifecycle timestamp overflow".into()))?
        };
        events.push(synthetic("run_exited", timestamp));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTarget {
    pub file_path: PathBuf,
    pub harness: HarnessType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SearchRoot {
    pub harness: HarnessType,
    pub path: PathBuf,
}

pub fn search_roots(home: &Path, lookup: impl Fn(&str) -> Option<String>) -> Vec<SearchRoot> {
    let mut claude = vec![
        home.join(".claude/projects"),
        home.join(".claude/sessions"),
        home.join(".local/share/ai-registry/claude-code/projects"),
        home.join(".local/share/ai-registry/claude-code/sessions"),
    ];
    if let Some(xdg) = lookup("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        claude.extend([
            Path::new(&xdg).join("ai-registry/claude-code/projects"),
            Path::new(&xdg).join("ai-registry/claude-code/sessions"),
        ]);
    }
    for name in ["CLAUDE_PROJECTS_DIR", "CLAUDE_SESSIONS_DIR"] {
        if let Some(path) = lookup(name).filter(|v| !v.is_empty()) {
            claude.insert(0, path.into());
        }
    }
    let mut roots = claude
        .into_iter()
        .map(|path| SearchRoot {
            harness: HarnessType::ClaudeCode,
            path,
        })
        .collect::<Vec<_>>();
    for (harness, name, fallback) in [
        (
            HarnessType::Pi,
            "PI_SESSIONS_DIR",
            home.join(".pi/agent/sessions"),
        ),
        (
            HarnessType::Codex,
            "CODEX_SESSIONS_DIR",
            home.join(".codex/sessions"),
        ),
        (
            HarnessType::Opencode,
            "OPENCODE_DB_PATH",
            lookup("XDG_DATA_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share"))
                .join("opencode/opencode.db"),
        ),
    ] {
        roots.push(SearchRoot {
            harness,
            path: lookup(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or(fallback),
        });
    }
    roots
}

fn files(cancel: &CancellationToken, root: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    check_cancel(cancel)?;
    for entry in entries(root)? {
        let kind = entry.file_type()?;
        if kind.is_dir() {
            files(cancel, &entry.path(), result)?;
        } else if kind.is_file() {
            result.push(entry.path());
        }
    }
    Ok(())
}

pub fn resolve_target(
    cancel: &CancellationToken,
    target: &Path,
    hint: Option<HarnessType>,
    roots: &[SearchRoot],
) -> Result<ResolvedTarget> {
    check_cancel(cancel)?;
    if target.as_os_str().is_empty() {
        return Err(Error::InvalidInput(
            "target file path or session ID cannot be empty".into(),
        ));
    }
    if target.exists() {
        let path = if target.is_dir() {
            let mut listing = Vec::new();
            files(cancel, target, &mut listing)?;
            listing
                .into_iter()
                .find(|p| matches!(p.extension().and_then(|s| s.to_str()), Some("jsonl" | "db")))
                .ok_or_else(|| {
                    Error::UnknownHarness(
                        "directory contains no session transcript or database".into(),
                    )
                })?
        } else {
            target.clean()
        };
        return Ok(ResolvedTarget {
            harness: hint.map_or_else(|| detect_harness(&path), Ok)?,
            file_path: path,
            session_id: None,
        });
    }
    let id = target
        .to_str()
        .ok_or_else(|| Error::InvalidInput("session ID is not UTF-8".into()))?;
    let mut exact = Vec::new();
    let mut partial = Vec::new();
    let mut visited = HashSet::new();
    for root in roots
        .iter()
        .filter(|r| hint.as_ref().is_none_or(|h| h == &r.harness))
    {
        check_cancel(cancel)?;
        if !root.path.exists() {
            continue;
        }
        if root.harness == HarnessType::Opencode {
            let db = OpencodeDb::open(&root.path)?;
            if crate::opencode::query(cancel, &db, |db| db.get_sessions())?
                .iter()
                .any(|session| session.id == id)
                && visited.insert((root.path.clone(), id.to_owned()))
            {
                exact.push(ResolvedTarget {
                    file_path: root.path.clone(),
                    harness: root.harness.clone(),
                    session_id: Some(id.into()),
                });
            }
            continue;
        }
        let mut listing = Vec::new();
        files(cancel, &root.path, &mut listing)?;
        for path in listing {
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let canonical = if root.harness == HarnessType::Codex {
                path.to_str()
                    .and_then(|path| agent_parser::parse_codex_rollout_path(path).ok())
                    .map(|rollout| rollout.session_id)
            } else {
                None
            };
            let canonical = canonical.as_deref().unwrap_or(stem);
            if !visited.insert((path.clone(), canonical.to_owned())) {
                continue;
            }
            let candidate = ResolvedTarget {
                file_path: path.clone(),
                harness: root.harness.clone(),
                session_id: None,
            };
            if stem == id || canonical == id {
                exact.push(candidate);
            } else if stem.contains(id) || canonical.contains(id) {
                partial.push(candidate);
            }
        }
    }
    let matches = if exact.is_empty() { partial } else { exact };
    if matches.len() == 1 {
        return matches
            .into_iter()
            .next()
            .ok_or_else(|| Error::InvalidInput("session lookup produced no match".into()));
    }
    if matches.len() > 1 {
        return Err(Error::InvalidInput(format!(
            "session fragment {id:?} is ambiguous; use the complete session ID or transcript path (matches: {})",
            matches
                .iter()
                .map(|target| target.file_path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Err(Error::InvalidInput(format!(
        "session matching {id:?} not found as a file or in standard harness directories"
    )))
}
