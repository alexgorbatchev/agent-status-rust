use agent_common::types::{ProjectInfo, WorkspaceInfo};
use path_clean::PathClean;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::RwLock,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, process::Command};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct CacheEntry {
    pub project: ProjectInfo,
    pub workspace: WorkspaceInfo,
    pub resolved_at: Instant,
}

pub struct Resolver {
    cache: RwLock<HashMap<PathBuf, CacheEntry>>,
    ttl: Duration,
}

impl Resolver {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            ttl: if ttl.is_zero() {
                DEFAULT_CACHE_TTL
            } else {
                ttl
            },
        }
    }

    pub fn invalidate(&self, cwd: Option<&Path>) {
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        match cwd {
            Some(cwd) => {
                cache.remove(&cwd.clean());
            }
            None => cache.clear(),
        }
    }

    pub async fn resolve(
        &self,
        cancel: &CancellationToken,
        cwd: impl AsRef<Path>,
        branch_hint: &str,
    ) -> io::Result<(ProjectInfo, WorkspaceInfo)> {
        cancelled(cancel)?;
        let cwd = cwd.as_ref().clean();
        if let Some(entry) = self
            .cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&cwd)
            .filter(|e| {
                e.resolved_at.elapsed() < self.ttl
                    && (branch_hint.is_empty() || branch_hint == e.workspace.branch)
            })
        {
            return Ok((entry.project.clone(), entry.workspace.clone()));
        }
        let (project, workspace) = match self.resolve_git(cancel, &cwd, branch_hint).await {
            Ok(result) => result,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
            Err(_) => (
                local_project(&cwd),
                WorkspaceInfo {
                    cwd: cwd.to_string_lossy().into_owned(),
                    branch: branch_hint.into(),
                    ..Default::default()
                },
            ),
        };
        self.cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                cwd,
                CacheEntry {
                    project: project.clone(),
                    workspace: workspace.clone(),
                    resolved_at: Instant::now(),
                },
            );
        Ok((project, workspace))
    }

    async fn resolve_git(
        &self,
        cancel: &CancellationToken,
        cwd: &Path,
        branch_hint: &str,
    ) -> io::Result<(ProjectInfo, WorkspaceInfo)> {
        let common = PathBuf::from(
            git(
                cancel,
                cwd,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?,
        )
        .clean();
        let root = if common.file_name().is_some_and(|v| v == ".git") {
            common.parent().unwrap_or(&common).to_path_buf()
        } else {
            common
        };
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let top = PathBuf::from(git(cancel, cwd, &["rev-parse", "--show-toplevel"]).await?).clean();
        let top = std::fs::canonicalize(&top).unwrap_or(top);
        let mut workspace = WorkspaceInfo {
            cwd: cwd.to_string_lossy().into_owned(),
            is_worktree: top != root,
            branch: branch_hint.into(),
            ..Default::default()
        };
        if workspace.is_worktree {
            workspace.rel_path = Some(
                pathdiff::diff_paths(&top, &root)
                    .unwrap_or(top)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if branch_hint.is_empty() {
            match git(cancel, cwd, &["symbolic-ref", "--short", "HEAD"]).await {
                Ok(branch) => workspace.branch = branch,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
                Err(_) => {
                    workspace.is_detached = true;
                    workspace.branch =
                        match git(cancel, cwd, &["rev-parse", "--short", "HEAD"]).await {
                            Ok(branch) => branch,
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                                return Err(error);
                            }
                            Err(_) => "HEAD".into(),
                        };
                }
            }
        }
        let remote = match git(cancel, cwd, &["remote", "get-url", "origin"]).await {
            Ok(remote) => remote,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
            Err(_) => String::new(),
        };
        let project = match normalize_origin_url(&remote) {
            Some(id) => ProjectInfo {
                name: extract_repo_name(&id).into(),
                id,
                canonical_root: root.to_string_lossy().into_owned(),
            },
            None => local_project(&root),
        };
        Ok((project, workspace))
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_TTL)
    }
}

fn cancelled(cancel: &CancellationToken) -> io::Result<()> {
    if cancel.is_cancelled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "workspace resolution cancelled",
        ))
    } else {
        Ok(())
    }
}

async fn git(cancel: &CancellationToken, cwd: &Path, args: &[&str]) -> io::Result<String> {
    cancelled(cancel)?;
    let mut child = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().expect("piped child stdout");
    let mut bytes = Vec::new();
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => { child.kill().await?; return cancelled(cancel).map(|_| String::new()); },
        result = async { stdout.read_to_end(&mut bytes).await?; child.wait().await } => result?,
    };
    if !result.success() {
        return Err(io::Error::other("git metadata unavailable"));
    }
    Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
}

fn local_project(root: &Path) -> ProjectInfo {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let name = root
        .file_name()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into());
    ProjectInfo {
        id: format!(
            "local/{name}/{:x}",
            Sha256::digest(root.as_os_str().as_encoded_bytes())
        ),
        name,
        canonical_root: root.to_string_lossy().into_owned(),
    }
}

pub fn normalize_origin_url(raw: &str) -> Option<String> {
    if raw.contains("://") {
        let url = url::Url::parse(raw.trim()).ok()?;
        let host = url.host_str()?;
        let path = percent_encoding::percent_decode_str(url.path())
            .decode_utf8()
            .ok()?;
        let path = path.trim_start_matches('/').trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        return (!host.is_empty() && !path.is_empty()).then(|| format!("{host}/{path}"));
    }
    let url = gix_url::parse(raw.trim().into()).ok()?;
    let host = url.host()?;
    let path = std::str::from_utf8(url.path.as_ref())
        .ok()?
        .trim_start_matches('/')
        .trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() {
        None
    } else {
        Some(format!("{host}/{path}"))
    }
}

pub fn extract_repo_name(normalized: &str) -> &str {
    normalized.rsplit('/').next().unwrap_or_default()
}
