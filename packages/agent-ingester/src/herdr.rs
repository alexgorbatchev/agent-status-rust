//! Native newline-delimited Herdr RPC over one Unix connection per request.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("pane not found")]
    PaneNotFound,
    #[error("herdr service unavailable: {0}")]
    Unavailable(#[source] io::Error),
    #[error("herdr RPC I/O: {0}")]
    Io(#[from] io::Error),
    #[error("herdr RPC JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("herdr RPC error (code {code}): {message}")]
    Rpc { code: i64, message: String },
    #[error("herdr response ID {actual:?} does not match request {expected:?}")]
    ResponseIdentity { expected: String, actual: String },
    #[error("herdr RPC timed out")]
    Timeout,
    #[error("herdr RPC canceled")]
    Cancelled,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PaneInfo {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
}
#[derive(Deserialize)]
struct Response {
    id: String,
    #[serde(default)]
    result: Value,
    error: Option<RpcError>,
}
#[derive(Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Pane {
    pane_id: String,
    workspace_id: String,
    tab_id: String,
    title: String,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum PaneList {
    Object { panes: Vec<Pane> },
    Array(Vec<Pane>),
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct ProcessInfo {
    pid: i64,
    name: String,
    argv0: String,
    cwd: String,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Processes {
    foreground_processes: Vec<ProcessInfo>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct ProcessResult {
    process_info: Processes,
    foreground_processes: Vec<ProcessInfo>,
}

pub struct Client {
    socket_path: PathBuf,
    sequence: AtomicU64,
    timeout: Duration,
}
impl Client {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            sequence: AtomicU64::new(0),
            timeout: Duration::from_secs(5),
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    pub async fn call<T: DeserializeOwned>(
        &self,
        cancel: &CancellationToken,
        method: &str,
        params: Value,
    ) -> Result<T, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let operation = async {
            let stream = UnixStream::connect(&self.socket_path)
                .await
                .map_err(Error::Unavailable)?;
            let id = format!(
                "req-{}",
                self.sequence
                    .fetch_add(1, Ordering::Relaxed)
                    .wrapping_add(1)
            );
            let mut request = serde_json::to_vec(
                &json!({"id":id,"method":method,"params":if params.is_null() {json!({})} else {params}}),
            )?;
            request.push(b'\n');
            let mut reader = BufReader::new(stream);
            reader.get_mut().write_all(&request).await?;
            let mut line = Vec::new();
            // A complete response is delimited by a newline, not by connection EOF.
            reader
                .take(64 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut line)
                .await?;
            if line.len() > 64 * 1024 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "herdr response exceeds 64 MiB",
                )
                .into());
            }
            if line.last() != Some(&b'\n') {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete herdr response",
                )
                .into());
            }
            let response: Response = serde_json::from_slice(&line)?;
            if response.id != id {
                return Err(Error::ResponseIdentity {
                    expected: id,
                    actual: response.id,
                });
            }
            if let Some(error) = response.error {
                return Err(Error::Rpc {
                    code: error.code,
                    message: error.message,
                });
            }
            Ok(serde_json::from_value(response.result)?)
        };
        tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Cancelled),result=tokio::time::timeout(self.timeout,operation)=>result.map_err(|_|Error::Timeout)?}
    }
    pub async fn ping(&self, cancel: &CancellationToken) -> Result<(), Error> {
        self.call::<Value>(cancel, "ping", Value::Null).await?;
        Ok(())
    }
    pub async fn find_pane(
        &self,
        cancel: &CancellationToken,
        pid: i64,
        cwd: &str,
        harness: &str,
    ) -> Result<PaneInfo, Error> {
        let panes = match self.call(cancel, "pane.list", Value::Null).await? {
            PaneList::Object { panes } | PaneList::Array(panes) => panes,
        };
        let mut fallback = None;
        let clean_cwd = path_clean::clean(cwd);
        let harness = harness.to_lowercase();
        for pane in panes {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let result: ProcessResult = match self
                .call(cancel, "pane.process_info", json!({"pane_id":pane.pane_id}))
                .await
            {
                Ok(value) => value,
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(_) => continue,
            };
            let processes = if result.process_info.foreground_processes.is_empty() {
                result.foreground_processes
            } else {
                result.process_info.foreground_processes
            };
            let pane = PaneInfo {
                pane_id: pane.pane_id,
                workspace_id: pane.workspace_id,
                tab_id: pane.tab_id,
                title: pane.title,
            };
            for process in processes {
                if pid > 0 && process.pid == pid {
                    return Ok(pane);
                }
                if fallback.is_none()
                    && !cwd.is_empty()
                    && !process.cwd.is_empty()
                    && path_clean::clean(&process.cwd) == clean_cwd
                    && !harness.is_empty()
                    && (process.argv0.to_lowercase().contains(&harness)
                        || process.name.to_lowercase().contains(&harness))
                {
                    fallback = Some(pane.clone());
                }
            }
        }
        fallback.ok_or(Error::PaneNotFound)
    }
    pub async fn focus_pane(&self, cancel: &CancellationToken, pane_id: &str) -> Result<(), Error> {
        self.call::<Value>(cancel, "pane.focus", json!({"pane_id":pane_id}))
            .await?;
        #[cfg(target_os = "macos")]
        if let Some(app) = std::env::var_os("HERDR_TERMINAL_APP")
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var_os("TERM_PROGRAM").filter(|s| !s.is_empty()))
        {
            let mut command = activation_command(Path::new("/usr/bin/osascript"), &app);
            if let Ok(mut child) = command.spawn() {
                tokio::select! {biased;_=cancel.cancelled()=>{let _=child.kill().await;},_=child.wait()=>{}}
            }
        }
        Ok(())
    }
}

/// The application name is an argument rather than executable AppleScript text.
pub fn activation_command(executable: &Path, app: &std::ffi::OsStr) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(executable);
    command
        .args([
            "-e",
            "on run argv\n tell application (item 1 of argv) to activate\nend run",
            "--",
        ])
        .arg(app)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}
