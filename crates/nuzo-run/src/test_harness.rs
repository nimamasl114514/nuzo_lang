//! TestHarness — automatic test discovery, execution with timeout, and reporting.
//!
//! Scans directories for `*.nuzo` files, runs each in an isolated Session,
//! and reports pass/fail/timeout/error with timing statistics.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::Engine;
use crate::error::NuzoResult;
use crate::output::OutputSink;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const STRESS_TIMEOUT_MS: u64 = 30_000;
const PERF_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOutcome {
    Pass,
    Fail,
    Timeout,
}

#[derive(Debug, Clone)]
pub struct TestResult {
    pub path: PathBuf,
    pub name: String,
    pub duration: Duration,
    pub outcome: TestOutcome,
    pub message: String,
}

#[derive(Debug)]
pub struct TestSummary {
    pub results: Vec<TestResult>,
    pub total_duration: Duration,
}

impl TestSummary {
    pub fn total(&self) -> usize {
        self.results.len()
    }
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|r| r.outcome == TestOutcome::Pass).count()
    }
    pub fn failed(&self) -> usize {
        self.results.iter().filter(|r| r.outcome == TestOutcome::Fail).count()
    }
    pub fn timeouts(&self) -> usize {
        self.results.iter().filter(|r| r.outcome == TestOutcome::Timeout).count()
    }
}

pub struct TestHarness<'a> {
    engine: &'a Engine,
    timeout_ms: u64,
    pattern: Option<String>,
    verbose: bool,
}

impl<'a> TestHarness<'a> {
    pub fn new(engine: &'a Engine) -> Self {
        Self { engine, timeout_ms: DEFAULT_TIMEOUT_MS, pattern: None, verbose: false }
    }

    pub fn timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    pub fn filter(mut self, pattern: &str) -> Self {
        self.pattern = Some(pattern.to_string());
        self
    }

    pub fn verbose(mut self, v: bool) -> Self {
        self.verbose = v;
        self
    }

    pub fn run_dir(&self, dir: &Path) -> NuzoResult<TestSummary> {
        let files = Self::discover(dir)?;
        self.run_files(&files)
    }

    pub fn run_files(&self, files: &[PathBuf]) -> NuzoResult<TestSummary> {
        let total_start = Instant::now();
        let mut results = Vec::new();

        let total = files.len();
        for (idx, path) in files.iter().enumerate() {
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();

            if let Some(ref pat) = self.pattern {
                let path_str = path.to_string_lossy();
                if !path_str.contains(pat) && !name.contains(pat) {
                    continue;
                }
            }

            let timeout = self.resolve_timeout(path);
            let result = self.run_single(path, &name, timeout);
            let progress = format!("[{}/{}]", idx + 1, total);
            self.print_progress(&progress, &result);
            results.push(result);
        }

        Ok(TestSummary { results, total_duration: total_start.elapsed() })
    }

    fn discover(dir: &Path) -> NuzoResult<Vec<PathBuf>> {
        let mut files = Vec::new();
        Self::walk(dir, &mut files)?;
        files.sort();
        Ok(files)
    }

    fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> NuzoResult<()> {
        // wasm32 目标无文件系统，walk 直接返回空（保留参数以避免 unused 警告）
        #[cfg(not(target_arch = "wasm32"))]
        {
            if dir.is_dir() {
                for entry in std::fs::read_dir(dir).map_err(crate::error::io_err)? {
                    let entry = entry.map_err(crate::error::io_err)?;
                    let path = entry.path();
                    if path.is_dir() {
                        Self::walk(&path, files)?;
                    } else if path.extension().is_some_and(|e| e == "nuzo") {
                        files.push(path);
                    }
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (dir, files);
        }
        Ok(())
    }

    fn resolve_timeout(&self, path: &Path) -> u64 {
        let path_str = path.to_string_lossy().replace('\\', "/");
        if path_str.contains("/stress/") || path_str.contains("stress_") {
            STRESS_TIMEOUT_MS
        } else if path_str.contains("/perf/")
            || path_str.contains("bench_")
            || path_str.contains("perf_")
        {
            PERF_TIMEOUT_MS
        } else {
            self.timeout_ms
        }
    }

    fn run_single(&self, path: &Path, name: &str, timeout_ms: u64) -> TestResult {
        // wasm32 目标无文件系统，构造 Unsupported 错误走原有错误处理路径
        #[cfg(not(target_arch = "wasm32"))]
        let source_result = std::fs::read_to_string(path);
        #[cfg(target_arch = "wasm32")]
        let source_result: Result<String, std::io::Error> =
            Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "fs not available on wasm32"));
        let source = match source_result {
            Ok(s) => s,
            Err(e) => {
                return TestResult {
                    path: path.to_path_buf(),
                    name: name.to_string(),
                    duration: Duration::ZERO,
                    outcome: TestOutcome::Fail,
                    message: format!("IO error: {}", e),
                };
            }
        };

        let start = Instant::now();
        let (sink, _buf) = OutputSink::new_capture();
        let mut session = self.engine.new_session_with(sink);
        session.vm_mut().set_execution_timeout(Some(timeout_ms));

        let (outcome, message) = match session.eval(&source) {
            Ok(_) => (TestOutcome::Pass, String::new()),
            Err(e) => {
                use nuzo_core::NuzoErrorKind;
                match &e.kind {
                    NuzoErrorKind::ExecutionTimeout { .. } => {
                        (TestOutcome::Timeout, format!("exceeded {}ms limit", timeout_ms))
                    }
                    _ => (TestOutcome::Fail, format!("{}", e.kind)),
                }
            }
        };
        let duration = start.elapsed();

        TestResult { path: path.to_path_buf(), name: name.to_string(), duration, outcome, message }
    }

    fn print_progress(&self, progress: &str, r: &TestResult) {
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let (symbol, color) = match r.outcome {
            TestOutcome::Pass => (".", "\x1b[32m"),
            TestOutcome::Fail => ("F", "\x1b[31m"),
            TestOutcome::Timeout => ("T", "\x1b[33m"),
        };
        let _ = write!(lock, "{}{}\x1b[0m", color, symbol);
        let _ = lock.flush();
        if self.verbose || r.outcome != TestOutcome::Pass {
            let dur_ms = r.duration.as_secs_f64() * 1000.0;
            let _ = writeln!(lock, " {} {} ({:.1}ms) {}", progress, r.name, dur_ms, r.message);
        }
    }
}

pub fn print_summary(summary: &TestSummary) {
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let failures: Vec<&TestResult> =
        summary.results.iter().filter(|r| r.outcome != TestOutcome::Pass).collect();

    if !failures.is_empty() {
        println!("\x1b[31mFailures:\x1b[0m");
        for r in &failures {
            let tag = match r.outcome {
                TestOutcome::Fail => "\x1b[31m[FAIL]\x1b[0m",
                TestOutcome::Timeout => "\x1b[33m[TIMEOUT]\x1b[0m",
                TestOutcome::Pass => unreachable!(),
            };
            let rel = r.path.file_name().and_then(|f| f.to_str()).unwrap_or("?");
            let dur_ms = r.duration.as_secs_f64() * 1000.0;
            println!("  {} {} ({:.1}ms): {}", tag, rel, dur_ms, r.message);
        }
        println!();
    }

    let total_dur = summary.total_duration.as_secs_f64();
    let passed = summary.passed();
    let failed = summary.failed();
    let to = summary.timeouts();
    let total = summary.total();

    if failed == 0 && to == 0 {
        println!("\x1b[32mResult: {}/{} passed\x1b[0m in {:.2}s", passed, total, total_dur);
    } else {
        print!("\x1b[31mResult: {}/{} passed\x1b[0m", passed, total);
        if failed > 0 {
            print!(", \x1b[31m{} failures\x1b[0m", failed);
        }
        if to > 0 {
            print!(", \x1b[33m{} timeouts\x1b[0m", to);
        }
        println!(" in {:.2}s", total_dur);
    }
}
