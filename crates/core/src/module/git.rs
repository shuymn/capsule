//! Git module — displays git branch and working tree status.

use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Command,
};

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};
use crate::{
    render::style::{Color, ColorMap, Style},
    sealed,
};

/// Errors that can occur when querying git.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// Failed to execute the git command.
    #[error("failed to execute git command")]
    Command(#[source] std::io::Error),
}

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

/// Boxed future returned by async git status providers.
pub type GitStatusFuture =
    Pin<Box<dyn Future<Output = Result<Option<GitStatus>, GitError>> + Send + 'static>>;

/// Provides git repository information.
pub trait GitProvider: sealed::Sealed {
    /// Query the git status of the repository at `cwd`.
    ///
    /// `path_env` overrides the `PATH` environment variable for the spawned
    /// process, allowing the daemon to use the shell's PATH (important under
    /// launchd where the daemon's PATH is minimal).
    ///
    /// Returns `Ok(None)` if `cwd` is not inside a git repository.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] if the git command cannot be executed.
    fn status(&self, cwd: &Path, path_env: Option<&str>) -> Result<Option<GitStatus>, GitError>;

    /// Async daemon-facing status query.
    ///
    /// The default implementation delegates synchronous providers to a
    /// blocking thread. [`CommandGitProvider`] overrides this to run the git
    /// process with async I/O and `kill_on_drop`.
    fn status_async(self, cwd: PathBuf, path_env: Option<String>) -> GitStatusFuture
    where
        Self: Sized + Send + 'static,
    {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || self.status(&cwd, path_env.as_deref()))
                .await
                .map_err(|source| {
                    GitError::Command(std::io::Error::other(format!("join error: {source}")))
                })?
        })
    }
}

/// [`GitProvider`] that shells out to the `git` command.
#[derive(Debug, Clone)]
#[allow(clippy::module_name_repetitions)]
pub struct CommandGitProvider;

impl sealed::Sealed for CommandGitProvider {}

impl GitProvider for CommandGitProvider {
    fn status(&self, cwd: &Path, path_env: Option<&str>) -> Result<Option<GitStatus>, GitError> {
        let mut cmd = Command::new("git");
        cmd.args(["status", "--porcelain=v2", "--branch", "--show-stash"])
            .current_dir(cwd)
            .stderr(std::process::Stdio::null());
        if let Some(path) = path_env {
            cmd.env("PATH", path);
        }
        let output = cmd.output().map_err(GitError::Command)?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut status = parse_porcelain_v2(&stdout);

        // Detect in-progress operations via filesystem sentinel files.
        if let Some(git_dir) = find_git_dir(cwd) {
            status.state = detect_git_state(&git_dir);
        }

        Ok(Some(status))
    }

    fn status_async(self, cwd: PathBuf, path_env: Option<String>) -> GitStatusFuture {
        Box::pin(async move { command_git_status_async(&cwd, path_env.as_deref()).await })
    }
}

async fn command_git_status_async(
    cwd: &Path,
    path_env: Option<&str>,
) -> Result<Option<GitStatus>, GitError> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.kill_on_drop(true)
        .args(["status", "--porcelain=v2", "--branch", "--show-stash"])
        .current_dir(cwd)
        .stderr(std::process::Stdio::null());
    if let Some(path) = path_env {
        cmd.env("PATH", path);
    }
    let output = cmd.output().await.map_err(GitError::Command)?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut status = parse_porcelain_v2(&stdout);

    let state_cwd = cwd.to_path_buf();
    status.state = tokio::task::spawn_blocking(move || {
        find_git_dir(&state_cwd).and_then(|git_dir| detect_git_state(&git_dir))
    })
    .await
    .map_err(|source| GitError::Command(std::io::Error::other(format!("join error: {source}"))))?;

    Ok(Some(status))
}

/// Bundled style configuration for git output rendering.
#[derive(Debug, Clone, Copy)]
pub struct GitStyles {
    /// Style for the branch name and icon.
    pub branch: Style,
    /// Style for `(hash)` in detached `HEAD (hash)`.
    pub detached_hash: Style,
    /// Style for status indicators (e.g., `[!+]`).
    pub indicator: Style,
    /// Style for operation state labels (e.g., `(REBASING 2/5)`).
    pub state: Style,
    /// ANSI color code overrides.
    pub color_map: ColorMap,
}

impl Default for GitStyles {
    fn default() -> Self {
        Self {
            branch: Style::new().fg(Color::Magenta).bold(),
            detached_hash: Style::new().fg(Color::Green).dimmed(),
            indicator: Style::new().fg(Color::Red).bold(),
            state: Style::new().fg(Color::Yellow).bold(),
            color_map: ColorMap::default(),
        }
    }
}

/// Displays git branch and working tree status.
///
/// Returns `None` when not inside a git repository.
#[derive(Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct GitModule<G> {
    provider: G,
    styles: GitStyles,
}

impl<G> GitModule<G> {
    /// Creates a new `GitModule` with the given provider and default styles.
    pub fn new(provider: G) -> Self {
        Self {
            provider,
            styles: GitStyles::default(),
        }
    }

    /// Creates a new `GitModule` with explicit styles.
    pub const fn with_styles(provider: G, styles: GitStyles) -> Self {
        Self { provider, styles }
    }
}

impl<G: GitProvider> GitModule<G> {
    pub(crate) fn render_status(&self, status: &GitStatus) -> Option<ModuleOutput> {
        render_status_with_styles(status, &self.styles)
    }

    /// Renders git status for the given working directory.
    ///
    /// This is the core implementation used by both [`Module::render`] and
    /// the daemon's slow-module path (which has no full [`RenderContext`]).
    pub fn render_for_cwd(&self, cwd: &Path, path_env: Option<&str>) -> Option<ModuleOutput> {
        let status = match self.provider.status(cwd, path_env) {
            Ok(Some(s)) => s,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(error = %e, cwd = %cwd.display(), "git status failed");
                return None;
            }
        };
        self.render_status(&status)
    }
}

pub(crate) fn render_status_with_styles(
    status: &GitStatus,
    styles: &GitStyles,
) -> Option<ModuleOutput> {
    let content = format_git_output(status, styles);
    if content.is_empty() {
        return None;
    }
    Some(ModuleOutput { content })
}

impl<G: GitProvider> sealed::Sealed for GitModule<G> {}

impl<G: GitProvider> Module for GitModule<G> {
    fn name(&self) -> &'static str {
        "git"
    }

    fn speed(&self) -> ModuleSpeed {
        ModuleSpeed::Slow
    }

    fn render(&self, ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        self.render_for_cwd(ctx.cwd, None)
    }
}

// ---------------------------------------------------------------------------
// Git directory discovery and state detection
// ---------------------------------------------------------------------------

/// Find the `.git` directory for a repository containing `cwd`.
///
/// Walks up from `cwd` looking for a `.git` entry. If it is a regular
/// directory, returns it directly. If it is a file (git worktree), reads
/// the `gitdir:` pointer and resolves the path.
fn find_git_dir(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd;
    loop {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            return read_gitdir_pointer(&dot_git);
        }
        dir = dir.parent()?;
    }
}

/// Read a `.git` worktree pointer file and resolve the gitdir path.
fn read_gitdir_pointer(dot_git_file: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(dot_git_file).ok()?;
    let gitdir = content.strip_prefix("gitdir: ")?.trim();
    let path = Path::new(gitdir);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        dot_git_file.parent().map(|p| p.join(path))
    }
}

/// Detect the current in-progress git operation by inspecting sentinel
/// files in the git directory.
///
/// Priority order matches git's own status reporting.
fn detect_git_state(git_dir: &Path) -> Option<GitOperationState> {
    let rebase_merge = git_dir.join("rebase-merge");
    if rebase_merge.is_dir() {
        let step = read_usize_file(&rebase_merge.join("msgnum"));
        let total = read_usize_file(&rebase_merge.join("end"));
        return Some(GitOperationState {
            state: GitState::Rebase,
            step,
            total,
        });
    }

    let rebase_apply = git_dir.join("rebase-apply");
    if rebase_apply.is_dir() {
        let state = if rebase_apply.join("applying").exists() {
            GitState::Am
        } else {
            GitState::Rebase
        };
        let step = read_usize_file(&rebase_apply.join("next"));
        let total = read_usize_file(&rebase_apply.join("last"));
        return Some(GitOperationState { state, step, total });
    }

    if git_dir.join("MERGE_HEAD").exists() {
        return Some(GitOperationState::without_progress(GitState::Merge));
    }

    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Some(GitOperationState::without_progress(GitState::CherryPick));
    }

    if git_dir.join("REVERT_HEAD").exists() {
        return Some(GitOperationState::without_progress(GitState::Revert));
    }

    if git_dir.join("BISECT_LOG").exists() {
        return Some(GitOperationState::without_progress(GitState::Bisect));
    }

    None
}

/// Read a file containing a single `usize` value (used for rebase progress).
fn read_usize_file(path: &Path) -> Option<usize> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// Short hash length inside detached `HEAD (hash)` (common `git rev-parse --short` width).
const DETACHED_OID_PREFIX_LEN: usize = 7;

fn short_commit_prefix(full_oid: &str) -> &str {
    // Git OIDs are ASCII hex, so byte-slicing is safe up to len.
    &full_oid[..full_oid.len().min(DETACHED_OID_PREFIX_LEN)]
}

fn write_state(buf: &mut String, op_state: &GitOperationState) {
    use std::fmt::Write;
    match (op_state.step, op_state.total) {
        (Some(step), Some(total)) => {
            let _ = write!(buf, "({} {step}/{total})", op_state.state);
        }
        _ => {
            let _ = write!(buf, "({})", op_state.state);
        }
    }
}

fn format_git_output(status: &GitStatus, styles: &GitStyles) -> String {
    let mut out = String::with_capacity(64);

    if let Some(ref branch) = status.branch {
        out.push_str(&styles.branch.paint_with(branch, styles.color_map));
    } else if let Some(ref oid) = status.head_oid {
        let prefix = short_commit_prefix(oid);
        if !prefix.is_empty() {
            out.push_str(&styles.branch.paint_with("HEAD ", styles.color_map));
            let paren = format!("({prefix})");
            out.push_str(&styles.detached_hash.paint_with(&paren, styles.color_map));
        }
    }

    // State label (rebase, merge, etc.) between branch and indicators
    if let Some(ref op_state) = status.state {
        if !out.is_empty() {
            out.push(' ');
        }
        let mut state_buf = String::with_capacity(24);
        write_state(&mut state_buf, op_state);
        out.push_str(&styles.state.paint_with(&state_buf, styles.color_map));
    }

    // Indicator order follows Starship defaults: = $ ✘ » ! + ? ⇕/⇡⇣
    // Max content: 7 single-char indicators + 1 diverge indicator + 2 brackets = ~40 bytes (UTF-8 multi-byte)
    let mut indicators = String::with_capacity(40);
    if status.conflicted > 0 {
        indicators.push('=');
    }
    if status.stashed > 0 {
        indicators.push('$');
    }
    if status.deleted > 0 {
        indicators.push('✘');
    }
    if status.renamed > 0 {
        indicators.push('»');
    }
    if status.modified > 0 {
        indicators.push('!');
    }
    if status.staged > 0 {
        indicators.push('+');
    }
    if status.untracked > 0 {
        indicators.push('?');
    }
    if status.ahead > 0 && status.behind > 0 {
        indicators.push('⇕');
    } else if status.ahead > 0 {
        indicators.push('⇡');
    } else if status.behind > 0 {
        indicators.push('⇣');
    }

    if !indicators.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        indicators.insert(0, '[');
        indicators.push(']');
        out.push_str(&styles.indicator.paint_with(&indicators, styles.color_map));
    }

    out
}

#[cfg(test)]
mod tests;
