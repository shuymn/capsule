//! Git observations and bounded repository metadata acquisition.

use crate::acquire::{AcquireError, Runner};
use capsule_protocol::session::Snapshot;
use std::{
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

/// Ongoing git operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
pub enum GitState {
    /// Interactive or non-interactive rebase in progress.
    #[strum(serialize = "REBASING")]
    Rebase,
    /// Applying patches via `git am`.
    #[strum(serialize = "AM")]
    Am,
    /// Merge in progress.
    #[strum(serialize = "MERGING")]
    Merge,
    /// Cherry-pick in progress.
    #[strum(serialize = "CHERRY-PICKING")]
    CherryPick,
    /// Revert in progress.
    #[strum(serialize = "REVERTING")]
    Revert,
    /// Bisect session in progress.
    #[strum(serialize = "BISECTING")]
    Bisect,
}

/// Detected in-progress git operation with optional step progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitOperationState {
    /// The kind of operation.
    pub state: GitState,
    /// Current step (1-based), if applicable (rebase / am).
    pub step: Option<usize>,
    /// Total steps, if applicable (rebase / am).
    pub total: Option<usize>,
}

impl GitOperationState {
    /// Create an operation state with no step progress.
    const fn without_progress(state: GitState) -> Self {
        Self {
            state,
            step: None,
            total: None,
        }
    }
}

/// Git repository status information.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// Current branch name, or `None` if detached.
    pub branch: Option<String>,
    /// Full object id from `# branch.oid` (hex), set when git reports branch metadata.
    pub head_oid: Option<String>,
    /// Number of staged changes.
    pub staged: usize,
    /// Number of unstaged modifications.
    pub modified: usize,
    /// Number of untracked files.
    pub untracked: usize,
    /// Number of conflicted files.
    pub conflicted: usize,
    /// Number of stashed entries.
    pub stashed: usize,
    /// Number of deleted files.
    pub deleted: usize,
    /// Number of renamed files.
    pub renamed: usize,
    /// Commits ahead of upstream.
    pub ahead: usize,
    /// Commits behind upstream.
    pub behind: usize,
    /// Ongoing git operation (rebase, merge, etc.), if any.
    pub state: Option<GitOperationState>,
}

fn parse_porcelain_v2(output: &str) -> GitStatus {
    let mut status = GitStatus::default();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("# branch.oid ") {
            let oid = rest.trim();
            if !oid.is_empty() {
                status.head_oid = Some(oid.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("# branch.head ") {
            status.branch = if rest == "(detached)" {
                None
            } else {
                Some(rest.to_owned())
            };
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            parse_ahead_behind(rest, &mut status);
        } else if let Some(rest) = line.strip_prefix("# stash ") {
            status.stashed = rest.parse().unwrap_or(0);
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            parse_changed_entry(line, &mut status);
        } else if line.starts_with("u ") {
            status.conflicted += 1;
        } else if line.starts_with("? ") {
            status.untracked += 1;
        }
    }
    status
}

fn parse_ahead_behind(s: &str, status: &mut GitStatus) {
    for part in s.split_whitespace() {
        if let Some(n) = part.strip_prefix('+') {
            status.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix('-') {
            status.behind = n.parse().unwrap_or(0);
        }
    }
}

fn parse_changed_entry(line: &str, status: &mut GitStatus) {
    let Some(xy) = line.split_whitespace().nth(1) else {
        return;
    };
    let bytes = xy.as_bytes();
    if bytes.len() >= 2 {
        if bytes[0] != b'.' {
            status.staged += 1;
        }
        if bytes[1] != b'.' {
            status.modified += 1;
        }
        if bytes[0] == b'D' || bytes[1] == b'D' {
            status.deleted += 1;
        }
    }
    if line.starts_with("2 ") {
        status.renamed += 1;
    }
}

/// Filesystem and Git data acquired outside the view.
#[derive(Debug)]
pub struct ContextInfo {
    /// Repository-relative or home-abbreviated directory.
    pub directory: String,
    /// Current directory's acquired read-only permission flag.
    pub read_only: bool,
    /// Git status when enabled and available.
    pub git: Option<GitStatus>,
}

struct Repository {
    root: PathBuf,
    git_dir: PathBuf,
}

/// Format local directory data without filesystem I/O.
#[must_use]
pub fn local_directory(snapshot: &Snapshot) -> String {
    if let Some(home) = snapshot.env("HOME").filter(|home| !home.is_empty()) {
        let home = Path::new(home);
        if snapshot.cwd == home {
            return "~".to_owned();
        }
        if let Ok(suffix) = snapshot.cwd.strip_prefix(home) {
            return format!("~/{}", suffix.to_string_lossy());
        }
    }
    snapshot.cwd.to_string_lossy().into_owned()
}

/// Acquire directory metadata and Git with the shared bounded runner.
///
/// # Errors
/// Returns cancellation or a filesystem acquisition failure. A failed Git
/// command leaves directory data available and displays no Git information.
pub async fn acquire_context(
    snapshot: &Snapshot,
    runner: &Runner,
    cancel: &CancellationToken,
    git_enabled: bool,
) -> Result<ContextInfo, AcquireError> {
    let repo = find_repository(&snapshot.cwd, runner, cancel).await?;
    let directory = repo.as_ref().map_or_else(
        || local_directory(snapshot),
        |repo| {
            if snapshot.cwd == repo.root {
                repo.root.file_name().map_or_else(
                    || local_directory(snapshot),
                    |name| name.to_string_lossy().into_owned(),
                )
            } else {
                snapshot.cwd.strip_prefix(&repo.root).map_or_else(
                    |_| local_directory(snapshot),
                    |path| path.to_string_lossy().into_owned(),
                )
            }
        },
    );
    let read_only = runner
        .metadata(&snapshot.cwd, cancel)
        .await?
        .is_some_and(|metadata| metadata.permissions().readonly());
    let mut git = None;
    if git_enabled {
        let argv = [
            "git",
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
            "--show-stash",
        ]
        .map(str::to_owned);
        match runner.command(&argv, snapshot, cancel).await {
            Ok(output) => {
                let mut status = parse_porcelain_v2(&String::from_utf8_lossy(&output));
                if let Some(repo) = &repo {
                    status.state = detect_git_state(&repo.git_dir, runner, cancel).await?;
                }
                git = Some(status);
            }
            Err(AcquireError::Cancelled) => return Err(AcquireError::Cancelled),
            Err(error) => tracing::debug!(%error, "git acquisition unavailable"),
        }
    }
    Ok(ContextInfo {
        directory,
        read_only,
        git,
    })
}

async fn find_repository(
    cwd: &Path,
    runner: &Runner,
    cancel: &CancellationToken,
) -> Result<Option<Repository>, AcquireError> {
    for dir in cwd.ancestors().take(128) {
        let dot_git = dir.join(".git");
        let Some(metadata) = runner.metadata(&dot_git, cancel).await? else {
            continue;
        };
        if metadata.is_dir() {
            return Ok(Some(Repository {
                root: dir.to_owned(),
                git_dir: dot_git,
            }));
        }
        if metadata.is_file()
            && let Some(bytes) = runner.file(&dot_git, cancel).await?
            && let Some(path) = bytes.strip_prefix(b"gitdir: ")
        {
            let path = path.strip_suffix(b"\n").unwrap_or(path);
            let path = PathBuf::from(OsString::from_vec(path.to_vec()));
            let git_dir = if path.is_absolute() {
                path
            } else {
                dir.join(path)
            };
            return Ok(Some(Repository {
                root: dir.to_owned(),
                git_dir,
            }));
        }
    }
    Ok(None)
}

async fn detect_git_state(
    git_dir: &Path,
    runner: &Runner,
    cancel: &CancellationToken,
) -> Result<Option<GitOperationState>, AcquireError> {
    for (directory, current, total) in [
        ("rebase-merge", "msgnum", "end"),
        ("rebase-apply", "next", "last"),
    ] {
        let path = git_dir.join(directory);
        if runner
            .metadata(&path, cancel)
            .await?
            .is_some_and(|metadata| metadata.is_dir())
        {
            let state = if directory == "rebase-apply"
                && runner.exists(&path.join("applying"), cancel).await?
            {
                GitState::Am
            } else {
                GitState::Rebase
            };
            return Ok(Some(GitOperationState {
                state,
                step: read_count(&path.join(current), runner, cancel).await?,
                total: read_count(&path.join(total), runner, cancel).await?,
            }));
        }
    }
    for (file, state) in [
        ("MERGE_HEAD", GitState::Merge),
        ("CHERRY_PICK_HEAD", GitState::CherryPick),
        ("REVERT_HEAD", GitState::Revert),
        ("BISECT_LOG", GitState::Bisect),
    ] {
        if runner.exists(&git_dir.join(file), cancel).await? {
            return Ok(Some(GitOperationState::without_progress(state)));
        }
    }
    Ok(None)
}

async fn read_count(
    path: &Path,
    runner: &Runner,
    cancel: &CancellationToken,
) -> Result<Option<usize>, AcquireError> {
    Ok(runner
        .file(path, cancel)
        .await?
        .and_then(|bytes| std::str::from_utf8(&bytes).ok()?.trim().parse().ok()))
}

#[cfg(test)]
mod tests;
