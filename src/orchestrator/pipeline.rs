use crate::languages::{LanguageRunner, SupportedLanguage};
use crate::sandbox::{Sandbox, SandboxStatus};
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use tokio::sync::mpsc;
use futures_util::StreamExt;

use super::job::{JobRequest, JobResult, JudgeVerdict, TestCaseResult, ProgressEvent};

pub struct ExecutionPipeline;

impl ExecutionPipeline {
    pub async fn execute(
        request: &JobRequest,
        progress: Option<mpsc::UnboundedSender<ProgressEvent>>,
    ) -> Result<JobResult, String> {
        let language = SupportedLanguage::from_str(&request.language)
            .ok_or_else(|| format!("Unsupported language: {}", request.language))?;

        let runner = language.get_runner();
        let temp_dir = TempDir::new()
            .map_err(|e| format!("Failed to create temp directory: {}", e))?;

        let job_id = request.job_id.clone();
        let mut result = JobResult {
            job_id: job_id.clone(),
            verdict: JudgeVerdict::Accepted,
            total_cpu_time_ms: 0,
            peak_memory_kb: 0,
            compile_output: None,
            test_results: Vec::new(),
        };

        // Step 1: Write source code
        let src_path = temp_dir.path().join(runner.get_source_filename());
        fs::write(&src_path, &request.source_code)
            .map_err(|e| format!("Failed to write source: {}", e))?;

        // Step 2: Compile (if needed)
        if runner.is_compiled() {
            let _ = progress.as_ref().map(|p| p.send(ProgressEvent::Compiling));

            let bin_path = temp_dir.path().join("binary");

            if let Some(compile_config) = runner.get_compile_command(&src_path, &bin_path) {
                let compile_config = compile_config
                    .with_profile(crate::sandbox::config::ExecutionProfile::Compile)
                    .with_fs_isolation(false)
                    .with_network_isolation(false)
                    .with_pids_limit(128)
                    .with_work_dir(temp_dir.path().to_path_buf());

                let compile_result = Sandbox::execute(compile_config)
                    .await
                    .map_err(|e| format!("Sandbox error during compilation: {}", e))?;

                if compile_result.status != SandboxStatus::Ok {
                    result.verdict = JudgeVerdict::CompilationError;
                    result.compile_output =
                        Some(String::from_utf8_lossy(&compile_result.stderr).to_string());
                    let _ = progress.as_ref().map(|p| p.send(ProgressEvent::Finished {
                        verdict: JudgeVerdict::CompilationError
                    }));
                    return Ok(result);
                }

                result.total_cpu_time_ms += compile_result.cpu_time_ms;
                result.peak_memory_kb = result.peak_memory_kb.max(compile_result.memory_kb);
            }

            // Run test cases against compiled binary
            result = Self::run_tests(&runner, temp_dir.path(), request, result, progress.clone()).await?;
        } else {
            // For interpreted languages, run directly
            result = Self::run_tests(&runner, temp_dir.path(), request, result, progress.clone()).await?;
        }

        Ok(result)
    }

    async fn run_tests(
        runner: &Box<dyn LanguageRunner>,
        work_dir: &Path,
        request: &JobRequest,
        mut result: JobResult,
        progress: Option<mpsc::UnboundedSender<ProgressEvent>>,
    ) -> Result<JobResult, String> {
        // Path inside the sandbox jail after pivot_root
        let sandbox_bin_path = if runner.is_compiled() {
            Path::new("/sandbox").join("binary")
        } else {
            Path::new("/sandbox").join(runner.get_source_filename())
        };

        // Determine concurrent workers for test case evaluation (bounded to available parallelism)
        let concurrency = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4)
            .min(16);

        let total_tests = request.test_cases.len();

        let mut test_futs = Vec::with_capacity(total_tests);

        for (idx, test_case) in request.test_cases.iter().enumerate() {
            let run_config = runner.get_run_command(
                &sandbox_bin_path,
                test_case.input.as_bytes(),
                request.time_limit_ms,
                request.memory_limit_bytes,
            )
            .with_profile(crate::sandbox::config::ExecutionProfile::Run)
            .with_fs_isolation(true)
            .with_workspace_dir(work_dir.to_path_buf())
            .with_work_dir(std::path::PathBuf::from("/sandbox"))
            .with_network_isolation(true)
            .with_pids_limit(runner.max_pids());

            let expected = test_case.expected_output.clone();
            let time_limit_ms = request.time_limit_ms;
            let memory_limit_bytes = request.memory_limit_bytes;

            test_futs.push(async move {
                let exec_result = Sandbox::execute(run_config).await;
                (idx, expected, time_limit_ms, memory_limit_bytes, exec_result)
            });
        }

        let mut stream = futures_util::stream::iter(test_futs).buffer_unordered(concurrency);
        let mut raw_results = Vec::with_capacity(total_tests);

        let stop_on_fail = request.stop_on_first_fail.unwrap_or(false);

        while let Some((idx, expected, time_limit_ms, memory_limit_bytes, exec_res)) = stream.next().await {
            let exec_result = exec_res.map_err(|e| format!("Sandbox error during execution: {}", e))?;
            let memory_limit_kb = memory_limit_bytes / 1024;

            let verdict = match exec_result.status {
                SandboxStatus::Ok => {
                    if memory_limit_kb > 0 && exec_result.memory_kb > memory_limit_kb {
                        JudgeVerdict::MemoryLimitExceeded
                    } else if let Some(expected_str) = &expected {
                        if trim_bytes(&exec_result.stdout) == trim_bytes(expected_str.as_bytes()) {
                            JudgeVerdict::Accepted
                        } else {
                            JudgeVerdict::WrongAnswer
                        }
                    } else {
                        JudgeVerdict::Accepted
                    }
                }
                SandboxStatus::TimeLimitExceeded => JudgeVerdict::TimeLimitExceeded,
                SandboxStatus::MemoryLimitExceeded => JudgeVerdict::MemoryLimitExceeded,
                _ => {
                    if memory_limit_kb > 0 && exec_result.memory_kb > memory_limit_kb {
                        JudgeVerdict::MemoryLimitExceeded
                    } else if exec_result.cpu_time_ms >= time_limit_ms {
                        JudgeVerdict::TimeLimitExceeded
                    } else {
                        JudgeVerdict::RuntimeError
                    }
                }
            };

            let _ = progress.as_ref().map(|p| p.send(ProgressEvent::TestResult {
                test_case: idx + 1,
                verdict,
                time_ms: exec_result.cpu_time_ms,
                memory_kb: exec_result.memory_kb,
            }));

            raw_results.push((idx, verdict, exec_result));

            if stop_on_fail && verdict != JudgeVerdict::Accepted {
                // Short-circuit immediately on first failing test case
                break;
            }
        }

        // Sort results by original test case index so order is strictly deterministic
        raw_results.sort_by_key(|r| r.0);

        for (idx, verdict, exec_result) in raw_results {
            result.total_cpu_time_ms += exec_result.cpu_time_ms;
            result.peak_memory_kb = result.peak_memory_kb.max(exec_result.memory_kb);

            result.test_results.push(TestCaseResult {
                test_case_index: idx,
                status: verdict,
                cpu_time_ms: exec_result.cpu_time_ms,
                memory_kb: exec_result.memory_kb,
                stdout: exec_result.stdout,
                stderr: exec_result.stderr,
            });

            if verdict != JudgeVerdict::Accepted && result.verdict == JudgeVerdict::Accepted {
                result.verdict = verdict;
            }
        }

        if result.test_results.iter().all(|r| r.status == JudgeVerdict::Accepted) {
            result.verdict = JudgeVerdict::Accepted;
        }

        let passed = result.test_results.iter().filter(|t| t.status == JudgeVerdict::Accepted).count();
        let failed = result.test_results.len().saturating_sub(passed);
        crate::metrics::record_job(&request.language, &result.verdict, result.total_cpu_time_ms, passed, failed);

        let _ = progress.as_ref().map(|p| p.send(ProgressEvent::Finished {
            verdict: result.verdict,
        }));

        Ok(result)
    }
}

/// Zero-allocation byte-level whitespace trimming (avoids UTF-8 decode + String alloc)
fn trim_bytes(b: &[u8]) -> &[u8] {
    let start = b.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !c.is_ascii_whitespace()).map_or(start, |p| p + 1);
    &b[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_creation() {
        // Placeholder test - full integration requires sandbox environment
        let request = JobRequest::new(
            "cpp".to_string(),
            "int main(){}".to_string(),
            1000,
            256 * 1024 * 1024,
            vec![],
        );

        assert_eq!(request.language, "cpp");
        assert!(!request.job_id.is_empty());
    }
}
