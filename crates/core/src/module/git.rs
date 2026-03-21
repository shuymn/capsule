//! Git module — displays git branch and working tree status.

use std::{path::Path, process::Command};

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};
use crate::render::style::{Color, Style};

/// Errors that can occur when querying git.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// Failed to execute the git command.
    #[error("failed to execute git command")]
    Command(#[source] std::io::Error),
}

/// Git repository status information.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// Current branch name, or `None` if detached.
    pub branch: Option<String>,
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
}

/// Provides git repository information.
pub trait GitProvider {
    /// Query the git status of the repository at `cwd`.
    ///
    /// Returns `Ok(None)` if `cwd` is not inside a git repository.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] if the git command cannot be executed.
    fn status(&self, cwd: &Path) -> Result<Option<GitStatus>, GitError>;
}

/// [`GitProvider`] that shells out to the `git` command.
#[derive(Debug, Clone)]
#[allow(clippy::module_name_repetitions)]
pub struct CommandGitProvider;

impl GitProvider for CommandGitProvider {
    fn status(&self, cwd: &Path) -> Result<Option<GitStatus>, GitError> {
        let output = Command::new("git")
            .args(["status", "--porcelain=v2", "--branch", "--show-stash"])
            .current_dir(cwd)
            .stderr(std::process::Stdio::null())
            .output()
            .map_err(GitError::Command)?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Some(parse_porcelain_v2(&stdout)))
    }
}

/// Displays git branch and working tree status.
///
/// Returns `None` when not inside a git repository.
#[derive(Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct GitModule<G> {
    provider: G,
}

impl<G> GitModule<G> {
    /// Creates a new `GitModule` with the given provider.
    pub const fn new(provider: G) -> Self {
        Self { provider }
    }
}

impl<G: GitProvider> GitModule<G> {
    /// Renders git status for the given working directory.
    ///
    /// This is the core implementation used by both [`Module::render`] and
    /// the daemon's slow-module path (which has no full [`RenderContext`]).
    pub fn render_for_cwd(&self, cwd: &Path) -> Option<ModuleOutput> {
        let status = match self.provider.status(cwd) {
            Ok(Some(s)) => s,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(error = %e, cwd = %cwd.display(), "git status failed");
                return None;
            }
        };
        let content = format_git_output(&status);
        if content.is_empty() {
            return None;
        }
        Some(ModuleOutput { content })
    }
}

impl<G: GitProvider> Module for GitModule<G> {
    fn name(&self) -> &'static str {
        "git"
    }

    fn speed(&self) -> ModuleSpeed {
        ModuleSpeed::Slow
    }

    fn render(&self, ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        self.render_for_cwd(ctx.cwd)
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn parse_porcelain_v2(output: &str) -> GitStatus {
    let mut status = GitStatus::default();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
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

fn format_git_output(status: &GitStatus) -> String {
    let mut out = String::with_capacity(64);

    if let Some(ref branch) = status.branch {
        let style = Style::new().fg(Color::Magenta).bold();
        out.push_str(&style.paint(branch));
    }

    // Indicator order follows Starship defaults: = $ ✘ » ! + ? ⇕/⇡⇣
    let mut indicators = String::new();
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
    } else {
        if status.ahead > 0 {
            indicators.push('⇡');
        }
        if status.behind > 0 {
            indicators.push('⇣');
        }
    }

    if !indicators.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        let bracket_style = Style::new().fg(Color::Red).bold();
        indicators.insert(0, '[');
        indicators.push(']');
        out.push_str(&bracket_style.paint(&indicators));
    }

    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::render::layout::display_width;

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
    }

    #[test]
    fn test_parse_porcelain_v2_detached_head() {
        let output = "# branch.oid abc123\n# branch.head (detached)\n";
        let status = parse_porcelain_v2(output);
        assert_eq!(status.branch, None);
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
        let output = format_git_output(&status);
        assert_eq!(display_width(&output), 4, "visible width: {output:?}");
        assert!(output.contains("main"), "should contain branch name");
        assert!(
            output.contains("\x1b[1;35m"),
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
        let output = format_git_output(&status);
        // "main [!+?⇡]" = 4 + 1 + 6 = 11 visible chars
        assert_eq!(display_width(&output), 11, "visible width: {output:?}");
        assert!(output.contains("main"), "should contain branch");
        assert!(
            output.contains("[!+?⇡]"),
            "should contain bracketed indicators: {output:?}"
        );
        assert!(
            output.contains("\x1b[1;31m"),
            "brackets should be bold red: {output:?}"
        );
    }

    #[test]
    fn test_format_git_output_no_branch() {
        let status = GitStatus {
            branch: None,
            staged: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status);
        // "[+]" = 3 visible chars
        assert_eq!(display_width(&output), 3, "visible width: {output:?}");
        assert!(
            output.contains("[+]"),
            "should contain bracketed staged indicator: {output:?}"
        );
        assert!(
            output.contains("\x1b[1;31m"),
            "brackets should be bold red: {output:?}"
        );
    }

    // -- Mock provider tests --

    struct MockGitProvider {
        result: Option<GitStatus>,
    }

    impl GitProvider for MockGitProvider {
        fn status(&self, _cwd: &Path) -> Result<Option<GitStatus>, GitError> {
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
        let status = provider.status(path)?;
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
        let status = provider.status(dir.path())?;
        assert!(status.is_none(), "non-git dir should return None");
        Ok(())
    }

    // -- Starship-compatible indicator tests (Theme 10) --

    #[test]
    fn test_format_conflict_uses_equals_sign() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            conflicted: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status);
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
        let output = format_git_output(&status);
        assert!(output.contains("[$]"), "stash should show '$': {output:?}");
    }

    #[test]
    fn test_format_deleted_indicator() {
        let status = GitStatus {
            branch: Some("main".to_owned()),
            deleted: 1,
            ..GitStatus::default()
        };
        let output = format_git_output(&status);
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
        let output = format_git_output(&status);
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
        let output = format_git_output(&status);
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
        };
        let output = format_git_output(&status);
        // Strip all ANSI/zsh escapes to get visible text
        let clean = strip_ansi_and_zsh(&output);
        // Expected visible: "main [=$✘»!+?⇡]"
        assert_eq!(
            clean, "main [=$✘»!+?⇡]",
            "indicators should be in Starship order: {output:?}"
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
