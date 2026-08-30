use crate::sandbox::SandboxConfig;
use std::path::Path;
use super::{LanguageRunner, SupportedLanguage};

pub struct Kotlin;

impl LanguageRunner for Kotlin {
    fn language(&self) -> SupportedLanguage {
        SupportedLanguage::Kotlin
    }

    fn is_compiled(&self) -> bool {
        true
    }

    fn get_source_filename(&self) -> &'static str {
        "Solution.kt"
    }

    fn max_pids(&self) -> u32 {
        24
    }

    fn get_compile_command(&self, src_path: &Path, _bin_path: &Path) -> Option<SandboxConfig> {
        let src_str = src_path.to_string_lossy().to_string();
        let jar_path = src_path.parent().unwrap_or(Path::new(".")).join("Solution.jar").to_string_lossy().to_string();

        Some(
            SandboxConfig::new(std::path::PathBuf::from("/usr/bin/java"))
                .with_args(vec![
                    "-Xmx384m".to_string(),
                    "-XX:+UseSerialGC".to_string(),
                    "-XX:TieredStopAtLevel=1".to_string(),
                    "-cp".to_string(),
                    "/opt/kotlinc/lib/kotlin-compiler.jar".to_string(),
                    "org.jetbrains.kotlin.cli.jvm.K2JVMCompiler".to_string(),
                    src_str,
                    "-d".to_string(),
                    jar_path,
                    "-nowarn".to_string(),
                ])
                .with_time_limit(15000)
                .with_memory_limit(512 * 1024 * 1024),
        )
    }

    fn get_run_command(
        &self,
        _bin_path: &Path,
        test_stdin: &[u8],
        time_limit_ms: u64,
        memory_limit_bytes: u64,
    ) -> SandboxConfig {
        let mem_mb = memory_limit_bytes / (1024 * 1024);

        SandboxConfig::new(std::path::PathBuf::from("/usr/bin/java"))
            .with_args(vec![
                format!("-Xmx{}m", mem_mb),
                format!("-Xms{}m", mem_mb / 2),
                "-Xss1m".to_string(),
                "-XX:+UseSerialGC".to_string(),
                "-XX:TieredStopAtLevel=1".to_string(),
                "-XX:ActiveProcessorCount=1".to_string(),
                "-cp".to_string(),
                "/sandbox/Solution.jar:/opt/kotlinc/lib/kotlin-stdlib.jar".to_string(),
                "SolutionKt".to_string(),
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
    fn test_kotlin_language_properties() {
        let kt = Kotlin;
        assert!(kt.is_compiled());
        assert_eq!(kt.get_source_filename(), "Solution.kt");
    }
}
