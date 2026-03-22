//! Git module — displays git branch and working tree status.

use std::{
    path::{Path, PathBuf},
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
        let content = format_git_output(&status, &self.styles);
        if content.is_empty() {
            return None;
        }
        Some(ModuleOutput { content })
    }
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
    let end = full_oid.len().min(DETACHED_OID_PREFIX_LEN);
    full_oid.get(..end).unwrap_or("")
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
            let mut paren = String::with_capacity(prefix.len() + 2);
            paren.push('(');
            paren.push_str(prefix);
            paren.push(')');
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
mod tests {
    use std::path::Path;

    use super::*;
    use crate::{render::layout::display_width, test_utils::contains_style_sequence};

    // -- GitState Display tests --

    #[test]
    fn test_git_state_display_rebase() {
        assert_eq!(GitState::Rebase.to_string(), "REBASING");
    }

    #[test]
    fn test_git_state_display_am() {
        assert_eq!(GitState::Am.to_string(), "AM");
    }

    #[test]
    fn test_git_state_display_merge() {
        assert_eq!(GitState::Merge.to_string(), "MERGING");
    }

    #[test]
    fn test_git_state_display_cherry_pick() {
        assert_eq!(GitState::CherryPick.to_string(), "CHERRY-PICKING");
    }

    #[test]
    fn test_git_state_display_revert() {
        assert_eq!(GitState::Revert.to_string(), "REVERTING");
    }

    #[test]
    fn test_git_state_display_bisect() {
        assert_eq!(GitState::Bisect.to_string(), "BISECTING");
    }

    // -- find_git_dir tests --

    #[test]
    fn test_find_git_dir_normal_repo() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".git"))?;
        let result = find_git_dir(dir.path());
        assert_eq!(result, Some(dir.path().join(".git")));
        Ok(())
    }

    #[test]
    fn test_find_git_dir_subdirectory() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".git"))?;
        let sub = dir.path().join("src").join("deep");
        std::fs::create_dir_all(&sub)?;
        let result = find_git_dir(&sub);
        assert_eq!(result, Some(dir.path().join(".git")));
        Ok(())
    }

    #[test]
    fn test_find_git_dir_worktree() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let gitdir_target = dir.path().join("actual-gitdir");
        std::fs::create_dir(&gitdir_target)?;
        let worktree = dir.path().join("worktree");
        std::fs::create_dir(&worktree)?;
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}", gitdir_target.display()),
        )?;
        let result = find_git_dir(&worktree);
        assert_eq!(result, Some(gitdir_target));
        Ok(())
    }

    #[test]
    fn test_find_git_dir_worktree_relative() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let gitdir_target = dir.path().join("actual-gitdir");
        std::fs::create_dir(&gitdir_target)?;
        let worktree = dir.path().join("worktree");
        std::fs::create_dir(&worktree)?;
        std::fs::write(worktree.join(".git"), "gitdir: ../actual-gitdir\n")?;
        let result = find_git_dir(&worktree);
        assert!(result.is_some(), "should resolve relative gitdir pointer");
        assert!(
            result
                .as_ref()
                .is_some_and(|p| p.ends_with("actual-gitdir")),
            "resolved path should end with actual-gitdir: {result:?}",
        );
        Ok(())
    }

    #[test]
    fn test_find_git_dir_not_a_repo() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let result = find_git_dir(dir.path());
        // tempdir is under /tmp or similar — may find system .git if any; safest
        // is to verify that the returned path (if any) is not inside our tempdir.
        if let Some(ref p) = result {
            assert!(
                !p.starts_with(dir.path()),
                "should not find .git inside our tempdir: {p:?}",
            );
        }
        Ok(())
    }

    // -- detect_git_state tests --

    #[test]
    fn test_detect_rebase_merge_with_progress() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let rebase = dir.path().join("rebase-merge");
        std::fs::create_dir(&rebase)?;
        std::fs::write(rebase.join("msgnum"), "3\n")?;
        std::fs::write(rebase.join("end"), "7\n")?;
        let result = detect_git_state(dir.path());
        assert_eq!(
            result,
            Some(GitOperationState {
                state: GitState::Rebase,
                step: Some(3),
                total: Some(7),
            }),
        );
        Ok(())
    }

    #[test]
    fn test_detect_rebase_merge_without_progress() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join("rebase-merge"))?;
        let result = detect_git_state(dir.path());
        assert_eq!(
            result,
            Some(GitOperationState {
                state: GitState::Rebase,
                step: None,
                total: None,
            }),
            "rebase-merge dir without msgnum/end should have None step/total",
        );
        Ok(())
    }

    #[test]
    fn test_detect_rebase_apply() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let rebase = dir.path().join("rebase-apply");
        std::fs::create_dir(&rebase)?;
        std::fs::write(rebase.join("next"), "2\n")?;
        std::fs::write(rebase.join("last"), "5\n")?;
        let result = detect_git_state(dir.path());
        assert_eq!(
            result,
            Some(GitOperationState {
                state: GitState::Rebase,
                step: Some(2),
                total: Some(5),
            }),
        );
        Ok(())
    }

    #[test]
    fn test_detect_am() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let rebase = dir.path().join("rebase-apply");
        std::fs::create_dir(&rebase)?;
        std::fs::write(rebase.join("applying"), "")?;
        std::fs::write(rebase.join("next"), "1\n")?;
        std::fs::write(rebase.join("last"), "3\n")?;
        let result = detect_git_state(dir.path());
        assert_eq!(
            result,
            Some(GitOperationState {
                state: GitState::Am,
                step: Some(1),
                total: Some(3),
            }),
        );
        Ok(())
    }

    #[test]
    fn test_detect_merge() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("MERGE_HEAD"), "abc123\n")?;
        let result = detect_git_state(dir.path());
        assert_eq!(
            result,
            Some(GitOperationState::without_progress(GitState::Merge)),
        );
        Ok(())
    }

    #[test]
    fn test_detect_cherry_pick() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("CHERRY_PICK_HEAD"), "abc123\n")?;
        let result = detect_git_state(dir.path());
        assert_eq!(
            result,
            Some(GitOperationState::without_progress(GitState::CherryPick)),
        );
        Ok(())
    }

    #[test]
    fn test_detect_revert() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("REVERT_HEAD"), "abc123\n")?;
        let result = detect_git_state(dir.path());
        assert_eq!(
            result,
            Some(GitOperationState::without_progress(GitState::Revert)),
        );
        Ok(())
    }

    #[test]
    fn test_detect_bisect() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("BISECT_LOG"), "")?;
        let result = detect_git_state(dir.path());
        assert_eq!(
            result,
            Some(GitOperationState::without_progress(GitState::Bisect)),
        );
        Ok(())
    }

    #[test]
    fn test_detect_no_state() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let result = detect_git_state(dir.path());
        assert_eq!(result, None);
        Ok(())
    }

    #[test]
    fn test_detect_priority_rebase_over_merge() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join("rebase-merge"))?;
        std::fs::write(dir.path().join("MERGE_HEAD"), "abc123\n")?;
        let result = detect_git_state(dir.path());
        assert!(
            result.is_some_and(|s| s.state == GitState::Rebase),
            "rebase should take priority over merge: {result:?}",
        );
        Ok(())
    }

    fn default_styles() -> GitStyles {
        GitStyles::default()
    }

    // -- Parsing tests --

    #[test]
    fn test_parse_porcelain_v2_branch_and_counts() {
        let output = "\
# branch.oid abc123def456
# branch.head main
# branch.ab +1 -2
1 M. N... 000000 000000 abc123 def456 modified.rs
1 .M N... 000000 000000 abc123 def456 worktree.rs
? untracked.txt
";
        let status = parse_porcelain_v2(output);
        assert_eq!(status.branch, Some("main".to_owned()));
        assert_eq!(status.ahead, 1);
        assert_eq!(status.behind, 2);
        assert_eq!(status.staged, 1);
        assert_eq!(status.modified, 1);
        assert_eq!(status.untracked, 1);
        assert_eq!(status.conflicted, 0);
        assert_eq!(
            status.head_oid,
            Some("abc123def456".to_owned()),
            "full oid from porcelain"
        );
    }

    #[test]
    fn test_parse_porcelain_v2_detached_head() {
        let output = "# branch.oid abc123\n# branch.head (detached)\n";
        let status = parse_porcelain_v2(output);
        assert_eq!(status.branch, None);
        assert_eq!(status.head_oid, Some("abc123".to_owned()));
    }

    #[test]
    fn test_parse_porcelain_v2_staged_and_modified() {
        let output = "# branch.head feature\n1 MM N... 000000 000000 abc123 def456 both.rs\n";
        let status = parse_porcelain_v2(output);
        assert_eq!(status.staged, 1);
        assert_eq!(status.modified, 1);
    }

    #[test]
    fn test_parse_porcelain_v2_conflicted() {
        let output =
            "# branch.head main\nu UU N... 000000 000000 000000 abc123 def456 ghi789 conflict.rs\n";
        let status = parse_porcelain_v2(output);
        assert_eq!(status.conflicted, 1);
    }

    #[test]
    fn test_parse_porcelain_v2_rename_entry() {
        let output =
            "# branch.head main\n2 R. N... 000000 000000 abc123 def456 R100 new.rs\told.rs\n";
        let status = parse_porcelain_v2(output);
        assert_eq!(status.staged, 1);
        assert_eq!(status.modified, 0);
    }

    #[test]
    fn test_parse_porcelain_v2_empty_output() {
        let status = parse_porcelain_v2("");
        assert_eq!(status, GitStatus::default());
    }

    // -- Format tests --

    #[test]
    fn test_format_git_output_branch_only() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        assert_eq!(display_width(&output), 4, "visible width: {output:?}");
        assert!(output.contains("main"), "should contain branch name");
        assert!(
            contains_style_sequence(&output, &[1, 35]),
            "branch should be bold magenta"
        );
        // No indicators → display width is just the branch name
        assert_eq!(
            display_width(&output),
            display_width("main"),
            "no extra content when no status"
        );
    }

    #[test]
    fn test_format_git_output_bracket_indicators() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            staged: 2,
            modified: 1,
            untracked: 3,
            ahead: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        // "main [!+?⇡]" = 4 + 1 + 6 = 11 visible chars
        assert_eq!(display_width(&output), 11, "visible width: {output:?}");
        assert!(output.contains("main"), "should contain branch");
        assert!(
            output.contains("[!+?⇡]"),
            "should contain bracketed indicators: {output:?}"
        );
        assert!(
            contains_style_sequence(&output, &[1, 31]),
            "brackets should be bold red: {output:?}"
        );
    }

    #[test]
    fn test_format_git_output_detached_clean_shows_short_oid() {
        let status = GitStatus {
            branch: None,
            head_oid: Some("abcdef0123456789abcdef0123456789abcd".to_owned()),
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        assert_eq!(display_width(&output), 14, "visible width: {output:?}");
        assert!(
            output.contains("HEAD ") && output.contains("(abcdef0)"),
            "detached label should be HEAD (short oid); zsh escapes may split segments: {output:?}"
        );
        assert!(
            contains_style_sequence(&output, &[1, 35]),
            "HEAD should use branch style bold magenta: {output:?}"
        );
        assert!(
            contains_style_sequence(&output, &[2, 32])
                || contains_style_sequence(&output, &[32, 2]),
            "(hash) should use dimmed green: {output:?}"
        );
    }

    #[test]
    fn test_format_git_output_detached_with_indicators() {
        let status = GitStatus {
            branch: None,
            head_oid: Some("deadbeef".to_owned()),
            modified: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        let clean = strip_ansi_and_zsh(&output);
        assert_eq!(
            clean, "HEAD (deadbee) [!]",
            "short oid shorter than 7 uses full hash inside parens: {output:?}"
        );
    }

    #[test]
    fn test_format_git_output_no_branch() {
        let status = GitStatus {
            branch: None,
            staged: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        // "[+]" = 3 visible chars
        assert_eq!(display_width(&output), 3, "visible width: {output:?}");
        assert!(
            output.contains("[+]"),
            "should contain bracketed staged indicator: {output:?}"
        );
        assert!(
            contains_style_sequence(&output, &[1, 31]),
            "brackets should be bold red: {output:?}"
        );
    }

    // -- Mock provider tests --

    struct MockGitProvider {
        result: Option<GitStatus>,
    }

    impl sealed::Sealed for MockGitProvider {}

    impl GitProvider for MockGitProvider {
        fn status(
            &self,
            _cwd: &Path,
            _path_env: Option<&str>,
        ) -> Result<Option<GitStatus>, GitError> {
            Ok(self.result.clone())
        }
    }

    fn make_ctx() -> RenderContext<'static> {
        RenderContext {
            cwd: Path::new("/tmp"),
            home_dir: Path::new("/Users/testuser"),
            last_exit_code: 0,
            duration_ms: None,
            keymap: "main",
            cols: 80,
        }
    }

    #[test]
    fn test_module_not_a_repo_returns_none() {
        let module = GitModule::new(MockGitProvider { result: None });
        let ctx = make_ctx();
        assert!(module.render(&ctx).is_none());
    }

    #[test]
    fn test_module_staged_changes() {
        let module = GitModule::new(MockGitProvider {
            result: Some(GitStatus {
                branch: Some("main".to_owned()),
                staged: 2,
                ..GitStatus::default()
            }),
        });
        let ctx = make_ctx();
        let output = module.render(&ctx);
        assert!(output.is_some());
        let content = output.map(|o| o.content).unwrap_or_default();
        assert!(
            content.contains("[+]"),
            "expected bracketed staged indicator in: {content}"
        );
    }

    #[test]
    fn test_module_speed_is_slow() {
        let module = GitModule::new(MockGitProvider { result: None });
        assert_eq!(module.speed(), ModuleSpeed::Slow);
    }

    // -- Integration test with real git --

    #[test]
    fn test_module_real_git_repo_with_staged_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path();

        // Initialize a git repo
        let init = Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(path)
            .output()?;
        assert!(init.status.success(), "git init failed");

        // Configure git identity (needed in CI)
        Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()?;

        // Create and stage a file
        std::fs::write(path.join("hello.txt"), "hello")?;
        let add = Command::new("git")
            .args(["add", "hello.txt"])
            .current_dir(path)
            .output()?;
        assert!(add.status.success(), "git add failed");

        // Query via CommandGitProvider
        let provider = CommandGitProvider;
        let status = provider.status(path, None)?;
        let status = status.as_ref();
        assert!(status.is_some(), "should detect git repo");
        assert!(
            status.is_some_and(|s| s.staged > 0),
            "should have staged files"
        );

        // Query via GitModule
        let module = GitModule::new(CommandGitProvider);
        let ctx = RenderContext {
            cwd: path,
            home_dir: Path::new("/nonexistent"),
            last_exit_code: 0,
            duration_ms: None,
            keymap: "main",
            cols: 80,
        };
        let output = module.render(&ctx);
        assert!(output.is_some(), "git module should produce output");

        Ok(())
    }

    #[test]
    fn test_module_not_a_git_repo() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let provider = CommandGitProvider;
        let status = provider.status(dir.path(), None)?;
        assert!(status.is_none(), "non-git dir should return None");
        Ok(())
    }

    // -- Starship-compatible indicator tests --

    #[test]
    fn test_format_conflict_uses_equals_sign() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            conflicted: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        assert!(
            output.contains("[=]"),
            "conflict should use '=' not '~': {output:?}"
        );
    }

    #[test]
    fn test_format_stash_indicator() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            stashed: 3,
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        assert!(output.contains("[$]"), "stash should show '$': {output:?}");
    }

    #[test]
    fn test_format_deleted_indicator() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            deleted: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        assert!(
            output.contains("[✘]"),
            "deleted should show '✘': {output:?}"
        );
    }

    #[test]
    fn test_format_renamed_indicator() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            renamed: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        assert!(
            output.contains("[»]"),
            "renamed should show '»': {output:?}"
        );
    }

    #[test]
    fn test_format_diverged_indicator() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            ahead: 2,
            behind: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        assert!(
            output.contains('⇕'),
            "diverged (ahead+behind) should show '⇕': {output:?}"
        );
        assert!(
            !output.contains('⇡'),
            "diverged should not show separate '⇡': {output:?}"
        );
        assert!(
            !output.contains('⇣'),
            "diverged should not show separate '⇣': {output:?}"
        );
    }

    #[test]
    fn test_format_indicator_order() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            conflicted: 1,
            stashed: 1,
            deleted: 1,
            renamed: 1,
            modified: 1,
            staged: 1,
            untracked: 1,
            ahead: 1,
            behind: 0,
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        // Strip all ANSI/zsh escapes to get visible text
        let clean = strip_ansi_and_zsh(&output);
        // Expected visible: "main [=$✘»!+?⇡]"
        assert_eq!(
            clean, "main [=$✘»!+?⇡]",
            "indicators should be in Starship order: {output:?}"
        );
    }

    #[test]
    fn test_format_git_output_uses_custom_styles_and_color_map() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            modified: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(
            &status,
            &GitStyles {
                branch: Style::new().fg(Color::Cyan),
                detached_hash: Style::new().fg(Color::Green),
                indicator: Style::new().fg(Color::Yellow),
                color_map: ColorMap {
                    cyan: 96,
                    green: 32,
                    yellow: 93,
                    ..ColorMap::default()
                },
                ..default_styles()
            },
        );
        assert!(
            output.contains("\x1b[96m"),
            "branch should use remapped cyan: {output:?}"
        );
        assert!(
            output.contains("\x1b[93m"),
            "indicators should use remapped yellow: {output:?}"
        );
    }

    /// Strip ANSI escape sequences and zsh `%{{..%}}` wrappers.
    fn strip_ansi_and_zsh(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%' && chars.peek() == Some(&'{') {
                // Skip %{...%}
                chars.next(); // consume '{'
                while let Some(inner) = chars.next() {
                    if inner == '%' && chars.peek() == Some(&'}') {
                        chars.next(); // consume '}'
                        break;
                    }
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    // -- write_state tests --

    fn state_to_string(op_state: &GitOperationState) -> String {
        let mut buf = String::new();
        write_state(&mut buf, op_state);
        buf
    }

    #[test]
    fn test_write_state_with_progress() {
        let state = GitOperationState {
            state: GitState::Rebase,
            step: Some(2),
            total: Some(5),
        };
        assert_eq!(state_to_string(&state), "(REBASING 2/5)");
    }

    #[test]
    fn test_write_state_without_progress() {
        let state = GitOperationState::without_progress(GitState::Merge);
        assert_eq!(state_to_string(&state), "(MERGING)");
    }

    #[test]
    fn test_write_state_partial_progress_shows_no_progress() {
        let state = GitOperationState {
            state: GitState::Rebase,
            step: Some(3),
            total: None,
        };
        assert_eq!(
            state_to_string(&state),
            "(REBASING)",
            "partial progress should fall back to no-progress display",
        );
    }

    // -- format_git_output with state tests --

    #[test]
    fn test_format_git_output_with_rebase_state() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            state: Some(GitOperationState {
                state: GitState::Rebase,
                step: Some(2),
                total: Some(5),
            }),
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        let clean = strip_ansi_and_zsh(&output);
        assert_eq!(clean, "main (REBASING 2/5)");
    }

    #[test]
    fn test_format_git_output_with_merge_state() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            state: Some(GitOperationState::without_progress(GitState::Merge)),
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        let clean = strip_ansi_and_zsh(&output);
        assert_eq!(clean, "main (MERGING)");
    }

    #[test]
    fn test_format_git_output_state_with_indicators() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            state: Some(GitOperationState {
                state: GitState::Rebase,
                step: Some(2),
                total: Some(5),
            }),
            modified: 1,
            staged: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        let clean = strip_ansi_and_zsh(&output);
        assert_eq!(clean, "main (REBASING 2/5) [!+]");
    }

    #[test]
    fn test_format_git_output_detached_with_state() {
        let status = GitStatus {
            branch: None,
            head_oid: Some("abcdef0123456789".to_owned()),
            state: Some(GitOperationState::without_progress(GitState::CherryPick)),
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        let clean = strip_ansi_and_zsh(&output);
        assert_eq!(clean, "HEAD (abcdef0) (CHERRY-PICKING)");
    }

    #[test]
    fn test_format_git_output_state_styled_yellow_bold() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            state: Some(GitOperationState::without_progress(GitState::Merge)),
            ..GitStatus::default()
        };
        let output = format_git_output(&status, &default_styles());
        assert!(
            contains_style_sequence(&output, &[1, 33]),
            "state should be bold yellow: {output:?}",
        );
    }

    #[test]
    fn test_parse_stash_count() {
        let output = "\
# branch.head main
# stash 5
";
        let status = parse_porcelain_v2(output);
        assert_eq!(status.stashed, 5);
    }

    #[test]
    fn test_parse_deleted_file() {
        let output = "\
# branch.head main
1 D. N... 100644 000000 000000 abc123 000000 deleted.rs
";
        let status = parse_porcelain_v2(output);
        assert_eq!(status.deleted, 1, "index delete should be tracked");
    }

    #[test]
    fn test_parse_worktree_deleted_file() {
        let output = "\
# branch.head main
1 .D N... 100644 100644 000000 abc123 def456 deleted.rs
";
        let status = parse_porcelain_v2(output);
        assert_eq!(status.deleted, 1, "worktree delete should be tracked");
    }

    #[test]
    fn test_parse_renamed_file() {
        let output = "\
# branch.head main
2 R. N... 100644 100644 100644 abc123 def456 R100 new.rs\told.rs
";
        let status = parse_porcelain_v2(output);
        assert_eq!(status.renamed, 1, "rename should be tracked");
    }
}
