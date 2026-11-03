use super::*;

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

#[tokio::test]
async fn test_find_git_dir_normal_repo() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join(".git"))?;
    let result = find_repository(dir.path(), &Runner::default(), &CancellationToken::new())
        .await?
        .map(|repo| repo.git_dir);
    assert_eq!(result, Some(dir.path().join(".git")));
    Ok(())
}

#[tokio::test]
async fn test_find_git_dir_subdirectory() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join(".git"))?;
    let sub = dir.path().join("src").join("deep");
    std::fs::create_dir_all(&sub)?;
    let result = find_repository(&sub, &Runner::default(), &CancellationToken::new())
        .await?
        .map(|repo| repo.git_dir);
    assert_eq!(result, Some(dir.path().join(".git")));
    Ok(())
}

#[tokio::test]
async fn test_find_git_dir_worktree() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let gitdir_target = dir.path().join("actual-gitdir");
    std::fs::create_dir(&gitdir_target)?;
    let worktree = dir.path().join("worktree");
    std::fs::create_dir(&worktree)?;
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}", gitdir_target.display()),
    )?;
    let result = find_repository(&worktree, &Runner::default(), &CancellationToken::new())
        .await?
        .map(|repo| repo.git_dir);
    assert_eq!(result, Some(gitdir_target));
    Ok(())
}

#[tokio::test]
async fn test_find_git_dir_worktree_relative() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let gitdir_target = dir.path().join("actual-gitdir");
    std::fs::create_dir(&gitdir_target)?;
    let worktree = dir.path().join("worktree");
    std::fs::create_dir(&worktree)?;
    std::fs::write(worktree.join(".git"), "gitdir: ../actual-gitdir\n")?;
    let result = find_repository(&worktree, &Runner::default(), &CancellationToken::new())
        .await?
        .map(|repo| repo.git_dir);
    assert!(result.is_some(), "should resolve relative gitdir pointer");
    assert!(
        result
            .as_ref()
            .is_some_and(|p| p.ends_with("actual-gitdir")),
        "resolved path should end with actual-gitdir: {result:?}",
    );
    Ok(())
}

#[tokio::test]
async fn test_find_git_dir_not_a_repo() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let result = find_repository(dir.path(), &Runner::default(), &CancellationToken::new())
        .await?
        .map(|repo| repo.git_dir);
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

#[tokio::test]
async fn test_detect_rebase_merge_with_progress() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let rebase = dir.path().join("rebase-merge");
    std::fs::create_dir(&rebase)?;
    std::fs::write(rebase.join("msgnum"), "3\n")?;
    std::fs::write(rebase.join("end"), "7\n")?;
    let result =
        detect_git_state(dir.path(), &Runner::default(), &CancellationToken::new()).await?;
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

#[tokio::test]
async fn test_detect_rebase_merge_without_progress() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("rebase-merge"))?;
    let result =
        detect_git_state(dir.path(), &Runner::default(), &CancellationToken::new()).await?;
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

#[tokio::test]
async fn test_detect_rebase_apply() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let rebase = dir.path().join("rebase-apply");
    std::fs::create_dir(&rebase)?;
    std::fs::write(rebase.join("next"), "2\n")?;
    std::fs::write(rebase.join("last"), "5\n")?;
    let result =
        detect_git_state(dir.path(), &Runner::default(), &CancellationToken::new()).await?;
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

#[tokio::test]
async fn test_detect_am() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let rebase = dir.path().join("rebase-apply");
    std::fs::create_dir(&rebase)?;
    std::fs::write(rebase.join("applying"), "")?;
    std::fs::write(rebase.join("next"), "1\n")?;
    std::fs::write(rebase.join("last"), "3\n")?;
    let result =
        detect_git_state(dir.path(), &Runner::default(), &CancellationToken::new()).await?;
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

#[tokio::test]
async fn test_detect_merge() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("MERGE_HEAD"), "abc123\n")?;
    let result =
        detect_git_state(dir.path(), &Runner::default(), &CancellationToken::new()).await?;
    assert_eq!(
        result,
        Some(GitOperationState::without_progress(GitState::Merge)),
    );
    Ok(())
}

#[tokio::test]
async fn test_detect_cherry_pick() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("CHERRY_PICK_HEAD"), "abc123\n")?;
    let result =
        detect_git_state(dir.path(), &Runner::default(), &CancellationToken::new()).await?;
    assert_eq!(
        result,
        Some(GitOperationState::without_progress(GitState::CherryPick)),
    );
    Ok(())
}

#[tokio::test]
async fn test_detect_revert() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("REVERT_HEAD"), "abc123\n")?;
    let result =
        detect_git_state(dir.path(), &Runner::default(), &CancellationToken::new()).await?;
    assert_eq!(
        result,
        Some(GitOperationState::without_progress(GitState::Revert)),
    );
    Ok(())
}

#[tokio::test]
async fn test_detect_bisect() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("BISECT_LOG"), "")?;
    let result =
        detect_git_state(dir.path(), &Runner::default(), &CancellationToken::new()).await?;
    assert_eq!(
        result,
        Some(GitOperationState::without_progress(GitState::Bisect)),
    );
    Ok(())
}

#[tokio::test]
async fn test_detect_no_state() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let result =
        detect_git_state(dir.path(), &Runner::default(), &CancellationToken::new()).await?;
    assert_eq!(result, None);
    Ok(())
}

#[tokio::test]
async fn test_detect_priority_rebase_over_merge() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("rebase-merge"))?;
    std::fs::write(dir.path().join("MERGE_HEAD"), "abc123\n")?;
    let result =
        detect_git_state(dir.path(), &Runner::default(), &CancellationToken::new()).await?;
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

// -- acquire_context tests --

/// A snapshot in `directory/repo/sub`, under a repository root, whose `PATH`
/// holds only a dummy `git` running `script`.
fn dummy_git(directory: &Path, script: &str) -> Result<Snapshot, Box<dyn std::error::Error>> {
    let bin = directory.join("bin");
    let cwd = directory.join("repo/sub");
    std::fs::create_dir(&bin)?;
    std::fs::create_dir_all(&cwd)?;
    std::fs::create_dir(directory.join("repo/.git"))?;
    crate::test_utils::write_script(&bin.join("git"), script)?;
    Ok(Snapshot {
        cwd,
        env: vec![(OsString::from("PATH"), bin.into_os_string())],
    })
}

async fn context(snapshot: &Snapshot, git_enabled: bool) -> Result<ContextInfo, AcquireError> {
    acquire_context(
        snapshot,
        &Runner::default(),
        &CancellationToken::new(),
        git_enabled,
    )
    .await
}

/// A dummy `git` script that records its arguments in `record`.
fn recording(record: &Path) -> String {
    format!("printf '%s\\n' \"$@\" > '{}'", record.display())
}

/// Covers: T04
#[tokio::test]
async fn git_runs_the_fixed_status_argv() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let record = directory.path().join("argv");
    let snapshot = dummy_git(directory.path(), &recording(&record))?;
    assert!(context(&snapshot, true).await?.git.is_some());
    assert_eq!(
        std::fs::read_to_string(record)?,
        "--no-optional-locks\nstatus\n--porcelain=v2\n--branch\n--show-stash\n"
    );
    Ok(())
}

/// Covers: T04
#[tokio::test]
async fn disabled_git_runs_no_git_process() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let record = directory.path().join("argv");
    let snapshot = dummy_git(directory.path(), &recording(&record))?;
    assert!(context(&snapshot, false).await?.git.is_none());
    assert!(!record.exists());
    Ok(())
}

/// Covers: T04
#[tokio::test]
async fn failed_or_missing_git_shows_no_git_and_keeps_the_directory()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    // Output before a failing exit must not be displayed either.
    let snapshot = dummy_git(directory.path(), "printf '# branch.head main\\n'\nexit 1")?;
    let context_info = context(&snapshot, true).await?;
    assert!(context_info.git.is_none());
    assert_eq!(context_info.directory, "sub");

    let missing = Snapshot {
        env: vec![(
            OsString::from("PATH"),
            directory.path().join("empty").into_os_string(),
        )],
        ..snapshot
    };
    let context_info = context(&missing, true).await?;
    assert!(context_info.git.is_none());
    assert_eq!(context_info.directory, "sub");
    Ok(())
}

/// Covers: T04
#[tokio::test]
async fn repository_search_stops_after_the_depth_limit() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    std::fs::create_dir(directory.path().join(".git"))?;
    // `ancestors()` yields the cwd first, so the deepest found root is
    // `MAX_REPOSITORY_DEPTH - 1` levels up.
    let nested = |levels: usize| directory.path().join("d/".repeat(levels));
    let deepest = nested(MAX_REPOSITORY_DEPTH);
    std::fs::create_dir_all(&deepest)?;
    let runner = Runner::default();
    let cancel = CancellationToken::new();
    let found = find_repository(&nested(MAX_REPOSITORY_DEPTH - 1), &runner, &cancel).await?;
    assert_eq!(
        found.map(|repo| repo.root),
        Some(directory.path().to_owned())
    );
    assert!(find_repository(&deepest, &runner, &cancel).await?.is_none());
    Ok(())
}
