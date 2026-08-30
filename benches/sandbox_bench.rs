use criterion::{black_box, criterion_group, criterion_main, Criterion};
use akiro::{Sandbox, SandboxConfig};
use std::path::PathBuf;
use std::sync::Once;
use std::time::Duration;

// The full sandbox path relies on a process-global SIGCHLD reaper (installed in `main.rs`) to
// collect each child's exit and route it to the supervisor. A criterion binary has no `main.rs`,
// so without this the supervisor never sees `/bin/true` exit and blocks until the wall-time
// deadline (~4 s), which would bury the per-test-case jail cost we're trying to measure. Drive
// the reaper from a background polling thread so `execute()` returns at its true speed. The 200 µs
// poll jitter affects fs-on and fs-off identically, so it cancels in the on−off delta.
static REAPER: Once = Once::new();
fn ensure_reaper() {
    REAPER.call_once(|| {
        std::thread::spawn(|| loop {
            akiro::sandbox::reaper::reap_all();
            std::thread::sleep(Duration::from_micros(200));
        });
    });
}

fn bench_simple_echo(c: &mut Criterion) {
    ensure_reaper();
    c.bench_function("echo_hello_world", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| async {
            let config = SandboxConfig::new(black_box(PathBuf::from("/bin/echo")))
                .with_args(vec!["Hello World".to_string()])
                .with_time_limit(1000);

            Sandbox::execute(config).await.unwrap()
        });
    });
}

fn bench_cpu_time(c: &mut Criterion) {
    ensure_reaper();
    c.bench_function("noop_loop_1000ms", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| async {
            let config = SandboxConfig::new(black_box(PathBuf::from("/bin/sh")))
                .with_args(vec![
                    "-c".to_string(),
                    "i=0; while [ $i -lt 1000000 ]; do i=$((i+1)); done".to_string(),
                ])
                .with_time_limit(5000);

            Sandbox::execute(config).await.unwrap()
        });
    });
}

// ── P1 jail-cost measurement ────────────────────────────────────────────────
// Run a near-zero-work program (`/bin/true`) through the full sandbox twice, holding
// everything constant EXCEPT the filesystem jail. Both paths pay the same cgroup +
// fork + seccomp + privilege-drop cost; the DELTA between `fs_isolation_on` and
// `fs_isolation_off` is the per-test-case pivot_root jail cost (the ~10 read-only bind
// mounts + /dev + /proc + pivot). If that delta is a material share of a fast job's
// wall time, the prebuilt-skeleton optimization (P1 Step 2) is worth building.
//
// NOTE: requires a privileged, cgroup-v2-delegated environment (Sandbox::execute always
// creates a fail-closed cgroup). Run inside the akiro image / a privileged rust:bookworm
// container with `/sys/fs/cgroup/judge` delegated, and set JUDGE_MEM_BUDGET_BYTES high so
// the admission semaphore never gates the sequential iterations.

fn bench_run_fs_isolation_on(c: &mut Criterion) {
    ensure_reaper();
    c.bench_function("run_true_fs_isolation_on", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| async {
            let config = SandboxConfig::new(black_box(PathBuf::from("/bin/true")))
                .with_time_limit(1000)
                .with_network_isolation(true)
                .with_fs_isolation(true);

            Sandbox::execute(config).await.unwrap()
        });
    });
}

fn bench_run_fs_isolation_off(c: &mut Criterion) {
    ensure_reaper();
    c.bench_function("run_true_fs_isolation_off", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| async {
            let config = SandboxConfig::new(black_box(PathBuf::from("/bin/true")))
                .with_time_limit(1000)
                .with_network_isolation(true)
                .with_fs_isolation(false);

            Sandbox::execute(config).await.unwrap()
        });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(50);
    targets = bench_simple_echo, bench_cpu_time, bench_run_fs_isolation_on, bench_run_fs_isolation_off
);
criterion_main!(benches);
