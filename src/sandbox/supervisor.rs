use libc::{c_int, wait4, WIFEXITED, WIFSIGNALED, WEXITSTATUS, WTERMSIG, SIGKILL};
use std::io::Read;
use std::os::unix::io::FromRawFd;
use std::time::Instant;
use tokio::task;
use tokio::time::{sleep, Duration};
use nix::unistd::Pid;
use nix::sys::signal::{kill, Signal};
use thiserror::Error;

use crate::sandbox::config::SandboxConfig;
use crate::sandbox::result::{ExecutionResult, SandboxStatus};
use crate::sandbox::cgroups::CgroupManager;

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
}

impl ProcessSupervisor {
    pub fn new(pid: Pid, config: SandboxConfig, cgroup: Option<CgroupManager>) -> Self {
        Self {
            pid,
            config,
            start_time: Instant::now(),
            cgroup,
        }
    }

    pub async fn supervise(&self) -> Result<ExecutionResult, SupervisorError> {
        let wall_time_deadline = self.config.wall_time_limit_ms;

        // Try event-driven supervision via pidfd_open (Linux 5.3+)
        let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, self.pid.as_raw(), 0 as c_int) } as c_int;

        if pidfd >= 0 {
            // Event-driven: zero-latency child exit detection
            let async_fd = match tokio::io::unix::AsyncFd::new(pidfd) {
                Ok(fd) => fd,
                Err(_) => {
                    unsafe { libc::close(pidfd); }
                    return self.supervise_polling(wall_time_deadline).await;
                }
            };

            tokio::select! {
                _ = async_fd.readable() => {
                    // Child exited — instant wakeup (0ms latency)
                    unsafe { libc::close(pidfd); }
                    self.kill_process_tree();
                    return self.wait_for_child();
                }
                _ = sleep(Duration::from_millis(wall_time_deadline)) => {
                    // Time limit exceeded
                    unsafe { libc::close(pidfd); }
                    self.kill_process_tree();
                    return self.wait_for_child();
                }
            }
        } else {
            // Fallback for older kernels: polling with reduced interval
            self.supervise_polling(wall_time_deadline).await
        }
    }

    async fn supervise_polling(&self, wall_time_deadline: u64) -> Result<ExecutionResult, SupervisorError> {
        loop {
            let elapsed_ms = self.start_time.elapsed().as_millis() as u64;

            if elapsed_ms >= wall_time_deadline {
                self.kill_process_tree();
                break;
            }

            sleep(Duration::from_millis(5)).await; // 5ms poll (reduced from 20ms)

            if let Some(result) = self.try_wait()? {
                self.kill_process_tree();
                return Ok(result);
            }
        }

        self.kill_process_tree();
        self.wait_for_child()
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

    fn try_wait(&self) -> Result<Option<ExecutionResult>, SupervisorError> {
        let mut status: c_int = 0;
        let mut rusage: libc::rusage = unsafe { std::mem::zeroed() };

        unsafe {
            let ret = wait4(self.pid.as_raw(), &mut status, libc::WNOHANG, &mut rusage);
            if ret == -1 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ECHILD) {
                    return Ok(Some(self.build_result(0, rusage)));
                }
                return Err(SupervisorError::Wait4Error(format!("wait4 failed: {}", err)));
            }
            if ret == 0 {
                return Ok(None);
            }
        }

        Ok(Some(self.build_result(status, rusage)))
    }

    fn wait_for_child(&self) -> Result<ExecutionResult, SupervisorError> {
        let mut status: c_int = 0;
        let mut rusage: libc::rusage = unsafe { std::mem::zeroed() };

        for _ in 0..50 {
            unsafe {
                let ret = wait4(self.pid.as_raw(), &mut status, libc::WNOHANG, &mut rusage);
                if ret > 0 {
                    return Ok(self.build_result(status, rusage));
                }
                if ret == -1 {
                    let err = std::io::Error::last_os_error();
                    if err.raw_os_error() == Some(libc::ECHILD) {
                        return Ok(self.build_result(0, rusage));
                    }
                    return Err(SupervisorError::Wait4Error(format!("wait4 failed: {}", err)));
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        Ok(self.build_result(libc::SIGKILL, rusage))
    }

    fn build_result(&self, status: c_int, rusage: libc::rusage) -> ExecutionResult {
        let wall_time_ms = self.start_time.elapsed().as_millis() as u64;

        let cpu_time_ms = {
            let user_us = rusage.ru_utime.tv_sec * 1_000_000 + rusage.ru_utime.tv_usec;
            let sys_us = rusage.ru_stime.tv_sec * 1_000_000 + rusage.ru_stime.tv_usec;
            ((user_us + sys_us) / 1000) as u64
        };

        // Try to get memory from cgroup first (more accurate for physical memory)
        // Fall back to getrusage if cgroup is not available
        let memory_kb = if let Some(ref cgroup) = self.cgroup {
            if let Ok(stats) = cgroup.read_stats() {
                stats.memory_peak_bytes / 1024
            } else {
                rusage.ru_maxrss as u64
            }
        } else {
            rusage.ru_maxrss as u64
        };

        let exit_code = WEXITSTATUS(status) as i32;
        let memory_limit_kb = self.config.memory_limit_bytes / 1024;

        let sandbox_status = if WIFEXITED(status) {
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
            if ret <= 0 {
                break;
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
