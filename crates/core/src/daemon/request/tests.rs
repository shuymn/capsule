use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use capsule_protocol::{
    BuildId, Message, MessageReader, MessageWriter, PromptGeneration, Request, SessionId,
};
use tokio::{
    net::unix::{OwnedReadHalf, OwnedWriteHalf},
    time::sleep,
};

use super::super::test_support::{
    MockGitProvider, TestHarness, make_request, make_sleep_module, test_sid,
};
use crate::{
    config::{
        CacheConfig, Config, ModuleDef, ModuleSlot, ModuleWhen, SlowCacheMode, SourceDef,
        StyleConfig, TimeoutConfig,
    },
    module::GitStatus,
};

const HOT_RELOAD_WAIT: Duration = Duration::from_millis(20);

fn write_config(path: &Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

async fn rewrite_config(path: &Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    let before = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);

    loop {
        sleep(HOT_RELOAD_WAIT).await;
        write_config(path, content)?;
        let after = std::fs::metadata(path)?.modified()?;
        if before != Some(after) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("config mtime did not change after rewrite".into());
        }
    }
}

fn character_config(glyph: &str) -> String {
    format!("[character]\nglyph = \"{glyph}\"\n")
}

fn count_git_calls() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

fn other_sid() -> SessionId {
    SessionId::from_bytes([0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88])
}

fn make_request_with_sid(cwd: &str, session_id: SessionId, generation: u64, cols: u16) -> Request {
    Request {
        session_id,
        generation: PromptGeneration::new(generation),
        cwd: cwd.to_owned(),
        cols,
        last_exit_code: 0,
        duration_ms: None,
        keymap: "main".to_owned(),
        env_vars: vec![],
    }
}

async fn request_left2(
    reader: &mut MessageReader<OwnedReadHalf>,
    writer: &mut MessageWriter<OwnedWriteHalf>,
    generation: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    writer
        .write_message(&Message::Request(make_request("/tmp", generation, 80)))
        .await?;

    match reader.read_message().await? {
        Some(Message::RenderResult(rr)) => Ok(rr.left2),
        other => Err(format!("expected RenderResult, got {other:?}").into()),
    }
}

async fn request_left1(
    reader: &mut MessageReader<OwnedReadHalf>,
    writer: &mut MessageWriter<OwnedWriteHalf>,
    generation: u64,
    env_vars: Vec<(String, String)>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut req = make_request("/tmp", generation, 80);
    req.env_vars = env_vars;
    writer.write_message(&Message::Request(req)).await?;

    match reader.read_message().await? {
        Some(Message::RenderResult(rr)) => Ok(rr.left1),
        other => Err(format!("expected RenderResult, got {other:?}").into()),
    }
}

#[tokio::test]
async fn test_daemon_render_result() -> Result<(), Box<dyn std::error::Error>> {
    let harness = TestHarness::start(MockGitProvider::default()).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    let req = make_request("/tmp", 1, 80);
    writer.write_message(&Message::Request(req)).await?;

    let resp = reader.read_message().await?;
    match resp {
        Some(Message::RenderResult(rr)) => {
            assert_eq!(rr.session_id, test_sid());
            assert_eq!(rr.generation, PromptGeneration::new(1));
            assert!(
                rr.left1.contains("/tmp"),
                "left1 should contain directory: {}",
                rr.left1
            );
            assert!(!rr.left2.is_empty(), "left2 should contain character");
        }
        other => return Err(format!("expected RenderResult, got {other:?}").into()),
    }

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_slow_module_update() -> Result<(), Box<dyn std::error::Error>> {
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            staged: 2,
            ..GitStatus::default()
        }),
        ..MockGitProvider::default()
    };
    let harness = TestHarness::start(provider).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    let req = make_request("/tmp", 1, 80);
    writer.write_message(&Message::Request(req)).await?;

    let resp = reader.read_message().await?;
    assert!(
        matches!(&resp, Some(Message::RenderResult(_))),
        "expected RenderResult: {resp:?}"
    );

    let update = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    match update {
        Some(Message::Update(u)) => {
            assert_eq!(u.session_id, test_sid());
            assert_eq!(u.generation, PromptGeneration::new(1));
            assert!(
                u.left1.contains("main"),
                "update left1 should contain branch: {}",
                u.left1
            );
        }
        other => return Err(format!("expected Update, got {other:?}").into()),
    }

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_discards_stale_generation() -> Result<(), Box<dyn std::error::Error>> {
    let harness = TestHarness::start(MockGitProvider::default()).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request("/tmp", 5, 80)))
        .await?;
    let r1 = reader.read_message().await?;
    match &r1 {
        Some(Message::RenderResult(rr)) => {
            assert_eq!(rr.generation, PromptGeneration::new(5));
        }
        other => return Err(format!("expected RenderResult(gen=5), got {other:?}").into()),
    }

    writer
        .write_message(&Message::Request(make_request("/tmp", 3, 80)))
        .await?;
    writer
        .write_message(&Message::Request(make_request("/tmp", 6, 80)))
        .await?;

    let r2 = reader.read_message().await?;
    match &r2 {
        Some(Message::RenderResult(rr)) => {
            assert_eq!(rr.generation, PromptGeneration::new(6));
        }
        other => return Err(format!("expected RenderResult(gen=6), got {other:?}").into()),
    }

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_uses_slow_cache() -> Result<(), Box<dyn std::error::Error>> {
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        ..MockGitProvider::default()
    };
    let harness = TestHarness::start(provider).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request("/tmp", 1, 80)))
        .await?;

    let r1 = reader.read_message().await?;
    assert!(
        matches!(&r1, Some(Message::RenderResult(_))),
        "expected RenderResult: {r1:?}"
    );

    let u1 = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    assert!(
        matches!(&u1, Some(Message::Update(_))),
        "expected Update: {u1:?}"
    );

    writer
        .write_message(&Message::Request(make_request("/tmp", 2, 80)))
        .await?;

    let r2 = reader.read_message().await?;
    match &r2 {
        Some(Message::RenderResult(rr)) => {
            assert!(
                rr.left1.contains("main"),
                "cached response should contain git branch: {}",
                rr.left1
            );
        }
        other => {
            return Err(format!("expected RenderResult with cache hit, got {other:?}").into());
        }
    }

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_git_cache_hit_revalidates() -> Result<(), Box<dyn std::error::Error>> {
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        call_count: Some(Arc::clone(&call_count)),
        ..MockGitProvider::default()
    };
    let harness = TestHarness::start(provider).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request("/tmp", 1, 80)))
        .await?;
    let _ = reader.read_message().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    writer
        .write_message(&Message::Request(make_request("/tmp", 2, 80)))
        .await?;
    let second = reader.read_message().await?;
    match &second {
        Some(Message::RenderResult(rr)) => {
            assert!(
                rr.left1.contains("main"),
                "cache hit should render slow output"
            );
        }
        other => return Err(format!("expected RenderResult, got {other:?}").into()),
    }

    let update = tokio::time::timeout(Duration::from_millis(200), reader.read_message()).await;
    assert!(
        update.is_err(),
        "unchanged git result should not trigger Update"
    );
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "cache hit should still revalidate git in background"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_coalesces_cache_hit_revalidation() -> Result<(), Box<dyn std::error::Error>> {
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        delay: Duration::from_millis(150),
        call_count: Some(Arc::clone(&call_count)),
    };
    let harness = TestHarness::start(provider).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request("/tmp", 1, 80)))
        .await?;
    let _ = reader.read_message().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    writer
        .write_message(&Message::Request(make_request("/tmp", 2, 80)))
        .await?;
    writer
        .write_message(&Message::Request(make_request("/tmp", 3, 80)))
        .await?;

    for expected_generation in [2, 3] {
        match reader.read_message().await? {
            Some(Message::RenderResult(rr)) => {
                assert_eq!(rr.generation, PromptGeneration::new(expected_generation));
                assert!(
                    rr.left1.contains("main"),
                    "cache-hit RenderResult should include slow output: {}",
                    rr.left1
                );
            }
            other => return Err(format!("expected RenderResult, got {other:?}").into()),
        }
    }

    let update = tokio::time::timeout(Duration::from_millis(350), reader.read_message()).await;
    assert!(
        update.is_err(),
        "unchanged revalidation should not send an Update"
    );
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "concurrent cache-hit revalidations should share one slow compute"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_skips_git_when_disabled() -> Result<(), Box<dyn std::error::Error>> {
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        call_count: Some(Arc::clone(&call_count)),
        ..MockGitProvider::default()
    };
    let config = Config {
        git: crate::config::GitConfig {
            disabled: true,
            ..crate::config::GitConfig::default()
        },
        cache: CacheConfig {
            slow: SlowCacheMode::Off,
        },
        ..Config::default()
    };
    let harness = TestHarness::start_with_config(provider, config).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request("/tmp", 1, 80)))
        .await?;

    let resp = reader.read_message().await?;
    assert!(
        matches!(&resp, Some(Message::RenderResult(_))),
        "expected RenderResult: {resp:?}"
    );

    // Caching is off so slow compute runs inline; no Update should arrive.
    let update = tokio::time::timeout(Duration::from_millis(200), reader.read_message()).await;
    assert!(update.is_err(), "no Update expected when git is disabled");

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        0,
        "git provider should not be called when disabled"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_coalesces_slow_recompute() -> Result<(), Box<dyn std::error::Error>> {
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        delay: Duration::from_millis(150),
        call_count: Some(Arc::clone(&call_count)),
    };
    let harness = TestHarness::start(provider).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request("/tmp", 1, 80)))
        .await?;
    writer
        .write_message(&Message::Request(make_request("/tmp", 2, 80)))
        .await?;

    let mut saw_gen1 = false;
    let mut saw_gen2 = false;
    while !saw_gen1 || !saw_gen2 {
        match reader.read_message().await? {
            Some(Message::RenderResult(rr)) => {
                saw_gen1 |= rr.generation == PromptGeneration::new(1);
                saw_gen2 |= rr.generation == PromptGeneration::new(2);
            }
            other => {
                return Err(format!("expected RenderResult while draining, got {other:?}").into());
            }
        }
    }

    match tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await?? {
        Some(Message::Update(update)) => {
            assert_eq!(update.generation, PromptGeneration::new(2));
            assert!(update.left1.contains("main"));
        }
        other => return Err(format!("expected Update for generation 2, got {other:?}").into()),
    }

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "same cache key should share one slow recompute"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_hot_reload_invalidates_slow_cache() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let config_path = dir.path().join("config.toml");
    write_config(&config_path, "[git.indicator_style]\nfg = \"red\"\n")?;

    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            staged: 1,
            ..GitStatus::default()
        }),
        delay: Duration::from_millis(120),
        call_count: Some(Arc::clone(&call_count)),
    };
    let harness = TestHarness::start_with_config_path(
        provider,
        crate::config::load_config(&config_path),
        config_path.clone(),
    )
    .await?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request("/tmp", 1, 80)))
        .await?;
    let _ = reader.read_message().await?;

    rewrite_config(&config_path, "[git.indicator_style]\nfg = \"green\"\n").await?;
    sleep(Duration::from_millis(160)).await;
    let generation1 = PromptGeneration::new(1);
    let generation2 = PromptGeneration::new(2);
    if let Ok(Ok(Some(Message::Update(update)))) =
        tokio::time::timeout(Duration::from_millis(50), reader.read_message()).await
    {
        assert_eq!(update.generation, generation1);
    }

    writer
        .write_message(&Message::Request(make_request("/tmp", 2, 80)))
        .await?;
    loop {
        match reader.read_message().await? {
            Some(Message::RenderResult(rr)) => {
                assert_eq!(rr.generation, generation2);
                assert!(
                    !rr.left1.contains("main"),
                    "new config generation should not reuse stale slow cache: {}",
                    rr.left1
                );
                break;
            }
            Some(Message::Update(update)) => {
                assert_eq!(
                    update.generation, generation1,
                    "only the prior generation may arrive before RenderResult(gen=2)"
                );
            }
            other => return Err(format!("expected RenderResult, got {other:?}").into()),
        }
    }

    loop {
        match tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await?? {
            Some(Message::Update(update)) => {
                if update.generation == generation1 {
                    continue;
                }
                assert_eq!(update.generation, generation2);
                assert!(update.left1.contains("main"));
                assert!(
                    update.left1.contains("\x1b[32m"),
                    "updated prompt should use reloaded git style: {}",
                    update.left1
                );
                break;
            }
            other => return Err(format!("expected Update, got {other:?}").into()),
        }
    }

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "reloaded config should force a fresh slow recompute"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_env_dep_cache_hit() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        module: vec![ModuleDef {
            name: "env-sensitive".to_owned(),
            when: ModuleWhen {
                files: vec![],
                env: vec!["CAPSULE_PROFILE".to_owned()],
            },
            source: vec![SourceDef {
                name: "value".to_owned(),
                env: None,
                file: None,
                command: Some(vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    "echo dynamic".to_owned(),
                ]),
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: Some("via".to_owned()),
            arbitration: None,
            slot: ModuleSlot::default(),
        }],
        ..Config::default()
    };
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        call_count: Some(Arc::clone(&call_count)),
        ..MockGitProvider::default()
    };
    let harness = TestHarness::start_with_config(provider, config).await?;
    let cwd = harness.cwd_str().ok_or("missing work dir")?.to_owned();
    let (mut reader, mut writer) = harness.connect().await?;

    let mut first = make_request(&cwd, 1, 80);
    first.env_vars = vec![("CAPSULE_PROFILE".to_owned(), "dev".to_owned())];
    writer.write_message(&Message::Request(first)).await?;
    let _ = reader.read_message().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    let mut second = make_request(&cwd, 2, 80);
    second.env_vars = vec![("CAPSULE_PROFILE".to_owned(), "dev".to_owned())];
    writer.write_message(&Message::Request(second)).await?;
    let rr = reader.read_message().await?;
    match &rr {
        Some(Message::RenderResult(rr)) => {
            assert!(
                rr.left1.contains("main"),
                "cache hit should include slow output: {}",
                rr.left1
            );
        }
        other => {
            return Err(format!("expected RenderResult with cache hit, got {other:?}").into());
        }
    }
    let update = tokio::time::timeout(Duration::from_millis(200), reader.read_message()).await;
    assert!(
        update.is_err(),
        "same env value should produce cache hit with no Update"
    );
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "cache hit should still revalidate git in background"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_hello_ack() -> Result<(), Box<dyn std::error::Error>> {
    let build_id = BuildId::new("12345:1700000000000000000".to_owned());
    let harness =
        TestHarness::start_with_build_id(MockGitProvider::default(), Some(build_id.clone()))
            .await?;
    let (mut reader, mut writer) = harness.connect().await?;

    let hello = capsule_protocol::Hello {
        build_id: Some(BuildId::new("other-build-id".to_owned())),
    };
    writer.write_message(&Message::Hello(hello)).await?;

    let resp = reader.read_message().await?;
    match resp {
        Some(Message::HelloAck(ack)) => {
            assert_eq!(ack.build_id, Some(build_id));
        }
        other => return Err(format!("expected HelloAck, got {other:?}").into()),
    }

    harness.shutdown().await
}

#[tokio::test]
async fn test_fast_env_module_no_blocking_pool() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        module: vec![ModuleDef {
            name: "profile".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                name: "value".to_owned(),
                env: Some("CAPSULE_PROFILE".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
            slot: ModuleSlot::default(),
        }],
        timeout: TimeoutConfig {
            fast_ms: 0,
            ..TimeoutConfig::default()
        },
        ..Config::default()
    };
    let harness = TestHarness::start_with_config(MockGitProvider::default(), config).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    let left1 = request_left1(
        &mut reader,
        &mut writer,
        1,
        vec![("CAPSULE_PROFILE".to_owned(), "dev".to_owned())],
    )
    .await?;
    assert!(
        left1.contains("dev"),
        "env-only fast module should appear in RenderResult: {left1}"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_hot_reload_uses_updated_config() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let config_path = dir.path().join("config.toml");
    write_config(&config_path, &character_config("$"))?;

    let harness = TestHarness::start_with_config_path(
        MockGitProvider::default(),
        crate::config::load_config(&config_path),
        config_path.clone(),
    )
    .await?;
    let (mut reader, mut writer) = harness.connect().await?;

    let first = request_left2(&mut reader, &mut writer, 1).await?;
    assert!(
        first.contains('$'),
        "left2 should use initial glyph: {first}"
    );

    rewrite_config(&config_path, &character_config(">")).await?;

    let second = request_left2(&mut reader, &mut writer, 2).await?;
    assert!(
        second.contains('>'),
        "left2 should use reloaded glyph: {second}"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_hot_reload_keeps_previous_config() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let config_path = dir.path().join("config.toml");
    write_config(&config_path, &character_config("$"))?;

    let harness = TestHarness::start_with_config_path(
        MockGitProvider::default(),
        crate::config::load_config(&config_path),
        config_path.clone(),
    )
    .await?;
    let (mut reader, mut writer) = harness.connect().await?;

    let first = request_left2(&mut reader, &mut writer, 1).await?;
    assert!(
        first.contains('$'),
        "left2 should use initial glyph: {first}"
    );

    rewrite_config(&config_path, "[character]\nglyph = [\n").await?;

    let second = request_left2(&mut reader, &mut writer, 2).await?;
    assert!(
        second.contains('$'),
        "parse error should keep previous glyph: {second}"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_hot_reload_loads_new_config() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let config_path = dir.path().join("config.toml");

    let harness = TestHarness::start_with_config_path(
        MockGitProvider::default(),
        crate::config::Config::default(),
        config_path.clone(),
    )
    .await?;
    let (mut reader, mut writer) = harness.connect().await?;

    let first = request_left2(&mut reader, &mut writer, 1).await?;
    assert!(
        first.contains('\u{276f}'),
        "left2 should use default glyph before config exists: {first}"
    );

    rewrite_config(&config_path, &character_config(">")).await?;

    let second = request_left2(&mut reader, &mut writer, 2).await?;
    assert!(
        second.contains('>'),
        "created config should be loaded: {second}"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_empty_cwd_does_not_panic() -> Result<(), Box<dyn std::error::Error>> {
    let harness = TestHarness::start(MockGitProvider::default()).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    let req = make_request("", 1, 80);
    writer.write_message(&Message::Request(req)).await?;

    let resp = reader.read_message().await?;
    match resp {
        Some(Message::RenderResult(rr)) => {
            assert_eq!(rr.session_id, test_sid());
            assert_eq!(rr.generation, PromptGeneration::new(1));
        }
        other => return Err(format!("expected RenderResult, got {other:?}").into()),
    }

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_path_traversal_safe() -> Result<(), Box<dyn std::error::Error>> {
    let harness = TestHarness::start(MockGitProvider::default()).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    let req = make_request("/../../../etc/passwd", 1, 80);
    writer.write_message(&Message::Request(req)).await?;

    let resp = reader.read_message().await?;
    match resp {
        Some(Message::RenderResult(rr)) => {
            assert_eq!(rr.generation, PromptGeneration::new(1));
        }
        other => return Err(format!("expected RenderResult, got {other:?}").into()),
    }

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_cwds_isolate_slow_computes() -> Result<(), Box<dyn std::error::Error>> {
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        delay: Duration::from_millis(100),
        call_count: Some(Arc::clone(&call_count)),
    };
    let harness = TestHarness::start(provider).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request("/tmp/a", 1, 80)))
        .await?;
    writer
        .write_message(&Message::Request(make_request("/tmp/b", 2, 80)))
        .await?;

    let mut render_count = 0;
    while render_count < 2 {
        match reader.read_message().await? {
            Some(Message::RenderResult(_)) => render_count += 1,
            other => {
                return Err(format!("expected RenderResult while draining, got {other:?}").into());
            }
        }
    }

    match tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await?? {
        Some(Message::Update(u)) => {
            assert_eq!(
                u.generation,
                PromptGeneration::new(2),
                "only latest generation should get Update"
            );
        }
        other => return Err(format!("expected Update for gen=2, got {other:?}").into()),
    }

    let extra = tokio::time::timeout(Duration::from_millis(300), reader.read_message()).await;
    assert!(extra.is_err(), "stale gen=1 Update should be suppressed");

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "different cwds must trigger independent slow computes"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_reload_does_not_poison_cache() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let config_path = dir.path().join("config.toml");
    write_config(&config_path, "[git.indicator_style]\nfg = \"red\"\n")?;

    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("feature".to_owned()),
            staged: 1,
            ..GitStatus::default()
        }),
        delay: Duration::from_millis(200),
        call_count: Some(Arc::clone(&call_count)),
    };
    let harness = TestHarness::start_with_config_path(
        provider,
        crate::config::load_config(&config_path),
        config_path.clone(),
    )
    .await?;
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request("/tmp", 1, 80)))
        .await?;
    let _ = reader.read_message().await?;

    tokio::time::sleep(Duration::from_millis(20)).await;
    std::fs::write(&config_path, "[git.indicator_style]\nfg = \"green\"\n")?;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tokio::time::timeout(Duration::from_millis(100), reader.read_message()).await;

    writer
        .write_message(&Message::Request(make_request("/tmp", 2, 80)))
        .await?;
    match reader.read_message().await? {
        Some(Message::RenderResult(rr)) => {
            assert_eq!(rr.generation, PromptGeneration::new(2));
        }
        other => return Err(format!("expected RenderResult, got {other:?}").into()),
    }

    match tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await?? {
        Some(Message::Update(update)) => {
            assert_eq!(update.generation, PromptGeneration::new(2));
            assert!(
                update.left1.contains("feature"),
                "Update should contain branch: {}",
                update.left1
            );
            assert!(
                update.left1.contains("\x1b[32m"),
                "Update should use green style from reloaded config: {}",
                update.left1
            );
        }
        other => return Err(format!("expected Update, got {other:?}").into()),
    }

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_cross_session_cache() -> Result<(), Box<dyn std::error::Error>> {
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("shared".to_owned()),
            ..GitStatus::default()
        }),
        delay: Duration::from_millis(50),
        call_count: Some(Arc::clone(&call_count)),
    };
    let harness = TestHarness::start(provider).await?;

    let (mut reader1, mut writer1) = harness.connect().await?;
    writer1
        .write_message(&Message::Request(make_request("/tmp", 1, 80)))
        .await?;
    let _ = reader1.read_message().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader1.read_message()).await??;

    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    let (mut reader2, mut writer2) = harness.connect().await?;
    writer2
        .write_message(&Message::Request(make_request_with_sid(
            "/tmp",
            other_sid(),
            1,
            80,
        )))
        .await?;
    let resp2 = reader2.read_message().await?;
    match resp2 {
        Some(Message::RenderResult(rr)) => {
            assert!(
                rr.left1.contains("shared"),
                "second session should get cached slow output: {}",
                rr.left1
            );
        }
        other => return Err(format!("expected RenderResult, got {other:?}").into()),
    }

    let no_update = tokio::time::timeout(Duration::from_millis(200), reader2.read_message()).await;
    assert!(
        no_update.is_err(),
        "cache hit should not produce an Update for second session"
    );
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "cache hit should still revalidate git in background"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_suppresses_stale_updates() -> Result<(), Box<dyn std::error::Error>> {
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        delay: Duration::from_millis(100),
        call_count: Some(Arc::clone(&call_count)),
    };
    let harness = TestHarness::start(provider).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    for generation in 1..=4u64 {
        writer
            .write_message(&Message::Request(make_request("/tmp", generation, 80)))
            .await?;
    }

    for _ in 0..4 {
        match reader.read_message().await? {
            Some(Message::RenderResult(_)) => {}
            other => {
                return Err(format!("expected RenderResult while draining, got {other:?}").into());
            }
        }
    }

    let update = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    match update {
        Some(Message::Update(u)) => {
            assert_eq!(
                u.generation,
                PromptGeneration::new(4),
                "only the latest generation should receive Update"
            );
        }
        other => return Err(format!("expected Update, got {other:?}").into()),
    }

    let extra = tokio::time::timeout(Duration::from_millis(300), reader.read_message()).await;
    assert!(
        extra.is_err(),
        "stale generation Updates should be suppressed"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_env_dep_prevents_cache_reuse() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        module: vec![ModuleDef {
            name: "env-dep".to_owned(),
            when: ModuleWhen {
                files: vec![],
                env: vec!["MY_VAR".to_owned()],
            },
            source: vec![SourceDef {
                name: "value".to_owned(),
                env: None,
                file: None,
                command: Some(vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    "echo dynamic".to_owned(),
                ]),
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: Some("via".to_owned()),
            arbitration: None,
            slot: ModuleSlot::default(),
        }],
        ..Config::default()
    };
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        call_count: Some(Arc::clone(&call_count)),
        ..MockGitProvider::default()
    };
    let harness = TestHarness::start_with_config(provider, config).await?;
    let cwd = harness.cwd_str().ok_or("missing work dir")?.to_owned();
    let (mut reader, mut writer) = harness.connect().await?;

    let mut req1 = make_request(&cwd, 1, 80);
    req1.env_vars = vec![("MY_VAR".to_owned(), "a".to_owned())];
    writer.write_message(&Message::Request(req1)).await?;
    let _ = reader.read_message().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    let mut req2 = make_request(&cwd, 2, 80);
    req2.env_vars = vec![("MY_VAR".to_owned(), "b".to_owned())];
    writer.write_message(&Message::Request(req2)).await?;
    let _ = reader.read_message().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "different env values should produce different cache keys"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_file_dep_cache_hit() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        module: vec![make_sleep_module("file-dep", 50, "CACHED")],
        ..Config::default()
    };
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        call_count: Some(Arc::clone(&call_count)),
        ..MockGitProvider::default()
    };
    let harness = TestHarness::start_with_config(provider, config).await?;
    let cwd = harness.cwd_str().ok_or("missing work dir")?.to_owned();
    let (mut reader, mut writer) = harness.connect().await?;

    writer
        .write_message(&Message::Request(make_request(&cwd, 1, 80)))
        .await?;
    let _ = reader.read_message().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    writer
        .write_message(&Message::Request(make_request(&cwd, 2, 80)))
        .await?;
    let rr = reader.read_message().await?;
    match &rr {
        Some(Message::RenderResult(rr)) => {
            assert!(
                rr.left1.contains("main"),
                "cache hit should include git branch: {}",
                rr.left1
            );
        }
        other => {
            return Err(format!("expected RenderResult with cache hit, got {other:?}").into());
        }
    }
    let update = tokio::time::timeout(Duration::from_millis(200), reader.read_message()).await;
    assert!(
        update.is_err(),
        "file-dep module with cache hit should not trigger Update"
    );
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "cache hit should still revalidate git in background"
    );

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_status_metrics() -> Result<(), Box<dyn std::error::Error>> {
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        ..MockGitProvider::default()
    };
    let harness = TestHarness::start(provider).await?;
    let (mut reader, mut writer) = harness.connect().await?;

    // Send a prompt request first so metrics have data
    writer
        .write_message(&Message::Request(make_request("/tmp", 1, 80)))
        .await?;
    let _ = reader.read_message().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;

    // Send StatusRequest
    writer
        .write_message(&Message::StatusRequest(capsule_protocol::StatusRequest))
        .await?;

    let resp = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    match resp {
        Some(Message::StatusResponse(s)) => {
            assert!(s.pid > 0, "pid should be positive");
            assert!(
                s.requests_total >= 1,
                "should have at least one request: {}",
                s.requests_total
            );
            assert!(
                s.connections_active >= 1,
                "should have at least one active connection"
            );
        }
        other => return Err(format!("expected StatusResponse, got {other:?}").into()),
    }

    harness.shutdown().await
}

#[tokio::test]
async fn test_daemon_cache_off_recomputes_slow_modules() -> Result<(), Box<dyn std::error::Error>> {
    let call_count = count_git_calls();
    let provider = MockGitProvider {
        status: Some(GitStatus {
            branch: Some("main".to_owned()),
            ..GitStatus::default()
        }),
        call_count: Some(Arc::clone(&call_count)),
        ..MockGitProvider::default()
    };
    let config = Config {
        cache: CacheConfig {
            slow: SlowCacheMode::Off,
        },
        ..Config::default()
    };
    let harness = TestHarness::start_with_config(provider, config).await?;
    let cwd = harness.cwd_str().ok_or("missing work dir")?.to_owned();
    let (mut reader, mut writer) = harness.connect().await?;

    // First request: cache miss, slow compute runs git.
    writer
        .write_message(&Message::Request(make_request(&cwd, 1, 80)))
        .await?;
    let _ = reader.read_message().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Second request: with cache off, should recompute (not serve from cache).
    writer
        .write_message(&Message::Request(make_request(&cwd, 2, 80)))
        .await?;
    let _ = reader.read_message().await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "cache off should recompute slow modules every request"
    );

    harness.shutdown().await
}
