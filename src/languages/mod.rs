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
pub mod dart;
pub mod scala;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    Dart,
    Scala,
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
            SupportedLanguage::Dart => Box::new(dart::Dart),
            SupportedLanguage::Scala => Box::new(scala::Scala),
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
            "dart" => Some(SupportedLanguage::Dart),
            "scala" | "scalac" => Some(SupportedLanguage::Scala),
            "javascript" | "js" => Some(SupportedLanguage::JavaScript),
            "typescript" | "ts" => Some(SupportedLanguage::TypeScript),
            "sql" | "sqlite" | "sqlite3" => Some(SupportedLanguage::Sql),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SupportedLanguage::C => "c",
            SupportedLanguage::Cpp => "cpp",
            SupportedLanguage::Rust => "rust",
            SupportedLanguage::Go => "go",
            SupportedLanguage::Python => "python",
            SupportedLanguage::PyPy => "pypy",
            SupportedLanguage::Java => "java",
            SupportedLanguage::Kotlin => "kotlin",
            SupportedLanguage::CSharp => "csharp",
            SupportedLanguage::Zig => "zig",
            SupportedLanguage::Ruby => "ruby",
            SupportedLanguage::Php => "php",
            SupportedLanguage::Haskell => "haskell",
            SupportedLanguage::Dart => "dart",
            SupportedLanguage::Scala => "scala",
            SupportedLanguage::JavaScript => "javascript",
            SupportedLanguage::TypeScript => "typescript",
            SupportedLanguage::Sql => "sql",
        }
    }

    pub fn all_canonical_names() -> &'static [&'static str] {
        &[
            "c", "cpp", "rust", "go", "python", "pypy", "java", "kotlin",
            "csharp", "zig", "ruby", "php", "haskell", "dart", "scala",
            "javascript", "typescript", "sql",
        ]
    }

    pub fn parse_whitelist(input: &str) -> std::collections::HashSet<Self> {
        let mut set = std::collections::HashSet::new();
        for item in input.split(',') {
            let trimmed = item.trim();
            if !trimmed.is_empty() {
                if let Some(lang) = Self::from_str(trimmed) {
                    set.insert(lang);
                }
            }
        }
        set
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

    #[test]
    fn test_language_parse_whitelist() {
        let whitelist = SupportedLanguage::parse_whitelist("cpp, python, java, rust");
        assert_eq!(whitelist.len(), 4);
        assert!(whitelist.contains(&SupportedLanguage::Cpp));
        assert!(whitelist.contains(&SupportedLanguage::Python));
        assert!(whitelist.contains(&SupportedLanguage::Java));
        assert!(whitelist.contains(&SupportedLanguage::Rust));
        assert!(!whitelist.contains(&SupportedLanguage::Haskell));
        assert!(!whitelist.contains(&SupportedLanguage::Ruby));
    }
}
