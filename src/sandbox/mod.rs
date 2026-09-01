pub mod config;
pub mod result;
pub mod seccomp;
pub mod admission;

#[cfg(target_os = "linux")]
pub mod child;
#[cfg(target_os = "linux")]
pub mod supervisor;
#[cfg(target_os = "linux")]
pub mod cgroups;
#[cfg(target_os = "linux")]
pub mod fs;
#[cfg(target_os = "linux")]
pub mod reaper;

#[cfg(target_os = "linux")]
use {
    libc::c_int,
    nix::unistd::{fork, ForkResult},
    std::io::Write,
    std::os::unix::io::FromRawFd,
    tokio::task,
    child::{setup_child_process, ChildProcessPipes},
    cgroups::CgroupManager,
    supervisor::{ProcessSupervisor, read_pipe_output},
};

use thiserror::Error;

pub use config::SandboxConfig;
pub use result::{ExecutionResult, SandboxStatus};

#[cfg(target_os = "linux")]
#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("Fork failed: {0}")]
    ForkError(#[from] nix::Error),
    #[error("Child process error: {0}")]
    ChildError(#[from] child::ChildError),
    #[error("Supervisor error: {0}")]
    SupervisorError(#[from] supervisor::SupervisorError),
    #[error("Cgroup error: {0}")]
    CgroupError(#[from] cgroups::CgroupError),
    #[error("Seccomp error: {0}")]
    SeccompError(#[from] seccomp::SeccompError),
    #[error("Filesystem error: {0}")]
    FsError(#[from] fs::FsError),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("Sandbox is only available on Linux")]
    PlatformNotSupported,
}

pub struct Sandbox;

#[cfg(target_os = "linux")]
impl Sandbox {
    pub async fn execute(mut config: SandboxConfig) -> Result<ExecutionResult, SandboxError> {
        // Memory-aware admission: acquire the memory-budget permits FIRST, before allocating any
        // kernel resources (pipes / cgroup / fork), so a queued job waits holding nothing. Bound
        // to a named local declared first ⇒ by Rust's reverse drop order it releases LAST, after
        // the supervisor's cgroup teardown, so the budget is returned only once the memory is
        // actually freed. NOTE: `let _ = acquire(...)` would drop the permit immediately and
        // silently disable the gate — it must be a named binding held to function end.
        let _permit = admission::budget()
            .acquire_many_owned(admission::units_for(config.memory_limit_bytes))
            .await
            .expect("admission semaphore is 'static and is never closed");

        // Assign a per-execution jail id in the PARENT (before fork) so we can deterministically
        // remove the child's `/tmp/judge_root_<jail_id>` directory after it exits. The child's
        // `FsIsolation` Drop never runs — `execve` replaces its image first — so without this the
        // directory leaks one entry per test case onto the (overlay) `/tmp`.
        if config.enable_fs_isolation && config.jail_id.is_empty() {
            config.jail_id = uuid::Uuid::new_v4().to_string();
        }

        let pipes = ChildProcessPipes::new()?;

        // Create cgroup v2 for memory and CPU limits.
        // FAIL-CLOSED: running untrusted code with no physical RAM / PID ceiling is a DoS
        // and host-stability risk, so a cgroup setup failure aborts the job. This requires
        // the container to have cgroup-v2 controller delegation working (see entrypoint.sh).
        // Offloaded to a blocking task: the mkdir + memory.max/pids.max writes are sync fs
        // I/O and must not run on the async reactor under load.
        let cgroup = {
            let cfg = config.clone();
            match task::spawn_blocking(move || CgroupManager::new(&cfg)).await {
                Ok(Ok(cg)) => Some(cg),
                Ok(Err(e)) => return Err(SandboxError::CgroupError(e)),
                Err(_) => {
                    return Err(SandboxError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "cgroup setup task panicked",
                    )))
                }
            }
        };

        let mut fork_res = unsafe { fork() };
        let mut retries = 0;
        while let Err(nix::errno::Errno::EAGAIN) = fork_res {
            if retries >= 10 {
                break;
            }
            retries += 1;
            tokio::time::sleep(tokio::time::Duration::from_millis(20 * retries)).await;
            fork_res = unsafe { fork() };
        }

        match fork_res? {
            ForkResult::Parent { child } => {
                // Claim this child so the global reaper routes its exit status to the
                // supervisor (accurate exit code / cpu / memory → correct MLE/TLE/RE)
                // instead of discarding it. The handle is moved into the supervisor and
                // unregisters when supervision ends.
                let supervised = reaper::register_child(child.as_raw());

                pipes.close_parent_ends();

                // Attach child to cgroup immediately after fork.
                // FAIL-CLOSED: an unattached child runs outside all limits, so on failure we
                // kill it and surface the error rather than run it unconfined. The global
                // reaper collects the killed child (do not wait4 here — that races the reaper).
                if let Some(ref cg) = cgroup {
                    if let Err(e) = cg.attach_proc(child.as_raw()) {
                        unsafe { libc::kill(child.as_raw(), libc::SIGKILL); }
                        return Err(SandboxError::CgroupError(e));
                    }
                }

                let supervisor = ProcessSupervisor::new(child, config.clone(), cgroup, supervised);

                let stdin_handle = task::spawn_blocking({
                    let stdin_data = config.stdin_data.clone();
                    let stdin_write = pipes.stdin_write;
                    move || {
                        if let Some(data) = stdin_data {
                            let _ = write_all_to_fd(stdin_write, &data);
                        }
                        unsafe { libc::close(stdin_write) };
                    }
                });

                let stdout_future = read_pipe_output(pipes.stdout_read, config.max_output_bytes);
                let stderr_future = read_pipe_output(pipes.stderr_read, config.max_output_bytes);

                let supervise_res = supervisor.supervise().await;

                // Deterministic parent-side cleanup of the child's jail root directory. The
                // child has exited (supervise returned), so its private-namespace mounts are
                // gone and only the empty host-visible directory remains — a race-free `rmdir`.
                // `remove_dir` (not `remove_dir_all`) deliberately never crosses a mount point;
                // any rare leftover is swept at boot by entrypoint.sh.
                if config.enable_fs_isolation && !config.jail_id.is_empty() {
                    let _ = std::fs::remove_dir(format!("/tmp/judge_root_{}", config.jail_id));
                }
                // Bounded: if a program never drains a large stdin, the blocking writer
                // would otherwise wait forever. 5s is well beyond any real input feed.
                let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), stdin_handle).await;

                // Drain the child's captured stdout/stderr. The child has exited and the parent
                // closed its write-ends at fork, so EOF is guaranteed and a *scheduled* read
                // finishes in microseconds — this timeout is only a backstop against a truly
                // stuck read. It was hardcoded at 250ms, which under heavy CPU overcommit (many
                // inflight jobs starving the spawn_blocking read task of a timeslice) could expire
                // before the read grabbed the buffered output → empty stdout → a CORRECT run
                // scored as WrongAnswer. 5s default, env-tunable. A fired timeout is LOGGED, never
                // silently swallowed into a wrong verdict.
                let drain_timeout_ms: u64 = std::env::var("JUDGE_DRAIN_TIMEOUT_MS")
                    .ok().and_then(|s| s.parse().ok()).unwrap_or(5000);
                let drain_timeout = tokio::time::Duration::from_millis(drain_timeout_ms);
                let stdout_res = tokio::time::timeout(drain_timeout, stdout_future).await;
                let stderr_res = tokio::time::timeout(drain_timeout, stderr_future).await;

                if stdout_res.is_err() {
                    tracing::warn!(
                        "stdout drain timed out after {}ms (jail_id={}) — output may be truncated, risking a spurious WrongAnswer",
                        drain_timeout_ms, config.jail_id
                    );
                }
                if stderr_res.is_err() {
                    tracing::warn!("stderr drain timed out after {}ms (jail_id={})", drain_timeout_ms, config.jail_id);
                }

                let out = stdout_res.ok().and_then(|r| r.ok()).unwrap_or_default();
                let err = stderr_res.ok().and_then(|r| r.ok()).unwrap_or_default();

                match supervise_res {
                    Ok(mut res) => {
                        res.stdout = out;
                        res.stderr = err;

                        if res.stdout.len() >= config.max_output_bytes {
                            res.status = SandboxStatus::OutputLimitExceeded;
                        }

                        Ok(res)
                    }
                    Err(e) => Err(SandboxError::SupervisorError(e)),
                }
            }
            ForkResult::Child => {
                let _ = setup_child_process(&config, &pipes);
                unsafe { libc::exit(1) };
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Sandbox {
    pub async fn execute(_config: SandboxConfig) -> Result<ExecutionResult, SandboxError> {
        Err(SandboxError::PlatformNotSupported)
    }
}

#[cfg(target_os = "linux")]
fn write_all_to_fd(fd: c_int, data: &[u8]) -> std::io::Result<()> {
    let mut total_written = 0;
    while total_written < data.len() {
        let ret = unsafe {
            libc::write(
                fd,
                data[total_written..].as_ptr() as *const libc::c_void,
                data.len() - total_written,
            )
        };
        if ret <= 0 {
            break;
        }
        total_written += ret as usize;
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_simple_echo() {
        let config = SandboxConfig::new(PathBuf::from("/bin/echo"))
            .with_args(vec!["Hello World".to_string()])
            .with_time_limit(1000);

        let result = Sandbox::execute(config).await.expect("Execution failed");
        assert_eq!(result.status, SandboxStatus::Ok);
        assert_eq!(result.exit_code, 0);
        assert!(String::from_utf8_lossy(&result.stdout).contains("Hello World"));
    }

    #[tokio::test]
    async fn test_time_limit_exceeded() {
        let config = SandboxConfig::new(PathBuf::from("/bin/sleep"))
            .with_args(vec!["10".to_string()])
            .with_time_limit(500);

        let result = Sandbox::execute(config).await.expect("Execution failed");
        assert_eq!(result.status, SandboxStatus::TimeLimitExceeded);
    }

    #[tokio::test]
    async fn test_output_limit() {
        let config = SandboxConfig::new(PathBuf::from("/bin/dd"))
            .with_args(vec![
                "if=/dev/zero".to_string(),
                "bs=1M".to_string(),
                "count=100".to_string(),
            ])
            .with_max_output(1024 * 1024)
            .with_time_limit(5000);

        let result = Sandbox::execute(config).await.expect("Execution failed");
        assert!(matches!(
            result.status,
            SandboxStatus::OutputLimitExceeded | SandboxStatus::RuntimeError(_)
        ));
    }

    #[tokio::test]
    async fn test_exit_code() {
        let config = SandboxConfig::new(PathBuf::from("/bin/sh"))
            .with_args(vec!["-c".to_string(), "exit 42".to_string()])
            .with_time_limit(1000);

        let result = Sandbox::execute(config).await.expect("Execution failed");
        assert_eq!(result.exit_code, 42);
    }

    #[tokio::test]
    async fn test_stdin() {
        let config = SandboxConfig::new(PathBuf::from("/bin/cat"))
            .with_stdin(b"test input\n".to_vec())
            .with_time_limit(1000);

        let result = Sandbox::execute(config).await.expect("Execution failed");
        assert_eq!(result.status, SandboxStatus::Ok);
        assert_eq!(result.stdout, b"test input\n");
    }
}
