use std::sync::atomic::{AtomicU64, Ordering};
use crate::orchestrator::JudgeVerdict;

// Global lock-free atomic telemetry counters (consumes < 20 KB of RAM total)
static TOTAL_JOBS: AtomicU64 = AtomicU64::new(0);
static JOBS_ACCEPTED: AtomicU64 = AtomicU64::new(0);
static JOBS_WRONG_ANSWER: AtomicU64 = AtomicU64::new(0);
static JOBS_TLE: AtomicU64 = AtomicU64::new(0);
static JOBS_MLE: AtomicU64 = AtomicU64::new(0);
static JOBS_RUNTIME_ERROR: AtomicU64 = AtomicU64::new(0);
static JOBS_COMPILE_ERROR: AtomicU64 = AtomicU64::new(0);

static LANG_CPP: AtomicU64 = AtomicU64::new(0);
static LANG_PYTHON: AtomicU64 = AtomicU64::new(0);
static LANG_PYPY: AtomicU64 = AtomicU64::new(0);
static LANG_JAVA: AtomicU64 = AtomicU64::new(0);
static LANG_C: AtomicU64 = AtomicU64::new(0);
static LANG_JS: AtomicU64 = AtomicU64::new(0);
static LANG_TS: AtomicU64 = AtomicU64::new(0);
static LANG_SQL: AtomicU64 = AtomicU64::new(0);

static TOTAL_TESTCASES: AtomicU64 = AtomicU64::new(0);
static TESTCASES_PASSED: AtomicU64 = AtomicU64::new(0);
static TESTCASES_FAILED: AtomicU64 = AtomicU64::new(0);
static TOTAL_CPU_TIME_MS: AtomicU64 = AtomicU64::new(0);

/// Record completed job telemetry atomically
pub fn record_job(
    language: &str,
    verdict: &JudgeVerdict,
    cpu_time_ms: u64,
    passed_tests: usize,
    failed_tests: usize,
) {
    TOTAL_JOBS.fetch_add(1, Ordering::Relaxed);
    TOTAL_CPU_TIME_MS.fetch_add(cpu_time_ms, Ordering::Relaxed);
    TOTAL_TESTCASES.fetch_add((passed_tests + failed_tests) as u64, Ordering::Relaxed);
    TESTCASES_PASSED.fetch_add(passed_tests as u64, Ordering::Relaxed);
    TESTCASES_FAILED.fetch_add(failed_tests as u64, Ordering::Relaxed);

    match verdict {
        JudgeVerdict::Accepted => JOBS_ACCEPTED.fetch_add(1, Ordering::Relaxed),
        JudgeVerdict::WrongAnswer => JOBS_WRONG_ANSWER.fetch_add(1, Ordering::Relaxed),
        JudgeVerdict::TimeLimitExceeded => JOBS_TLE.fetch_add(1, Ordering::Relaxed),
        JudgeVerdict::MemoryLimitExceeded => JOBS_MLE.fetch_add(1, Ordering::Relaxed),
        JudgeVerdict::RuntimeError => JOBS_RUNTIME_ERROR.fetch_add(1, Ordering::Relaxed),
        JudgeVerdict::CompilationError => JOBS_COMPILE_ERROR.fetch_add(1, Ordering::Relaxed),
    };

    match language.to_lowercase().as_str() {
        "cpp" | "c++" => LANG_CPP.fetch_add(1, Ordering::Relaxed),
        "python" | "python3" | "py" => LANG_PYTHON.fetch_add(1, Ordering::Relaxed),
        "pypy" | "pypy3" => LANG_PYPY.fetch_add(1, Ordering::Relaxed),
        "java" => LANG_JAVA.fetch_add(1, Ordering::Relaxed),
        "c" => LANG_C.fetch_add(1, Ordering::Relaxed),
        "javascript" | "js" => LANG_JS.fetch_add(1, Ordering::Relaxed),
        "typescript" | "ts" => LANG_TS.fetch_add(1, Ordering::Relaxed),
        "sql" => LANG_SQL.fetch_add(1, Ordering::Relaxed),
        _ => 0,
    };
}

/// Read resident set size (RSS) memory in bytes from /proc/self/statm
fn get_process_memory_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
            let parts: Vec<&str> = content.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(pages) = parts[1].parse::<u64>() {
                    return pages * 4096;
                }
            }
        }
    }
    0
}

/// Render standard Prometheus exposition format v0.0.4
pub fn render_prometheus(
    total_workers: usize,
    idle_workers: usize,
    busy_workers: usize,
    queued_jobs: usize,
    uptime_secs: u64,
) -> String {
    let mem_bytes = get_process_memory_bytes();

    let mut out = String::with_capacity(2048);

    out.push_str("# HELP akiro_jobs_total Total evaluation jobs submitted to Akiro\n");
    out.push_str("# TYPE akiro_jobs_total counter\n");
    out.push_str(&format!("akiro_jobs_total{{verdict=\"Accepted\"}} {}\n", JOBS_ACCEPTED.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_total{{verdict=\"WrongAnswer\"}} {}\n", JOBS_WRONG_ANSWER.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_total{{verdict=\"TimeLimitExceeded\"}} {}\n", JOBS_TLE.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_total{{verdict=\"MemoryLimitExceeded\"}} {}\n", JOBS_MLE.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_total{{verdict=\"RuntimeError\"}} {}\n", JOBS_RUNTIME_ERROR.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_total{{verdict=\"CompilationError\"}} {}\n", JOBS_COMPILE_ERROR.load(Ordering::Relaxed)));

    out.push_str("\n# HELP akiro_jobs_by_language_total Total jobs grouped by programming language\n");
    out.push_str("# TYPE akiro_jobs_by_language_total counter\n");
    out.push_str(&format!("akiro_jobs_by_language_total{{language=\"cpp\"}} {}\n", LANG_CPP.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_by_language_total{{language=\"python\"}} {}\n", LANG_PYTHON.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_by_language_total{{language=\"pypy3\"}} {}\n", LANG_PYPY.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_by_language_total{{language=\"java\"}} {}\n", LANG_JAVA.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_by_language_total{{language=\"c\"}} {}\n", LANG_C.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_by_language_total{{language=\"javascript\"}} {}\n", LANG_JS.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_by_language_total{{language=\"typescript\"}} {}\n", LANG_TS.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_jobs_by_language_total{{language=\"sql\"}} {}\n", LANG_SQL.load(Ordering::Relaxed)));

    out.push_str("\n# HELP akiro_testcases_total Total isolated sandbox testcase executions\n");
    out.push_str("# TYPE akiro_testcases_total counter\n");
    out.push_str(&format!("akiro_testcases_total{{status=\"passed\"}} {}\n", TESTCASES_PASSED.load(Ordering::Relaxed)));
    out.push_str(&format!("akiro_testcases_total{{status=\"failed\"}} {}\n", TESTCASES_FAILED.load(Ordering::Relaxed)));

    out.push_str("\n# HELP akiro_cpu_time_ms_total Cumulative CPU time spent inside sandboxes\n");
    out.push_str("# TYPE akiro_cpu_time_ms_total counter\n");
    out.push_str(&format!("akiro_cpu_time_ms_total {}\n", TOTAL_CPU_TIME_MS.load(Ordering::Relaxed)));

    out.push_str("\n# HELP akiro_workers Total, idle, and busy judge workers in cluster\n");
    out.push_str("# TYPE akiro_workers gauge\n");
    out.push_str(&format!("akiro_workers{{state=\"total\"}} {}\n", total_workers));
    out.push_str(&format!("akiro_workers{{state=\"idle\"}} {}\n", idle_workers));
    out.push_str(&format!("akiro_workers{{state=\"busy\"}} {}\n", busy_workers));

    out.push_str("\n# HELP akiro_queue_depth Number of jobs currently queued\n");
    out.push_str("# TYPE akiro_queue_depth gauge\n");
    out.push_str(&format!("akiro_queue_depth {}\n", queued_jobs));

    out.push_str("\n# HELP akiro_process_memory_bytes Resident memory (RSS) of Akiro process\n");
    out.push_str("# TYPE akiro_process_memory_bytes gauge\n");
    out.push_str(&format!("akiro_process_memory_bytes {}\n", mem_bytes));

    out.push_str("\n# HELP akiro_uptime_seconds Total process uptime in seconds\n");
    out.push_str("# TYPE akiro_uptime_seconds gauge\n");
    out.push_str(&format!("akiro_uptime_seconds {}\n", uptime_secs));

    out
}
