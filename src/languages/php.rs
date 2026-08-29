use crate::sandbox::SandboxConfig;
use std::path::Path;
use super::{LanguageRunner, SupportedLanguage};

pub struct Php;

impl LanguageRunner for Php {
    fn language(&self) -> SupportedLanguage {
        SupportedLanguage::Php
    }

    fn is_compiled(&self) -> bool {
        false
    }

    fn get_source_filename(&self) -> &'static str {
        "Solution.php"
    }

    fn max_pids(&self) -> u32 {
        4
    }

    fn get_compile_command(&self, _src_path: &Path, _bin_path: &Path) -> Option<SandboxConfig> {
        None
    }

    fn get_run_command(
        &self,
        bin_path: &Path,
        test_stdin: &[u8],
        time_limit_ms: u64,
        memory_limit_bytes: u64,
    ) -> SandboxConfig {
        SandboxConfig::new(std::path::PathBuf::from("/usr/bin/php"))
            .with_args(vec![
                "-f".to_string(),
                bin_path.to_string_lossy().to_string(),
            ])
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
    fn test_php_language_properties() {
        let runner = Php;
        assert!(!runner.is_compiled());
        assert_eq!(runner.get_source_filename(), "Solution.php");
    }
}
