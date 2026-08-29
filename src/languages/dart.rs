use crate::sandbox::SandboxConfig;
use std::path::Path;
use super::{LanguageRunner, SupportedLanguage};

pub struct Dart;

impl LanguageRunner for Dart {
    fn language(&self) -> SupportedLanguage {
        SupportedLanguage::Dart
    }

    fn is_compiled(&self) -> bool {
        true
    }

    fn get_source_filename(&self) -> &'static str {
        "Solution.dart"
    }

    fn max_pids(&self) -> u32 {
        4
    }

    fn get_compile_command(&self, src_path: &Path, bin_path: &Path) -> Option<SandboxConfig> {
        Some(
            SandboxConfig::new(std::path::PathBuf::from("/usr/bin/dart"))
                .with_args(vec![
                    "compile".to_string(),
                    "exe".to_string(),
                    src_path.to_string_lossy().to_string(),
                    "-o".to_string(),
                    bin_path.to_string_lossy().to_string(),
                ])
                .with_time_limit(15000)
                .with_memory_limit(512 * 1024 * 1024),
        )
    }

    fn get_run_command(
        &self,
        bin_path: &Path,
        test_stdin: &[u8],
        time_limit_ms: u64,
        memory_limit_bytes: u64,
    ) -> SandboxConfig {
        SandboxConfig::new(bin_path.to_path_buf())
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
    fn test_dart_language_properties() {
        let runner = Dart;
        assert!(runner.is_compiled());
        assert_eq!(runner.get_source_filename(), "Solution.dart");
    }
}
