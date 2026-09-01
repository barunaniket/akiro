use libc::{c_int, WIFEXITED, WIFSIGNALED, WEXITSTATUS, WTERMSIG, SIGKILL};
use std::time::Instant;
use tokio::task;
use tokio::time::{sleep, Duration};
use nix::unistd::Pid;
use thiserror::Error;

use crate::sandbox::config::SandboxConfig;
use crate::sandbox::result::{ExecutionResult, SandboxStatus};
use crate::sandbox::cgroups::CgroupManager;
use crate::sandbox::reaper::SupervisedChild;

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("Failed to read from pipe: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("Wait4 failed: {0}")]
    Wait4Error(String),
    #[error("Signal error: {0}")]
    SignalError(#[from] nix::Error),
}

pub struct ProcessSupervisor {
    pid: Pid,
    config: SandboxConfig,
    start_time: Instant,
    cgroup: Option<CgroupManager>,
    supervised: SupervisedChild,
}

impl ProcessSupervisor {
    pub fn new(
        pid: Pid,
        config: SandboxConfig,
        cgroup: Option<CgroupManager>,
        supervised: SupervisedChild,
    ) -> Self {
        Self {
            pid,
            config,
            start_time: Instant::now(),
            cgroup,
            supervised,
        }
    }

    /// Supervise the child to natural exit or the wall-time deadline.
    ///
    /// Detection is by polling the reaper's stash (filled on SIGCHLD) every few ms, NOT a
    /// pidfd readiness edge. Under load that edge can be missed — the child finishes, the
    /// reaper stashes its exit immediately, but a pidfd `select` never wakes, so the job
    /// hangs until the wall-deadline timer (the intermittent ~wall-length stall we saw:
    /// 17/227 jobs pinned at exactly the deadline). Polling the stash cannot miss an edge
    /// and is fully async (never blocks a worker thread). The 5 ms poll adds negligible
    /// latency versus the 0 ms pidfd path but removes the stall entirely.
    pub async fn supervise(mut self) -> Result<ExecutionResult, SupervisorError> {
        let wall_time_deadline = self.config.wall_time_limit_ms;
        let mut kill_grace: Option<Instant> = None;

        let result = loop {
            // Reaper delivered this child's exit (natural or post-kill)?
            if let Some((status, rusage)) = self.supervised.try_take() {
                if kill_grace.is_none() {
                    // Natural exit — make sure no stragglers remain in the process tree.
                    self.kill_process_tree();
                }
                break self.build_result(status, rusage);
            }

            let now = Instant::now();

            // Wall-time deadline: kill the tree, then allow a short grace for the reaper to
            // deliver the (now guaranteed) exit status.
            if kill_grace.is_none()
                && self.start_time.elapsed().as_millis() as u64 >= wall_time_deadline
            {
                self.kill_process_tree();
                kill_grace = Some(now + Duration::from_secs(1));
            }
            if let Some(deadline) = kill_grace {
                if now >= deadline {
                    // Reaper never delivered (should not happen once killed) — synthesize.
                    let rusage: libc::rusage = unsafe { std::mem::zeroed() };
                    break self.build_result(SIGKILL, rusage);
                }
            }

            sleep(Duration::from_millis(5)).await;
        };

        // Offload cgroup teardown (kill_all + remove_dir, which can block on EBUSY) off the
        // reactor so it never stalls the async runtime under load.
        self.cleanup_cgroup_offloaded();
        Ok(result)
    }

    /// Move the cgroup into a blocking task so its Drop (which may retry `remove_dir` on
    /// EBUSY) runs off the async reactor. Fire-and-forget; the job result is already built.
    fn cleanup_cgroup_offloaded(&mut self) {
        if let Some(cg) = self.cgroup.take() {
            tokio::task::spawn_blocking(move || drop(cg));
        }
    }

    fn kill_process_tree(&self) {
        if let Some(ref cg) = self.cgroup {
            cg.kill_all();
        }
        unsafe {
            // Signal entire process group (negative PID)
            libc::kill(-self.pid.as_raw(), libc::SIGKILL);
            libc::kill(self.pid.as_raw(), libc::SIGKILL);
        }
    }

    fn build_result(&self, status: c_int, rusage: libc::rusage) -> ExecutionResult {
        let wall_time_ms = self.start_time.elapsed().as_millis() as u64;

        let cpu_time_ms = {
            let user_us = rusage.ru_utime.tv_sec * 1_000_000 + rusage.ru_utime.tv_usec;
            let sys_us = rusage.ru_stime.tv_sec * 1_000_000 + rusage.ru_stime.tv_usec;
            ((user_us + sys_us) / 1000) as u64
        };

        // Read cgroup stats once: the memory OOM-kill flag, and the peak memory.
        //
        // Report the MAX of the cgroup peak and getrusage's peak RSS. They measure
        // different things: cgroup `memory.peak` only counts memory *charged to this job's
        // cgroup* (mostly anonymous pages) — an interpreter's shared, file-backed code/libs
        // come from the read-only bind-mounts already in the host page cache, so they are
        // charged elsewhere and the cgroup under-reports (e.g. 256 KB for an 8 MB Python
        // process). `ru_maxrss` is the process's full peak RSS. The max gives the true
        // footprint in every case (anonymous bomb, interpreter, multi-process compile).
        let stats = self.cgroup.as_ref().and_then(|cg| cg.read_stats().ok());
        let memory_kb = stats
            .as_ref()
            .map(|s| (s.memory_peak_bytes / 1024).max(rusage.ru_maxrss as u64))
            .unwrap_or(rusage.ru_maxrss as u64);
        let oom_killed = stats.as_ref().map(|s| s.oom_kill_count > 0).unwrap_or(false);

        let exit_code = WEXITSTATUS(status) as i32;
        let memory_limit_kb = self.config.memory_limit_bytes / 1024;

        // A cgroup memory OOM-kill is the ground-truth signal for MemoryLimitExceeded and
        // takes precedence. (The kernel pins memory.current at exactly memory.max, so the
        // old `memory_kb > limit` strict check never fired for a real OOM kill — it was
        // mislabeled TimeLimitExceeded.)
        let sandbox_status = if oom_killed {
            SandboxStatus::MemoryLimitExceeded
        } else if WIFEXITED(status) {
            if exit_code == 0 {
                if memory_limit_kb > 0 && memory_kb > memory_limit_kb {
                    SandboxStatus::MemoryLimitExceeded
                } else {
                    SandboxStatus::Ok
                }
            } else {
                SandboxStatus::RuntimeError(exit_code)
            }
        } else if WIFSIGNALED(status) {
            let sig = WTERMSIG(status);
            if sig == SIGKILL {
                if wall_time_ms >= self.config.wall_time_limit_ms || cpu_time_ms >= self.config.time_limit_ms.saturating_sub(50) {
                    SandboxStatus::TimeLimitExceeded
                } else if memory_limit_kb > 0 && memory_kb > memory_limit_kb {
                    SandboxStatus::MemoryLimitExceeded
                } else {
                    SandboxStatus::TimeLimitExceeded
                }
            } else {
                SandboxStatus::Signaled(sig)
            }
        } else {
            SandboxStatus::RuntimeError(-1)
        };

        ExecutionResult::new(sandbox_status, exit_code)
            .with_cpu_time(cpu_time_ms)
            .with_wall_time(wall_time_ms)
            .with_memory(memory_kb)
    }
}

pub async fn read_pipe_output(fd: c_int, max_bytes: usize) -> Result<Vec<u8>, SupervisorError> {
    task::spawn_blocking(move || {
        let mut buffer = Vec::with_capacity(max_bytes.min(65536));
        let mut chunk = [0u8; 8192];
        loop {
            let ret = unsafe {
                libc::read(
                    fd,
                    chunk.as_mut_ptr() as *mut libc::c_void,
                    chunk.len(),
                )
            };
            if ret < 0 {
                // A blocking read interrupted by a signal (e.g. the reaper's SIGCHLD storm
                // under heavy concurrency) returns EINTR — retry, do NOT treat it as EOF, or
                // we would drop the child's output and score a correct run as WrongAnswer.
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                break; // genuine read error
            }
            if ret == 0 {
                break; // EOF: all write-ends of the pipe are closed
            }
            let bytes_read = ret as usize;
            if buffer.len() + bytes_read > max_bytes {
                let remaining = max_bytes - buffer.len();
                buffer.extend_from_slice(&chunk[..remaining]);
                break;
            } else {
                buffer.extend_from_slice(&chunk[..bytes_read]);
            }
        }
        unsafe { libc::close(fd) };
        Ok(buffer)
    })
    .await
    .map_err(|_| SupervisorError::ReadError(std::io::Error::new(
        std::io::ErrorKind::Other,
        "Task join error",
    )))?
}
