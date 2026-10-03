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
