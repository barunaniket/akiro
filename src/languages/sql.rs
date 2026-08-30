use crate::sandbox::SandboxConfig;
use std::path::Path;
use super::{LanguageRunner, SupportedLanguage};

pub struct Sql;

impl LanguageRunner for Sql {
    fn language(&self) -> SupportedLanguage {
        SupportedLanguage::Sql
    }

    fn is_compiled(&self) -> bool {
        false
    }

    fn get_source_filename(&self) -> &'static str {
        "solution.sql"
    }

    fn max_pids(&self) -> u32 {
        // NOT 2: although SQLite itself is single-threaded, `get_run_command` wraps it in a
        // shell PIPELINE (`sh -c '( ...cat... ) | sqlite3'`) that needs ~5 concurrent processes
        // (sh + subshell + cat(s) + sqlite3 + test builtins). A cap of 2 makes fork() fail
        // intermittently ("sh: Cannot fork") → spurious RuntimeError. 16 gives comfortable
        // headroom while still bounding a fork bomb.
        16
    }

    fn get_compile_command(&self, _src_path: &Path, _bin_path: &Path) -> Option<SandboxConfig> {
        None
    }

    fn get_run_command(
        &self,
        _bin_path: &Path,
        test_stdin: &[u8],
        time_limit_ms: u64,
        memory_limit_bytes: u64,
    ) -> SandboxConfig {
        // Pipes (optional init.sql) -> solution.sql -> (optional verify.sql) -> (optional stdin) into in-memory SQLite3
        let cmd = r#"
            (
                if [ -f /sandbox/init.sql ]; then cat /sandbox/init.sql; fi
                cat /sandbox/solution.sql
                if [ -f /sandbox/verify.sql ]; then cat /sandbox/verify.sql; fi
                cat -
            ) | sqlite3 -header -csv :memory:
        "#;

        SandboxConfig::new(std::path::PathBuf::from("/bin/sh"))
            .with_args(vec!["-c".to_string(), cmd.to_string()])
            .with_stdin(test_stdin.to_vec())
            .with_time_limit(time_limit_ms)
            .with_memory_limit(memory_limit_bytes)
            .with_max_output(10 * 1024 * 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_properties() {
        let sql = Sql;
        assert!(!sql.is_compiled());
        assert_eq!(sql.get_source_filename(), "solution.sql");
        assert_eq!(sql.max_pids(), 2);
    }
}
