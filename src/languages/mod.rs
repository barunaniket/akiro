use std::path::Path;
use crate::sandbox::SandboxConfig;

pub mod c_cpp;
pub mod rust;
pub mod python;
pub mod java;
pub mod golang;
pub mod javascript;
pub mod sql;
pub mod kotlin;
pub mod csharp;
pub mod zig;
pub mod ruby;
pub mod php;
pub mod haskell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedLanguage {
    C,
    Cpp,
    Rust,
    Go,
    Python,
    PyPy,
    Java,
    Kotlin,
    CSharp,
    Zig,
    Ruby,
    Php,
    Haskell,
    JavaScript,
    TypeScript,
    Sql,
}

impl SupportedLanguage {
    pub fn get_runner(&self) -> Box<dyn LanguageRunner> {
        match self {
            SupportedLanguage::C => Box::new(c_cpp::C),
            SupportedLanguage::Cpp => Box::new(c_cpp::Cpp),
            SupportedLanguage::Rust => Box::new(rust::Rust),
            SupportedLanguage::Go => Box::new(golang::Go),
            SupportedLanguage::Python => Box::new(python::Python),
            SupportedLanguage::PyPy => Box::new(python::PyPy),
            SupportedLanguage::Java => Box::new(java::Java),
            SupportedLanguage::Kotlin => Box::new(kotlin::Kotlin),
            SupportedLanguage::CSharp => Box::new(csharp::CSharp),
            SupportedLanguage::Zig => Box::new(zig::Zig),
            SupportedLanguage::Ruby => Box::new(ruby::Ruby),
            SupportedLanguage::Php => Box::new(php::Php),
            SupportedLanguage::Haskell => Box::new(haskell::Haskell),
            SupportedLanguage::JavaScript => Box::new(javascript::JavaScript),
            SupportedLanguage::TypeScript => Box::new(javascript::TypeScript),
            SupportedLanguage::Sql => Box::new(sql::Sql),
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "c" => Some(SupportedLanguage::C),
            "cpp" | "c++" => Some(SupportedLanguage::Cpp),
            "rust" | "rs" => Some(SupportedLanguage::Rust),
            "go" | "golang" => Some(SupportedLanguage::Go),
            "python" | "py" | "python3" | "cpython" => Some(SupportedLanguage::Python),
            "pypy" | "pypy3" => Some(SupportedLanguage::PyPy),
            "java" => Some(SupportedLanguage::Java),
            "kotlin" | "kt" => Some(SupportedLanguage::Kotlin),
            "csharp" | "cs" | "c#" => Some(SupportedLanguage::CSharp),
            "zig" => Some(SupportedLanguage::Zig),
            "ruby" | "rb" => Some(SupportedLanguage::Ruby),
            "php" => Some(SupportedLanguage::Php),
            "haskell" | "hs" | "ghc" => Some(SupportedLanguage::Haskell),
            "javascript" | "js" => Some(SupportedLanguage::JavaScript),
            "typescript" | "ts" => Some(SupportedLanguage::TypeScript),
            "sql" | "sqlite" | "sqlite3" => Some(SupportedLanguage::Sql),
            _ => None,
        }
    }
}

pub trait LanguageRunner: Send + Sync {
    fn language(&self) -> SupportedLanguage;
    fn is_compiled(&self) -> bool;
    fn get_source_filename(&self) -> &'static str;
    fn max_pids(&self) -> u32 {
        2
    }
    fn get_compile_command(&self, src_path: &Path, bin_path: &Path) -> Option<SandboxConfig>;
    fn get_run_command(
        &self,
        bin_path: &Path,
        test_stdin: &[u8],
        time_limit_ms: u64,
        memory_limit_bytes: u64,
    ) -> SandboxConfig;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_from_str() {
        assert_eq!(SupportedLanguage::from_str("c"), Some(SupportedLanguage::C));
        assert_eq!(
            SupportedLanguage::from_str("cpp"),
            Some(SupportedLanguage::Cpp)
        );
        assert_eq!(
            SupportedLanguage::from_str("rust"),
            Some(SupportedLanguage::Rust)
        );
        assert_eq!(
            SupportedLanguage::from_str("python"),
            Some(SupportedLanguage::Python)
        );
        assert_eq!(SupportedLanguage::from_str("java"), Some(SupportedLanguage::Java));
        assert_eq!(SupportedLanguage::from_str("go"), Some(SupportedLanguage::Go));
        assert_eq!(SupportedLanguage::from_str("js"), Some(SupportedLanguage::JavaScript));
        assert_eq!(SupportedLanguage::from_str("ts"), Some(SupportedLanguage::TypeScript));
        assert_eq!(SupportedLanguage::from_str("sql"), Some(SupportedLanguage::Sql));
    }

    #[test]
    fn test_language_get_runner() {
        let c_runner = SupportedLanguage::C.get_runner();
        assert!(c_runner.is_compiled());

        let python_runner = SupportedLanguage::Python.get_runner();
        assert!(!python_runner.is_compiled());

        let js_runner = SupportedLanguage::JavaScript.get_runner();
        assert!(!js_runner.is_compiled());

        let sql_runner = SupportedLanguage::Sql.get_runner();
        assert!(!sql_runner.is_compiled());
    }
}
