//! Tests for parallel module execution and timeout behavior.
//!
//! Executable doc: `cargo test -p capsule-core -- parallel`

use std::time::{Duration, Instant};

use capsule_protocol::Message;

use super::test_support::{MockGitProvider, TestHarness, make_request, make_sleep_module};
use crate::config::{Config, TimeoutConfig};

/// Two slow modules each sleeping 200ms should complete in ~200ms (parallel),
/// not ~400ms (sequential).
#[tokio::test]
async fn test_parallel_slow_modules_execute_concurrently() -> Result<(), Box<dyn std::error::Error>>
{
    let config = Config {
        module: vec![
            make_sleep_module("mod_a", 200, "AAA"),
            make_sleep_module("mod_b", 200, "BBB"),
        ],
        timeout: TimeoutConfig {
            slow_ms: 5000,
            ..TimeoutConfig::default()
        },
        ..Config::default()
    };

    let harness = TestHarness::start_with_config(MockGitProvider::default(), config).await?;
    let cwd = harness.cwd_str().ok_or("work_dir missing")?;
    let (mut reader, mut writer) = harness.connect().await?;

    let start = Instant::now();
    writer
        .write_message(&Message::Request(make_request(cwd, 1, 120)))
        .await?;

    // Read RenderResult
    let rr = reader.read_message().await?;
    assert!(
        matches!(&rr, Some(Message::RenderResult(_))),
        "expected RenderResult: {rr:?}"
    );

    // Read Update with slow module results
    let update = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    let elapsed = start.elapsed();

    match update {
        Some(Message::Update(u)) => {
            assert!(
                u.left1.contains("AAA"),
                "module A should be present in update: {}",
                u.left1
            );
            assert!(
                u.left1.contains("BBB"),
                "module B should be present in update: {}",
                u.left1
            );
            // Parallel: ~200ms. Sequential would be ~400ms.
            // Use generous margin for CI.
            assert!(
                elapsed < Duration::from_millis(1500),
                "parallel execution should complete much faster than sequential, took {elapsed:?}"
            );
        }
        other => return Err(format!("expected Update, got {other:?}").into()),
    }

    harness.shutdown().await
}

/// A module that exceeds the timeout should be omitted from the prompt.
#[tokio::test]
async fn test_parallel_timeout_omits_slow_module() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        module: vec![
            make_sleep_module("fast_mod", 50, "FAST"),
            make_sleep_module("slow_mod", 3000, "SLOW"),
        ],
        timeout: TimeoutConfig {
            slow_ms: 500,
            ..TimeoutConfig::default()
        },
        ..Config::default()
    };

    let harness = TestHarness::start_with_config(MockGitProvider::default(), config).await?;
    let cwd = harness.cwd_str().ok_or("work_dir missing")?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request(cwd, 1, 120)))
        .await?;

    // Read RenderResult
    let rr = reader.read_message().await?;
    assert!(
        matches!(&rr, Some(Message::RenderResult(_))),
        "expected RenderResult: {rr:?}"
    );

    // Read Update
    let update = tokio::time::timeout(Duration::from_secs(3), reader.read_message()).await??;
    match update {
        Some(Message::Update(u)) => {
            assert!(
                u.left1.contains("FAST"),
                "fast module should be present: {}",
                u.left1
            );
            assert!(
                !u.left1.contains("SLOW"),
                "timed-out module should be omitted: {}",
                u.left1
            );
        }
        other => return Err(format!("expected Update, got {other:?}").into()),
    }

    harness.shutdown().await
}

/// Modules should appear in definition order regardless of completion order.
/// `mod_a` sleeps 200ms, `mod_b` sleeps 50ms — `mod_b` finishes first but
/// should appear after `mod_a` in the prompt.
#[tokio::test]
async fn test_parallel_preserves_definition_order() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        module: vec![
            make_sleep_module("mod_a", 200, "FIRST"),
            make_sleep_module("mod_b", 50, "SECOND"),
        ],
        timeout: TimeoutConfig {
            slow_ms: 5000,
            ..TimeoutConfig::default()
        },
        ..Config::default()
    };

    let harness = TestHarness::start_with_config(MockGitProvider::default(), config).await?;
    let cwd = harness.cwd_str().ok_or("work_dir missing")?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request(cwd, 1, 120)))
        .await?;

    let rr = reader.read_message().await?;
    assert!(
        matches!(&rr, Some(Message::RenderResult(_))),
        "expected RenderResult: {rr:?}"
    );

    let update = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    match update {
        Some(Message::Update(u)) => {
            let first_pos = u.left1.find("FIRST").ok_or("FIRST not found in update")?;
            let second_pos = u.left1.find("SECOND").ok_or("SECOND not found in update")?;
            assert!(
                first_pos < second_pos,
                "FIRST should appear before SECOND (definition order): {}",
                u.left1
            );
        }
        other => return Err(format!("expected Update, got {other:?}").into()),
    }

    harness.shutdown().await
}

/// When all modules complete before timeout, don't wait for the timeout.
#[tokio::test]
async fn test_parallel_early_completion_does_not_wait() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        module: vec![make_sleep_module("quick", 50, "QUICK")],
        timeout: TimeoutConfig {
            slow_ms: 10000,
            ..TimeoutConfig::default()
        },
        ..Config::default()
    };

    let harness = TestHarness::start_with_config(MockGitProvider::default(), config).await?;
    let cwd = harness.cwd_str().ok_or("work_dir missing")?;
    let (mut reader, mut writer) = harness.connect().await?;

    let start = Instant::now();
    writer
        .write_message(&Message::Request(make_request(cwd, 1, 120)))
        .await?;

    let rr = reader.read_message().await?;
    assert!(
        matches!(&rr, Some(Message::RenderResult(_))),
        "expected RenderResult: {rr:?}"
    );

    let update = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    let elapsed = start.elapsed();

    match update {
        Some(Message::Update(u)) => {
            assert!(
                u.left1.contains("QUICK"),
                "module should be present: {}",
                u.left1
            );
            // Should complete in ~50ms + overhead, not 10s
            assert!(
                elapsed < Duration::from_secs(2),
                "early completion should not wait for timeout, took {elapsed:?}"
            );
        }
        other => return Err(format!("expected Update, got {other:?}").into()),
    }

    harness.shutdown().await
}
