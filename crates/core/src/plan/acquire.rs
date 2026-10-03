use capsule_protocol::session::Snapshot;
use tokio_util::sync::CancellationToken;

use super::{Candidate, ModuleWhen, Source, ValuePlan};
use crate::acquire::{AcquireError, MAX_OUTPUT_BYTES, Runner};

/// Check conditions before starting the module's value acquisitions.
///
/// Environment presence includes empty values. A pending/failed condition never
/// authorizes commands. Uses the caller's Tokio runtime and cancellation token.
///
/// # Errors
///
/// Returns cancellation or a bounded file-condition failure if no file matches.
pub async fn acquire_condition(
    when: &ModuleWhen,
    snapshot: &Snapshot,
    runner: &Runner,
    cancel: &CancellationToken,
) -> Result<bool, AcquireError> {
    if cancel.is_cancelled() {
        return Err(AcquireError::Cancelled);
    }
    let env_matches =
        when.env.is_empty() || when.env.iter().any(|name| snapshot.env(name).is_some());
    if !env_matches || when.files.is_empty() {
        return Ok(env_matches);
    }
    let mut failure = None;
    for path in &when.files {
        match runner.metadata(&snapshot.cwd.join(path), cancel).await {
            Ok(Some(metadata)) if metadata.is_file() => return Ok(true),
            Ok(_) => {}
            Err(AcquireError::Cancelled) => return Err(AcquireError::Cancelled),
            Err(error) => failure = Some(error),
        }
    }
    failure.map_or(Ok(false), Err)
}

/// Resolve a value sequentially, stopping at the first successful candidate.
///
/// `Some("")` is a successful empty value and never triggers a later command.
/// Missing sources and failed candidates permit fallback; cancellation does not.
/// File/command text is trimmed, while environment text is preserved as supplied.
///
/// # Errors
///
/// Returns cancellation immediately, or the last failure when no candidate is
/// ready. Returns `Ok(None)` when every candidate is missing or fails extraction.
pub async fn acquire_value(
    value: &ValuePlan,
    snapshot: &Snapshot,
    runner: &Runner,
    cancel: &CancellationToken,
) -> Result<Option<String>, AcquireError> {
    let mut failure = None;
    for candidate in &value.candidates {
        if cancel.is_cancelled() {
            return Err(AcquireError::Cancelled);
        }
        match acquire_candidate(candidate, snapshot, runner, cancel).await {
            Ok(Some(value)) => return Ok(Some(value)),
            Ok(None) => {}
            Err(AcquireError::Cancelled) => return Err(AcquireError::Cancelled),
            Err(error) => failure = Some(error),
        }
    }
    failure.map_or(Ok(None), Err)
}

async fn acquire_candidate(
    candidate: &Candidate,
    snapshot: &Snapshot,
    runner: &Runner,
    cancel: &CancellationToken,
) -> Result<Option<String>, AcquireError> {
    let text = match &candidate.source {
        Source::Env(name) => {
            let Some(value) = snapshot.env(name) else {
                return Ok(None);
            };
            if value.as_encoded_bytes().len() > MAX_OUTPUT_BYTES {
                return Err(AcquireError::OutputTooLarge);
            }
            return extract(&value.to_string_lossy(), candidate);
        }
        Source::File(path) => {
            let Some(bytes) = runner.file(&snapshot.cwd.join(path), cancel).await? else {
                return Ok(None);
            };
            bytes
        }
        Source::Command(argv) => runner.command(argv, snapshot, cancel).await?,
    };
    extract(String::from_utf8_lossy(&text).trim(), candidate)
}

fn extract(text: &str, candidate: &Candidate) -> Result<Option<String>, AcquireError> {
    if text.len() > MAX_OUTPUT_BYTES {
        return Err(AcquireError::OutputTooLarge);
    }
    if let Some(regex) = &candidate.regex {
        return Ok(regex
            .captures(text)
            .and_then(|captures| captures.get(1).map(|value| value.as_str().to_owned())));
    }
    Ok(Some(text.to_owned()))
}
