// SPDX-License-Identifier: MIT
// Iris — iris-core::pipeline
//
// Process-wide pipeline diagnostics counters + Prometheus exposition.
// Referenced by the runtime's /metrics endpoint and the ForceRebase
// diagnostics command.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static REBASE_COUNT: AtomicU64 = AtomicU64::new(0);
static START_UNIX_MS: AtomicU64 = AtomicU64::new(0);

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn start_ms() -> u64 {
    // First caller stamps process start; subsequent callers read it.
    let cur = START_UNIX_MS.load(Ordering::Relaxed);
    if cur != 0 {
        return cur;
    }
    let now = now_unix_ms();
    match START_UNIX_MS.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed) {
        Ok(_) => now,
        Err(existing) => existing,
    }
}

/// Increment the rebase counter (diagnostics command / tests).
pub fn force_increment_rebase_for_test() {
    REBASE_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Current rebase counter value.
pub fn rebase_count() -> u64 {
    REBASE_COUNT.load(Ordering::Relaxed)
}

/// Prometheus text-format exposition of the pipeline counters.
pub fn prometheus_text() -> String {
    let uptime_ms = now_unix_ms().saturating_sub(start_ms());
    format!(
        "# HELP iris_rebase_total Number of pipeline rebase events.\n\
         # TYPE iris_rebase_total counter\n\
         iris_rebase_total {}\n\
         # HELP iris_uptime_milliseconds Process uptime in milliseconds.\n\
         # TYPE iris_uptime_milliseconds gauge\n\
         iris_uptime_milliseconds {}\n",
        rebase_count(),
        uptime_ms
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebase_counter_increments_and_appears_in_exposition() {
        let before = rebase_count();
        force_increment_rebase_for_test();
        assert_eq!(rebase_count(), before + 1);
        let text = prometheus_text();
        assert!(text.contains("iris_rebase_total"));
        assert!(text.contains("iris_uptime_milliseconds"));
        assert!(text.contains(&format!("iris_rebase_total {}", before + 1)));
    }
}
