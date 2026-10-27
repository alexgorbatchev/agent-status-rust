use agent_observer::git::{Resolver, normalize_origin_url};
use std::{path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn unrelated_local_directories_with_same_basename_have_distinct_project_identity() {
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let first = dir.path().join("first/repository");
    let second = dir.path().join("second/repository");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    command(&first, &["init", "-b", "main"]);
    command(&second, &["init", "-b", "main"]);
    let resolver = Resolver::default();
    let cancel = CancellationToken::new();
    let (first_project, _) = resolver.resolve(&cancel, &first, "").await.unwrap();
    let (second_project, _) = resolver.resolve(&cancel, &second, "").await.unwrap();
    assert_eq!(first_project.name, second_project.name);
    assert_ne!(first_project.canonical_root, second_project.canonical_root);
    assert_ne!(first_project.id, second_project.id);
    let (repeated, _) = resolver.resolve(&cancel, &first, "").await.unwrap();
    assert_eq!(first_project.id, repeated.id);
}

fn command(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn remote_identity_accepts_git_protocols_and_omits_credentials() {
    for remote in [
        "git@example.com:owner/repo.git",
        "https://user:password@example.com/owner/repo.git",
        "ssh://git@example.com:2222/owner/repo.git",
        "git://example.com/owner/repo.git",
        "https://example.com/owner/repo.git/",
    ] {
        assert_eq!(
            normalize_origin_url(remote).as_deref(),
            Some("example.com/owner/repo")
        );
    }
    for remote in ["", "not-a-url", "/local/repo", "https://example.com/"] {
        assert_eq!(normalize_origin_url(remote), None);
    }
    assert_eq!(
        normalize_origin_url("https://example.com/owner/repo.git?token=private#fragment")
            .as_deref(),
        Some("example.com/owner/repo")
    );
    assert_eq!(
        normalize_origin_url("https://example.com/owner/repo%20name.git").as_deref(),
        Some("example.com/owner/repo name")
    );
}

#[tokio::test]
async fn resolves_actual_git_worktrees_detached_heads_and_cache_invalidation() {
    std::fs::create_dir_all(".tmp").unwrap();
    let root = tempfile::tempdir_in(".tmp").unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    command(&repo, &["init", "-b", "main"]);
    command(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    );
    command(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "git@example.com:owner/project.git",
        ],
    );
    let resolver = Resolver::new(Duration::ZERO);
    let cancel = CancellationToken::new();
    let (project, workspace) = resolver.resolve(&cancel, &repo, "").await.unwrap();
    assert_eq!(project.id, "example.com/owner/project");
    assert_eq!(project.name, "project");
    assert_eq!(workspace.branch, "main");
    assert!(!workspace.is_worktree);
    command(&repo, &["checkout", "--detach"]);
    assert_eq!(
        resolver.resolve(&cancel, &repo, "").await.unwrap().1.branch,
        "main"
    );
    resolver.invalidate(Some(&repo));
    let detached = resolver.resolve(&cancel, &repo, "").await.unwrap().1;
    assert!(detached.is_detached);
    assert!(!detached.branch.is_empty());
    let hinted = resolver
        .resolve(&cancel, &repo, "transcript-branch")
        .await
        .unwrap()
        .1;
    assert_eq!(hinted.branch, "transcript-branch");
    let linked = root.path().join("linked");
    command(
        &repo,
        &["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
    );
    let (linked_project, linked_workspace) = resolver.resolve(&cancel, &linked, "").await.unwrap();
    assert_eq!(linked_project, project);
    assert!(linked_workspace.is_worktree);
    assert_eq!(linked_workspace.rel_path.as_deref(), Some("../linked"));
    assert_eq!(linked_workspace.branch, "feature");
    resolver.invalidate(None);
    assert!(
        resolver
            .resolve(&cancel, &repo, "")
            .await
            .unwrap()
            .1
            .is_detached
    );
}

#[tokio::test]
async fn non_git_fallback_preserves_cwd_and_branch_but_never_swallows_cancellation() {
    std::fs::create_dir_all(".tmp").unwrap();
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let resolver = Resolver::new(Duration::from_millis(1));
    let cancel = CancellationToken::new();
    let (project, workspace) = resolver.resolve(&cancel, dir.path(), "hint").await.unwrap();
    assert!(project.id.starts_with("local/"));
    assert_eq!(workspace.branch, "hint");
    cancel.cancel();
    assert_eq!(
        resolver
            .resolve(&cancel, dir.path(), "hint")
            .await
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Interrupted
    );
}
