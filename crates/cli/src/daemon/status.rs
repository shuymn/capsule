use std::time::Duration;

use capsule_protocol::{Message, PROTOCOL_VERSION, StatusRequest};

use super::socket_path;

/// Query daemon metrics and print them.
///
/// # Errors
///
/// Returns an error if the daemon is not running or the status exchange fails.
pub fn status(json: bool) -> anyhow::Result<()> {
    let sock = socket_path()?;
    let req = Message::StatusRequest(StatusRequest {
        version: PROTOCOL_VERSION,
    });

    match crate::connect::sync_request(&sock, &req, Duration::from_secs(5))? {
        Message::StatusResponse(resp) => {
            if json {
                print_status_json(&resp);
            } else {
                print_status_human(&resp);
            }
        }
        _ => anyhow::bail!("unexpected message type from daemon"),
    }
    Ok(())
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let remain = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m {remain}s")
    } else if hours > 0 {
        format!("{hours}h {mins}m {remain}s")
    } else if mins > 0 {
        format!("{mins}m {remain}s")
    } else {
        format!("{remain}s")
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "advisory metrics — precision loss in display percentages is acceptable"
)]
fn print_status_human(r: &capsule_protocol::StatusResponse) {
    let total = r.cache_hits + r.cache_misses;
    let hit_rate = if total > 0 {
        format!("{:.1}%", r.cache_hits as f64 / total as f64 * 100.0)
    } else {
        "n/a".to_owned()
    };
    let avg_slow = if r.slow_computes_started > 0 {
        format!(
            "{:.1}ms",
            r.slow_compute_duration_us as f64 / r.slow_computes_started as f64 / 1000.0
        )
    } else {
        "n/a".to_owned()
    };

    println!(
        "capsule daemon (pid {}) uptime {}\n",
        r.pid,
        format_uptime(r.uptime_secs)
    );
    println!("cache:");
    println!(
        "  hits: {}  misses: {}  hit_rate: {}",
        r.cache_hits, r.cache_misses, hit_rate
    );
    println!(
        "  evictions: {}  entries: {}",
        r.cache_evictions, r.cache_entries
    );
    println!("  inflight_coalesces: {}", r.inflight_coalesces);
    println!("\nrequests:");
    println!(
        "  total: {}  stale_discards: {}",
        r.requests_total, r.stale_discards
    );
    println!("\nslow_compute:");
    println!(
        "  started: {}  avg_duration: {}",
        r.slow_computes_started, avg_slow
    );
    println!(
        "  git_timeouts: {}  custom_module_timeouts: {}",
        r.git_timeouts, r.custom_module_timeouts
    );
    println!("\nsessions:");
    println!(
        "  active: {}  pruned: {}",
        r.active_sessions, r.sessions_pruned
    );
    println!("\nconnections:");
    println!(
        "  total: {}  active: {}",
        r.connections_total, r.connections_active
    );
    println!("\nconfig:");
    println!(
        "  generation: {}  reloads: {}  reload_errors: {}",
        r.config_generation, r.config_reloads, r.config_reload_errors
    );
}

fn print_status_json(r: &capsule_protocol::StatusResponse) {
    if let Err(error) = serde_json::to_writer(std::io::stdout(), r) {
        eprintln!("failed to write JSON: {error}");
    }
    println!();
}
