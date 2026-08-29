use crate::sandbox::SandboxConfig;
use std::path::Path;
use super::{LanguageRunner, SupportedLanguage};

pub struct Scala;

impl LanguageRunner for Scala {
    fn language(&self) -> SupportedLanguage {
        SupportedLanguage::Scala
    }

    fn is_compiled(&self) -> bool {
        true
    }

    fn get_source_filename(&self) -> &'static str {
        "Solution.scala"
    }

    fn max_pids(&self) -> u32 {
        16
    }

    fn get_compile_command(&self, src_path: &Path, bin_path: &Path) -> Option<SandboxConfig> {
        let dest_dir = bin_path.parent().unwrap_or(Path::new("/sandbox"));
        Some(
            SandboxConfig::new(std::path::PathBuf::from("/usr/bin/scalac"))
                .with_args(vec![
                    "-d".to_string(),
                    dest_dir.to_string_lossy().to_string(),
                    src_path.to_string_lossy().to_string(),
                ])
                .with_time_limit(20000)
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
        let dest_dir = bin_path.parent().unwrap_or(Path::new("/sandbox"));
        let classpath = format!("{}:/usr/share/java/scala-library.jar", dest_dir.display());
        SandboxConfig::new(std::path::PathBuf::from("/usr/bin/java"))
            .with_args(vec![
                "-cp".to_string(),
                classpath,
                "-XX:+TieredCompilation".to_string(),
                "-XX:TieredStopAtLevel=1".to_string(),
                "-XX:+UseSerialGC".to_string(),
                "-Xms16m".to_string(),
                "-Xmx128m".to_string(),
                "Solution".to_string(),
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
    fn test_scala_language_properties() {
        let runner = Scala;
        assert!(runner.is_compiled());
        assert_eq!(runner.get_source_filename(), "Solution.scala");
    }
}
