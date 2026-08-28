use crate::sandbox::SandboxConfig;
use std::path::Path;
use super::{LanguageRunner, SupportedLanguage};

pub struct CSharp;

impl LanguageRunner for CSharp {
    fn language(&self) -> SupportedLanguage {
        SupportedLanguage::CSharp
    }

    fn is_compiled(&self) -> bool {
        true
    }

    fn get_source_filename(&self) -> &'static str {
        "Solution.cs"
    }

    fn max_pids(&self) -> u32 {
        16
    }

    fn get_compile_command(&self, src_path: &Path, bin_path: &Path) -> Option<SandboxConfig> {
        let src_str = src_path.to_string_lossy().to_string();
        let bin_str = bin_path.to_string_lossy().to_string();

        Some(
            SandboxConfig::new(std::path::PathBuf::from("/usr/bin/mono-csc"))
                .with_args(vec![
                    "-optimize+".to_string(),
                    "-nologo".to_string(),
                    format!("-out:{}", bin_str),
                    src_str,
                ])
                .with_time_limit(10000)
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
        SandboxConfig::new(std::path::PathBuf::from("/usr/bin/mono"))
            .with_args(vec![
                "--gc=sgen".to_string(),
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
    fn test_csharp_language_properties() {
        let cs = CSharp;
        assert!(cs.is_compiled());
        assert_eq!(cs.get_source_filename(), "Solution.cs");
    }
}
