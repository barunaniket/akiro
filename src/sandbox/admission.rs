//! Global, memory-aware admission control for sandbox executions.
//!
//! On a small host (the target VM is 2 cores / 1 GB) the existing job-count gates — `num_workers`
//! in the worker pool and `num_workers * 2` in the redis consumer — do **not** bound *memory*:
//! each job fans out to `cores` concurrent test executions, and a job may request up to the
//! validation ceiling. Several concurrent compiles or large runs can therefore exceed physical
//! RAM and invite the *host* OOM killer into the judge process itself — a stability hole and a
//! one-request DoS.
//!
//! This module adds a single process-global semaphore that every [`crate::sandbox::Sandbox::execute`]
//! call must acquire before committing memory (cgroup create + fork). Permits represent a MEMORY
//! BUDGET measured in fixed 256 MB units; a job acquires `ceil(mem / unit)` units, so admission is
//! throttled by megabytes, not by task count. It is strictly additive to (tighter than) the
//! existing gates and, because both ingress paths funnel through `Sandbox::execute`, it is the one
//! common choke point.

use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;

/// One admission unit. Matches the default per-run memory limit (256 MB).
const BASE_UNIT_BYTES: u64 = 256 * 1024 * 1024;

/// Default total budget when `JUDGE_MEM_BUDGET_BYTES` is unset. Conservative for a 1 GB host:
/// leaves ~256 MB for the host OS, the judge server, and embedded Redis.
const DEFAULT_BUDGET_BYTES: u64 = 768 * 1024 * 1024;

/// Default per-job memory clamp when `JUDGE_MAX_MEMORY_BYTES` is unset (see [`max_job_memory_bytes`]).
const DEFAULT_MAX_JOB_BYTES: u64 = 512 * 1024 * 1024;

/// Default compile-phase memory clamp when `JUDGE_COMPILE_MEMORY_BYTES` is unset.
const DEFAULT_COMPILE_BYTES: u64 = 512 * 1024 * 1024;

/// Parse a byte count from an env value: a plain integer of bytes, or a number with a
/// `k`/`m`/`g` suffix (1024-based, case-insensitive). Returns `None` on empty/garbage.
pub fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let last = s.as_bytes()[s.len() - 1].to_ascii_lowercase();
    let (num, mult): (&str, u64) = match last {
        b'k' => (&s[..s.len() - 1], 1024),
        b'm' => (&s[..s.len() - 1], 1024 * 1024),
        b'g' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    num.trim().parse::<u64>().ok().map(|n| n.saturating_mul(mult))
}

fn env_bytes(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| parse_bytes(&s))
        .unwrap_or(default)
}

/// Total admission budget in 256 MB units (read once, cached). Always `>= 1`.
fn total_units() -> usize {
    static UNITS: OnceLock<usize> = OnceLock::new();
    *UNITS.get_or_init(|| {
        let budget = env_bytes("JUDGE_MEM_BUDGET_BYTES", DEFAULT_BUDGET_BYTES);
        ((budget / BASE_UNIT_BYTES) as usize).max(1)
    })
}

/// A clone of the process-global admission semaphore (an `Arc`, so callers can use the `_owned`
/// acquire API). Lazily initialized on first use — deliberately NOT in `main`, so unit tests and
/// the criterion bench that call `Sandbox::execute` directly (with no `main`) are gated too.
pub fn budget() -> Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(SEM.get_or_init(|| Arc::new(Semaphore::new(total_units()))))
}

/// Units a job of `mem_bytes` must acquire: `ceil(mem / unit)`, clamped to the total budget so a
/// single (possibly oversized) job can never deadlock waiting for more permits than exist.
pub fn units_for(mem_bytes: u64) -> u32 {
    let units = ((mem_bytes + BASE_UNIT_BYTES - 1) / BASE_UNIT_BYTES).max(1);
    units.min(total_units() as u64) as u32
}

/// Per-job memory ceiling (`JUDGE_MAX_MEMORY_BYTES`, default 512 MB). Applied in `pool.submit` so
/// BOTH ingress paths are covered — the redis path never calls `JobRequest::validate`.
pub fn max_job_memory_bytes() -> u64 {
    env_bytes("JUDGE_MAX_MEMORY_BYTES", DEFAULT_MAX_JOB_BYTES)
}

/// Compile-phase memory ceiling (`JUDGE_COMPILE_MEMORY_BYTES`, default 512 MB). Keeps two
/// concurrent compiles from committing more than physical RAM on a 1 GB host.
pub fn compile_memory_bytes() -> u64 {
    env_bytes("JUDGE_COMPILE_MEMORY_BYTES", DEFAULT_COMPILE_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bytes_handles_suffixes_and_plain() {
        assert_eq!(parse_bytes("512"), Some(512));
        assert_eq!(parse_bytes("512m"), Some(512 * 1024 * 1024));
        assert_eq!(parse_bytes("1G"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_bytes("64k"), Some(64 * 1024));
        assert_eq!(parse_bytes("  768M  "), Some(768 * 1024 * 1024));
        assert_eq!(parse_bytes(""), None);
        assert_eq!(parse_bytes("abc"), None);
    }

    #[test]
    fn units_for_rounds_up_and_clamps() {
        // 256 MB unit. A default 256 MB run = 1 unit; a 512 MB compile = 2 units.
        assert_eq!(units_for(256 * 1024 * 1024), 1);
        assert_eq!(units_for(257 * 1024 * 1024), 2);
        assert_eq!(units_for(512 * 1024 * 1024), 2);
        assert_eq!(units_for(0), 1);
        // Never exceeds the total budget (default 768 MB = 3 units), so a lone huge job can't
        // block forever waiting on permits that don't exist.
        assert!(units_for(64u64 * 1024 * 1024 * 1024) <= total_units() as u32);
    }
}
