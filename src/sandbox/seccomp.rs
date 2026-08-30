use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SeccompError {
    #[error("Seccomp initialization failed: {0}")]
    InitError(String),
    #[error("Failed to add rule: {0}")]
    RuleError(String),
    #[error("Failed to load filter: {0}")]
    LoadError(String),
    #[error("Platform not supported")]
    UnsupportedPlatform,
}

pub struct SeccompProfile {
    // Informational allow-set documenting the syscalls a typical runner needs.
    // The enforced filter is default-ALLOW with an explicit deny-list (see
    // `blocked_syscall_numbers`), chosen over strict default-deny so that diverse
    // language runtimes (JVM, bun, PyPy) are not killed for a benign syscall we
    // failed to anticipate. Retained for reference and a possible future strict mode.
    #[allow(dead_code)]
    allowed_syscalls: HashMap<&'static str, i32>,
}

impl SeccompProfile {
    pub fn standard_runner() -> Self {
        let mut allowed_syscalls = HashMap::new();

        // Core I/O operations (x86_64 syscall numbers)
        allowed_syscalls.insert("read", 0);
        allowed_syscalls.insert("write", 1);
        allowed_syscalls.insert("open", 2);
        allowed_syscalls.insert("close", 3);
        allowed_syscalls.insert("lseek", 8);
        allowed_syscalls.insert("pread64", 17);
        allowed_syscalls.insert("pwrite64", 18);
        allowed_syscalls.insert("readv", 19);
        allowed_syscalls.insert("writev", 20);
        allowed_syscalls.insert("openat", 257);
        allowed_syscalls.insert("fstat", 5);
        allowed_syscalls.insert("newfstatat", 262);
        allowed_syscalls.insert("statx", 332);

        // Memory management
        allowed_syscalls.insert("brk", 12);
        allowed_syscalls.insert("mmap", 9);
        allowed_syscalls.insert("munmap", 11);
        allowed_syscalls.insert("mprotect", 10);
        allowed_syscalls.insert("mremap", 25);
        allowed_syscalls.insert("madvise", 28);
        allowed_syscalls.insert("mlock", 149);
        allowed_syscalls.insert("munlock", 150);

        // Process lifecycle
        allowed_syscalls.insert("exit", 60);
        allowed_syscalls.insert("exit_group", 231);
        allowed_syscalls.insert("rt_sigreturn", 15);
        allowed_syscalls.insert("rt_sigaction", 13);
        allowed_syscalls.insert("rt_sigprocmask", 14);
        allowed_syscalls.insert("sigaltstack", 131);

        // Timing
        allowed_syscalls.insert("clock_gettime", 228);
        allowed_syscalls.insert("gettimeofday", 96);
        allowed_syscalls.insert("nanosleep", 35);
        allowed_syscalls.insert("clock_nanosleep", 230);

        // System info (read-only)
        allowed_syscalls.insert("uname", 63);
        allowed_syscalls.insert("getpid", 39);
        allowed_syscalls.insert("getppid", 110);
        allowed_syscalls.insert("getuid", 102);
        allowed_syscalls.insert("geteuid", 107);
        allowed_syscalls.insert("getgid", 104);
        allowed_syscalls.insert("getegid", 108);
        allowed_syscalls.insert("getpgrp", 111);
        allowed_syscalls.insert("getrusage", 98);

        // IPC (limited)
        allowed_syscalls.insert("futex", 202);
        allowed_syscalls.insert("futex_waitv", 449);

        // Modern Linux
        allowed_syscalls.insert("getrandom", 318);
        allowed_syscalls.insert("rseq", 334);

        // Architecture-specific
        allowed_syscalls.insert("set_tid_address", 218);
        allowed_syscalls.insert("set_robust_list", 273);
        allowed_syscalls.insert("get_robust_list", 274);

        // Special
        allowed_syscalls.insert("prctl", 157);
        allowed_syscalls.insert("ioctl", 16);

        SeccompProfile { allowed_syscalls }
    }

    pub fn install(&self) -> Result<(), SeccompError> {
        #[cfg(target_os = "linux")]
        {
            // Set NO_NEW_PRIVS to prevent privilege escalation via setuid/setcap
            // and to permit loading a seccomp filter without CAP_SYS_ADMIN.
            unsafe {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(SeccompError::InitError("Failed to set PR_SET_NO_NEW_PRIVS".to_string()));
                }
            }

            // Compile and load a real BPF filter into the kernel.
            install_bpf_filter()?;
        }

        Ok(())
    }
}

/// Syscalls unconditionally denied (EPERM) for untrusted Run-profile code.
///
/// These are escape / tamper vectors that no legitimate compute submission needs.
/// Deliberately NOT blocked, because language runtimes depend on them:
///   - `clone`/`clone3`/`fork`/`vfork` — JVM & runtime threads, subprocess harnesses
///   - `execve`/`execveat`             — the sandbox's own launch of the program
///   - `kill`/`tgkill`                 — runtime signal handling
///   - the `socket` family             — network egress is already severed at the
///     namespace layer (CLONE_NEWNET); blocking `socket` here breaks glibc NSS /
///     `getaddrinfo` probes in some runtimes for no added isolation.
///
/// This list is the single source of truth: `blocked_syscall_numbers()` maps it to
/// arch-correct numbers via `libc::SYS_*`, and the unit tests assert its invariants.
pub fn blocked_syscalls() -> Vec<&'static str> {
    vec![
        // Process introspection / debugging
        "ptrace",
        "process_vm_readv",
        "process_vm_writev",
        // Filesystem / mount-namespace escape
        "mount",
        "umount2",
        "pivot_root",
        "chroot",
        "swapon",
        "swapoff",
        // Namespace manipulation (post-setup)
        "setns",
        "unshare",
        // Kernel module (in)security
        "init_module",
        "finit_module",
        "delete_module",
        "kexec_load",
        "reboot",
        // Privileged kernel interfaces
        "bpf",
        "perf_event_open",
        // Kernel keyring
        "add_key",
        "keyctl",
        "request_key",
        // Host clock / accounting tamper
        "settimeofday",
        "clock_settime",
        "adjtimex",
        "clock_adjtime",
        "acct",
        "quotactl",
    ]
}

/// Kernel syscall numbers for `blocked_syscalls()`, resolved per-arch by libc.
#[cfg(target_os = "linux")]
fn blocked_syscall_numbers() -> Vec<libc::c_long> {
    vec![
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_setns,
        libc::SYS_unshare,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_kexec_load,
        libc::SYS_reboot,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_add_key,
        libc::SYS_keyctl,
        libc::SYS_request_key,
        libc::SYS_settimeofday,
        libc::SYS_clock_settime,
        libc::SYS_adjtimex,
        libc::SYS_clock_adjtime,
        libc::SYS_acct,
        libc::SYS_quotactl,
    ]
}

/// Build a default-ALLOW seccomp filter that returns EPERM for each denied syscall,
/// then load it onto the current (post-fork, single) thread via
/// `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, ...)`.
#[cfg(target_os = "linux")]
fn install_bpf_filter() -> Result<(), SeccompError> {
    use seccompiler::{apply_filter, BpfProgram, SeccompAction, SeccompFilter, TargetArch};
    use std::collections::BTreeMap;

    // Empty rule vec == unconditional match on that syscall number -> match action.
    let mut rules: BTreeMap<libc::c_long, Vec<seccompiler::SeccompRule>> = BTreeMap::new();
    for nr in blocked_syscall_numbers() {
        rules.insert(nr, vec![]);
    }

    let target_arch: TargetArch = std::env::consts::ARCH
        .try_into()
        .map_err(|_| SeccompError::UnsupportedPlatform)?;

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,                     // default for everything not listed
        SeccompAction::Errno(libc::EPERM as u32), // listed syscalls -> EPERM
        target_arch,
    )
    .map_err(|e| SeccompError::RuleError(format!("{e:?}")))?;

    let program: BpfProgram = filter
        .try_into()
        .map_err(|e| SeccompError::LoadError(format!("{e:?}")))?;

    apply_filter(&program).map_err(|e| SeccompError::LoadError(format!("{e:?}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_profile_has_core_syscalls() {
        let profile = SeccompProfile::standard_runner();
        assert!(profile.allowed_syscalls.contains_key("read"));
        assert!(profile.allowed_syscalls.contains_key("write"));
        assert!(profile.allowed_syscalls.contains_key("exit_group"));
        assert!(profile.allowed_syscalls.contains_key("brk"));
        assert!(profile.allowed_syscalls.contains_key("mmap"));
    }

    #[test]
    fn test_blocked_syscalls_deny_escape_and_tamper() {
        let blocked = blocked_syscalls();
        // Filesystem / namespace escape vectors
        assert!(blocked.contains(&"mount"));
        assert!(blocked.contains(&"pivot_root"));
        assert!(blocked.contains(&"chroot"));
        assert!(blocked.contains(&"setns"));
        assert!(blocked.contains(&"unshare"));
        // Debugging / introspection
        assert!(blocked.contains(&"ptrace"));
        // Kernel tamper
        assert!(blocked.contains(&"init_module"));
        assert!(blocked.contains(&"reboot"));
        assert!(blocked.contains(&"bpf"));
    }

    #[test]
    fn test_blocked_syscalls_allow_runtime_essentials() {
        // These MUST stay allowed or language runtimes (and the launch execve
        // itself) would be killed. This is a correctness guarantee, not a nicety.
        let blocked = blocked_syscalls();
        for essential in ["clone", "clone3", "fork", "vfork", "execve", "kill", "tgkill"] {
            assert!(
                !blocked.contains(&essential),
                "{essential} must not be in the seccomp deny-list"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_blocked_names_and_numbers_agree() {
        // Name list and number list must stay the same length (kept in sync by hand).
        assert_eq!(blocked_syscalls().len(), blocked_syscall_numbers().len());
    }
}
