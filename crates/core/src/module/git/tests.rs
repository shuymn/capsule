use std::path::{Path, PathBuf};

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
    let output = "# branch.head main\n2 R. N... 000000 000000 abc123 def456 R100 new.rs\told.rs\n";
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
    let output = format_git_output(&status, &GitStyles::default());
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
    let output = format_git_output(&status, &GitStyles::default());
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
    let output = format_git_output(&status, &GitStyles::default());
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
        contains_style_sequence(&output, &[2, 32]) || contains_style_sequence(&output, &[32, 2]),
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
    let output = format_git_output(&status, &GitStyles::default());
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
    let output = format_git_output(&status, &GitStyles::default());
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
    fn status(&self, _cwd: &Path, _path_env: Option<&str>) -> Result<Option<GitStatus>, GitError> {
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

const GIT_ENV_REGRESSION_CHILD: &str = "CAPSULE_GIT_ENV_REGRESSION_CHILD";
const GIT_ENV_REGRESSION_CWD: &str = "CAPSULE_GIT_ENV_REGRESSION_CWD";
const GIT_ENV_REGRESSION_MARKER: &str = "capsule_git_env_regression_child_success";

fn test_git_command() -> Command {
    let mut command = Command::new("git");
    clear_git_local_env!(&mut command);
    command
}

fn staged_git_repo() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path();

    let init = test_git_command()
        .args(["init", "-b", "main"])
        .current_dir(path)
        .output()?;
    assert!(init.status.success(), "git init failed");

    test_git_command()
        .args(["config", "user.name", "test"])
        .current_dir(path)
        .output()?;
    test_git_command()
        .args(["config", "user.email", "test@test.com"])
        .current_dir(path)
        .output()?;

    std::fs::write(path.join("hello.txt"), "hello")?;
    let add = test_git_command()
        .args(["add", "hello.txt"])
        .current_dir(path)
        .output()?;
    assert!(add.status.success(), "git add failed");

    Ok(dir)
}

async fn run_git_env_regression_child_if_requested() -> Result<bool, Box<dyn std::error::Error>> {
    if std::env::var(GIT_ENV_REGRESSION_CHILD).ok().as_deref() != Some("1") {
        return Ok(false);
    }

    let cwd = PathBuf::from(std::env::var(GIT_ENV_REGRESSION_CWD)?);
    let sync_status = CommandGitProvider.status(&cwd, None)?;
    assert!(
        sync_status.is_none(),
        "sync git status should use cwd, not inherited Git env"
    );

    let async_status = CommandGitProvider.status_async(cwd, None).await?;
    assert!(
        async_status.is_none(),
        "async git status should use cwd, not inherited Git env"
    );

    println!("{GIT_ENV_REGRESSION_MARKER}");
    Ok(true)
}

fn run_git_env_regression_child(test_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let polluted_repo = staged_git_repo()?;
    let non_repo = tempfile::tempdir()?;
    let output = Command::new(std::env::current_exe()?)
        .args(["--exact", test_name, "--nocapture"])
        .env(GIT_ENV_REGRESSION_CHILD, "1")
        .env(GIT_ENV_REGRESSION_CWD, non_repo.path())
        .env("GIT_DIR", polluted_repo.path().join(".git"))
        .env("GIT_WORK_TREE", polluted_repo.path())
        .output()?;

    let child_stdout = String::from_utf8_lossy(&output.stdout);
    let child_stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "child regression test failed\nstdout:\n{child_stdout}\nstderr:\n{child_stderr}"
    );
    assert!(
        child_stdout.contains(GIT_ENV_REGRESSION_MARKER),
        "child regression test did not run\nstdout:\n{child_stdout}\nstderr:\n{child_stderr}"
    );

    Ok(())
}

#[tokio::test]
async fn test_command_git_provider_ignores_git_local_env() -> Result<(), Box<dyn std::error::Error>>
{
    if run_git_env_regression_child_if_requested().await? {
        return Ok(());
    }

    run_git_env_regression_child(
        "module::git::tests::test_command_git_provider_ignores_git_local_env",
    )
}

#[test]
fn test_module_real_git_repo_with_staged_file() -> Result<(), Box<dyn std::error::Error>> {
    let dir = staged_git_repo()?;
    let path = dir.path();

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

#[tokio::test]
async fn test_command_git_provider_status_async_real_repo_with_staged_file()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = staged_git_repo()?;
    let path = dir.path();

    let status = CommandGitProvider
        .status_async(path.to_path_buf(), None)
        .await?;
    assert!(status.is_some(), "should detect git repo");
    assert!(
        status.as_ref().is_some_and(|status| status.staged > 0),
        "should have staged files"
    );
    Ok(())
}

#[tokio::test]
async fn test_command_git_provider_status_async_not_a_git_repo()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let status = CommandGitProvider
        .status_async(dir.path().to_path_buf(), None)
        .await?;
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
    let output = format_git_output(&status, &GitStyles::default());
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
    let output = format_git_output(&status, &GitStyles::default());
    assert!(output.contains("[$]"), "stash should show '$': {output:?}");
}

#[test]
fn test_format_deleted_indicator() {
    let status = GitStatus {
        branch: Some("main".to_owned()),
        deleted: 1,
        ..GitStatus::default()
    };
    let output = format_git_output(&status, &GitStyles::default());
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
    let output = format_git_output(&status, &GitStyles::default());
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
    let output = format_git_output(&status, &GitStyles::default());
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
    let output = format_git_output(&status, &GitStyles::default());
    // Strip all ANSI/zsh escapes to get visible text
    let clean = strip_ansi_and_zsh(&output);
    // Expected visible: "main [=$✘»!+?⇡]"
    assert_eq!(
        clean, "main [=$✘»!+?⇡]",
        "indicators should be in Starship order: {output:?}"
    );
}

#[test]
fn test_format_git_output_custom_styles() {
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
            ..GitStyles::default()
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
fn test_write_state_progress() {
    let state = GitOperationState {
        state: GitState::Rebase,
        step: Some(2),
        total: Some(5),
    };
    assert_eq!(state_to_string(&state), "(REBASING 2/5)");
}

#[test]
fn test_write_state_no_progress() {
    let state = GitOperationState::without_progress(GitState::Merge);
    assert_eq!(state_to_string(&state), "(MERGING)");
}

#[test]
fn test_write_state_partial_progress_falls_back() {
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
fn test_format_git_output_rebase_state() {
    let status = GitStatus {
        branch: Some("main".to_owned()),
        state: Some(GitOperationState {
            state: GitState::Rebase,
            step: Some(2),
            total: Some(5),
        }),
        ..GitStatus::default()
    };
    let output = format_git_output(&status, &GitStyles::default());
    let clean = strip_ansi_and_zsh(&output);
    assert_eq!(clean, "main (REBASING 2/5)");
}

#[test]
fn test_format_git_output_merge_state() {
    let status = GitStatus {
        branch: Some("main".to_owned()),
        state: Some(GitOperationState::without_progress(GitState::Merge)),
        ..GitStatus::default()
    };
    let output = format_git_output(&status, &GitStyles::default());
    let clean = strip_ansi_and_zsh(&output);
    assert_eq!(clean, "main (MERGING)");
}

#[test]
fn test_format_git_output_state_indicators() {
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
    let output = format_git_output(&status, &GitStyles::default());
    let clean = strip_ansi_and_zsh(&output);
    assert_eq!(clean, "main (REBASING 2/5) [!+]");
}

#[test]
fn test_format_git_output_detached_state() {
    let status = GitStatus {
        branch: None,
        head_oid: Some("abcdef0123456789".to_owned()),
        state: Some(GitOperationState::without_progress(GitState::CherryPick)),
        ..GitStatus::default()
    };
    let output = format_git_output(&status, &GitStyles::default());
    let clean = strip_ansi_and_zsh(&output);
    assert_eq!(clean, "HEAD (abcdef0) (CHERRY-PICKING)");
}

#[test]
fn test_format_git_output_state_style() {
    let status = GitStatus {
        branch: Some("main".to_owned()),
        state: Some(GitOperationState::without_progress(GitState::Merge)),
        ..GitStatus::default()
    };
    let output = format_git_output(&status, &GitStyles::default());
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
