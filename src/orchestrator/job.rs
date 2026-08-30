use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub input: String,
    pub expected_output: Option<String>,
}

fn default_job_id() -> String {
    Uuid::new_v4().to_string()
}

fn default_time_limit_ms() -> u64 {
    2000
}

fn default_memory_limit_bytes() -> u64 {
    256 * 1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRequest {
    #[serde(default = "default_job_id")]
    pub job_id: String,
    pub language: String,
    pub source_code: String,
    #[serde(default = "default_time_limit_ms")]
    pub time_limit_ms: u64,
    #[serde(default = "default_memory_limit_bytes")]
    pub memory_limit_bytes: u64,
    pub test_cases: Vec<TestCase>,
    #[serde(default)]
    pub stop_on_first_fail: Option<bool>,
}

/// Hard ceilings applied to every submission before it is queued or sandboxed.
/// These bound memory pressure in the async layer (bodies are already capped at 2 MB,
/// but a single body can still fan out into many large testcases) independent of the
/// per-job sandbox limits.
pub const MAX_TEST_CASES: usize = 100;
pub const MAX_TESTCASE_BYTES: usize = 512 * 1024; // 512 KB per input / expected_output
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024; // 1 MB of source
pub const MAX_TIME_LIMIT_MS: u64 = 60_000; // 60 s wall ceiling
pub const MAX_MEMORY_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2 GB ceiling

impl JobRequest {
    /// Validate a submission's shape and size before it consumes worker/sandbox resources.
    /// Returns a human-readable error string on the first violation.
    pub fn validate(&self) -> Result<(), String> {
        if self.source_code.len() > MAX_SOURCE_BYTES {
            return Err(format!(
                "source_code is {} bytes (max {})",
                self.source_code.len(),
                MAX_SOURCE_BYTES
            ));
        }
        if self.test_cases.is_empty() {
            return Err("at least one test case is required".to_string());
        }
        if self.test_cases.len() > MAX_TEST_CASES {
            return Err(format!(
                "too many test cases: {} (max {})",
                self.test_cases.len(),
                MAX_TEST_CASES
            ));
        }
        for (i, tc) in self.test_cases.iter().enumerate() {
            if tc.input.len() > MAX_TESTCASE_BYTES {
                return Err(format!(
                    "test case {} input is {} bytes (max {})",
                    i,
                    tc.input.len(),
                    MAX_TESTCASE_BYTES
                ));
            }
            if let Some(expected) = &tc.expected_output {
                if expected.len() > MAX_TESTCASE_BYTES {
                    return Err(format!(
                        "test case {} expected_output is {} bytes (max {})",
                        i,
                        expected.len(),
                        MAX_TESTCASE_BYTES
                    ));
                }
            }
        }
        if self.time_limit_ms == 0 || self.time_limit_ms > MAX_TIME_LIMIT_MS {
            return Err(format!(
                "time_limit_ms {} out of range (1..={})",
                self.time_limit_ms, MAX_TIME_LIMIT_MS
            ));
        }
        if self.memory_limit_bytes == 0 || self.memory_limit_bytes > MAX_MEMORY_LIMIT_BYTES {
            return Err(format!(
                "memory_limit_bytes {} out of range (1..={})",
                self.memory_limit_bytes, MAX_MEMORY_LIMIT_BYTES
            ));
        }
        Ok(())
    }

    pub fn new(
        language: String,
        source_code: String,
        time_limit_ms: u64,
        memory_limit_bytes: u64,
        test_cases: Vec<TestCase>,
    ) -> Self {
        Self {
            job_id: Uuid::new_v4().to_string(),
            language,
            source_code,
            time_limit_ms,
            memory_limit_bytes,
            test_cases,
            stop_on_first_fail: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JudgeVerdict {
    Accepted,
    WrongAnswer,
    TimeLimitExceeded,
    MemoryLimitExceeded,
    RuntimeError,
    CompilationError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProgressEvent {
    Compiling,
    Running { test_case: usize, total: usize },
    TestResult {
        test_case: usize,
        verdict: JudgeVerdict,
        time_ms: u64,
        memory_kb: u64,
    },
    Finished { verdict: JudgeVerdict },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCaseResult {
    pub test_case_index: usize,
    pub status: JudgeVerdict,
    pub cpu_time_ms: u64,
    pub memory_kb: u64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResult {
    pub job_id: String,
    pub verdict: JudgeVerdict,
    pub total_cpu_time_ms: u64,
    pub peak_memory_kb: u64,
    pub compile_output: Option<String>,
    pub test_results: Vec<TestCaseResult>,
}

impl JobResult {
    pub fn compilation_error(job_id: String, error_message: String) -> Self {
        Self {
            job_id,
            verdict: JudgeVerdict::CompilationError,
            total_cpu_time_ms: 0,
            peak_memory_kb: 0,
            compile_output: Some(error_message),
            test_results: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_request_creation() {
        let req = JobRequest::new(
            "cpp".to_string(),
            "int main(){}".to_string(),
            1000,
            256 * 1024 * 1024,
            vec![TestCase {
                input: "1 2".to_string(),
                expected_output: Some("3".to_string()),
            }],
        );

        assert_eq!(req.language, "cpp");
        assert_eq!(req.time_limit_ms, 1000);
        assert_eq!(req.test_cases.len(), 1);
    }

    #[test]
    fn test_validate_accepts_normal_request() {
        let req = JobRequest::new(
            "python".to_string(),
            "print(1)".to_string(),
            2000,
            256 * 1024 * 1024,
            vec![TestCase { input: "1".to_string(), expected_output: Some("1".to_string()) }],
        );
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_validate_rejects_too_many_testcases() {
        let mut req = JobRequest::new(
            "python".to_string(),
            "print(1)".to_string(),
            2000,
            256 * 1024 * 1024,
            vec![],
        );
        req.test_cases = (0..(MAX_TEST_CASES + 1))
            .map(|_| TestCase { input: "x".to_string(), expected_output: None })
            .collect();
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_oversized_input_and_empty() {
        let big = "a".repeat(MAX_TESTCASE_BYTES + 1);
        let req = JobRequest::new(
            "python".to_string(),
            "print(1)".to_string(),
            2000,
            256 * 1024 * 1024,
            vec![TestCase { input: big, expected_output: None }],
        );
        assert!(req.validate().is_err());

        let empty = JobRequest::new("python".to_string(), "x".to_string(), 2000, 1024, vec![]);
        assert!(empty.validate().is_err());
    }

    #[test]
    fn test_compilation_error_result() {
        let result = JobResult::compilation_error(
            "job-123".to_string(),
            "error: expected ';'".to_string(),
        );

        assert_eq!(result.verdict, JudgeVerdict::CompilationError);
        assert!(result.compile_output.is_some());
    }
}
