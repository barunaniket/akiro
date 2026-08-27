use crate::sandbox::SandboxConfig;
use std::path::Path;
use super::{LanguageRunner, SupportedLanguage};

pub struct Python;
pub struct PyPy;

impl LanguageRunner for Python {
    fn language(&self) -> SupportedLanguage {
        SupportedLanguage::Python
    }

    fn is_compiled(&self) -> bool {
        false
    }

    fn get_source_filename(&self) -> &'static str {
        "main.py"
    }

    fn get_compile_command(&self, _src_path: &Path, _bin_path: &Path) -> Option<SandboxConfig> {
        // Python doesn't need explicit compilation
        None
    }

    fn get_run_command(
        &self,
        bin_path: &Path,
        test_stdin: &[u8],
        time_limit_ms: u64,
        memory_limit_bytes: u64,
    ) -> SandboxConfig {
        // Use -B (prevent pycache) and -O (basic bytecode optimization in RAM)
        let src_str = bin_path.to_string_lossy().to_string();

        SandboxConfig::new(std::path::PathBuf::from("/usr/bin/python3"))
            .with_args(vec!["-B".to_string(), "-O".to_string(), src_str])
            .with_stdin(test_stdin.to_vec())
            .with_time_limit(time_limit_ms)
            .with_memory_limit(memory_limit_bytes)
            .with_max_output(10 * 1024 * 1024)
    }
}

impl LanguageRunner for PyPy {
    fn language(&self) -> SupportedLanguage {
        SupportedLanguage::PyPy
    }

    fn is_compiled(&self) -> bool {
        false
    }

    fn get_source_filename(&self) -> &'static str {
        "main.py"
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
        // PyPy3 JIT Engine with -B and -O
        let src_str = bin_path.to_string_lossy().to_string();

        SandboxConfig::new(std::path::PathBuf::from("/usr/bin/pypy3"))
            .with_args(vec!["-B".to_string(), "-O".to_string(), src_str])
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
    fn test_python_language_properties() {
        let python = Python;
        assert!(!python.is_compiled());
        assert_eq!(python.get_source_filename(), "main.py");
    }
}
