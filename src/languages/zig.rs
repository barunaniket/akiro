use crate::sandbox::SandboxConfig;
use std::path::Path;
use super::{LanguageRunner, SupportedLanguage};

pub struct Zig;

impl LanguageRunner for Zig {
    fn language(&self) -> SupportedLanguage {
        SupportedLanguage::Zig
    }

    fn is_compiled(&self) -> bool {
        true
    }

    fn get_source_filename(&self) -> &'static str {
        "Solution.zig"
    }

    fn max_pids(&self) -> u32 {
        2
    }

    fn get_compile_command(&self, src_path: &Path, bin_path: &Path) -> Option<SandboxConfig> {
        let src_str = src_path.to_string_lossy().to_string();
        let bin_str = bin_path.to_string_lossy().to_string();

        Some(
            SandboxConfig::new(std::path::PathBuf::from("/usr/local/bin/zig"))
                .with_args(vec![
                    "build-exe".to_string(),
                    src_str,
                    "-O".to_string(),
                    "ReleaseFast".to_string(),
                    format!("-femit-bin={}", bin_str),
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
    fn test_zig_language_properties() {
        let zig = Zig;
        assert!(zig.is_compiled());
        assert_eq!(zig.get_source_filename(), "Solution.zig");
    }
}
