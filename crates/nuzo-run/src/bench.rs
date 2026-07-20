//! BenchHarness — built-in benchmarking support.

use crate::engine::Engine;
use crate::error::NuzoResult;
use crate::output::OutputSink;
use nuzo_core::{InternalError, NuzoError};
use std::time::{Duration, Instant};

/// Benchmark execution mode: controls what phases are measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchMode {
    /// Only measure compilation time
    CompileOnly,
    /// Only measure execution time (assumes compilation is done)
    ExecuteOnly,
    /// Measure end-to-end time (compile + execute)
    EndToEnd,
}

pub struct BenchConfig {
    pub warmup: u32,
    pub iterations: u32,
    /// 单次迭代超时,超过则中断并返回已有采样
    pub iter_timeout: Duration,
    /// 总基准超时(含 warmup + 所有迭代)
    pub total_timeout: Duration,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            warmup: 3,
            iterations: 100,
            iter_timeout: Duration::from_secs(30),
            total_timeout: Duration::from_secs(120),
        }
    }
}

pub struct BenchHarness<'a> {
    engine: &'a Engine,
    config: BenchConfig,
}

#[derive(Debug)]
pub struct BenchResult {
    pub name: String,
    pub mean_ns: u64,
    pub p50_ns: u64,
    pub p99_ns: u64,
    pub ops_per_sec: f64,
}

impl<'a> BenchHarness<'a> {
    pub(crate) fn new(engine: &'a Engine) -> Self {
        Self { engine, config: BenchConfig::default() }
    }

    pub fn warmup(mut self, n: u32) -> Self {
        self.config.warmup = n;
        self
    }
    pub fn iterations(mut self, n: u32) -> Self {
        self.config.iterations = n;
        self
    }

    /// Run a script benchmark with a specific execution mode.
    ///
    /// - `BenchMode::CompileOnly`: measure only compilation time
    /// - `BenchMode::ExecuteOnly`: measure only execution time (compile first, then time execution)
    /// - `BenchMode::EndToEnd`: measure full compile+execute time (same as `run_script`)
    pub fn run_script_mode(
        self,
        name: &str,
        source: &str,
        mode: BenchMode,
    ) -> NuzoResult<BenchResult> {
        match mode {
            BenchMode::EndToEnd => self.run_script(name, source),
            BenchMode::CompileOnly => self.run_compile_only(name, source),
            BenchMode::ExecuteOnly => self.run_execute_only(name, source),
        }
    }

    fn run_compile_only(self, name: &str, source: &str) -> NuzoResult<BenchResult> {
        let total_start = Instant::now();

        // warmup compilation
        let probe = Instant::now();
        let _ = self.engine.compile(source);
        let probe_elapsed = probe.elapsed();

        let effective_warmup = if probe_elapsed > Duration::from_secs(5) {
            1
        } else if probe_elapsed > Duration::from_secs(1) {
            2
        } else {
            self.config.warmup
        };
        for _ in 0..effective_warmup {
            let _ = self.engine.compile(source);
        }

        let effective_iters = self.adapt_iterations(probe_elapsed);
        let mut durations = Vec::with_capacity(effective_iters as usize);
        for _ in 0..effective_iters {
            if total_start.elapsed() > self.config.total_timeout {
                break;
            }
            let t = Instant::now();
            let _ = self.engine.compile(source)?;
            let elapsed = t.elapsed();
            if elapsed > self.config.iter_timeout {
                break;
            }
            durations.push(elapsed);
        }
        Self::stats(name, &durations)
    }

    fn run_execute_only(self, name: &str, source: &str) -> NuzoResult<BenchResult> {
        let total_start = Instant::now();

        // compile once, then measure execution only
        let compiled = self.engine.compile(source)?;

        // warmup execution
        let probe = Instant::now();
        let mut s = self.engine.new_session_with(OutputSink::Null);
        let _ = s.execute(compiled.clone());
        let probe_elapsed = probe.elapsed();

        let effective_warmup = if probe_elapsed > Duration::from_secs(5) {
            1
        } else if probe_elapsed > Duration::from_secs(1) {
            2
        } else {
            self.config.warmup
        };
        for _ in 0..effective_warmup {
            let mut s = self.engine.new_session_with(OutputSink::Null);
            let _ = s.execute(compiled.clone());
        }

        let effective_iters = self.adapt_iterations(probe_elapsed);
        let mut durations = Vec::with_capacity(effective_iters as usize);
        for _ in 0..effective_iters {
            if total_start.elapsed() > self.config.total_timeout {
                break;
            }
            let mut s = self.engine.new_session_with(OutputSink::Null);
            let t = Instant::now();
            let _ = s.execute(compiled.clone())?;
            let elapsed = t.elapsed();
            if elapsed > self.config.iter_timeout {
                break;
            }
            durations.push(elapsed);
        }
        Self::stats(name, &durations)
    }

    pub fn run_script(self, name: &str, source: &str) -> NuzoResult<BenchResult> {
        let total_start = Instant::now();

        // 自适应 warmup: 首次运行计时,如果太慢则减少 warmup 次数
        let mut s = self.engine.new_session_with(OutputSink::Null);
        let probe = Instant::now();
        let _ = s.run(source);
        let probe_elapsed = probe.elapsed();

        let effective_warmup = if probe_elapsed > Duration::from_secs(5) {
            1 // 慢脚本只 warmup 1 次
        } else if probe_elapsed > Duration::from_secs(1) {
            2
        } else {
            self.config.warmup
        };

        for _ in 0..effective_warmup {
            let mut s = self.engine.new_session_with(OutputSink::Null);
            let _ = s.run(source);
        }

        // 自适应迭代: 根据首次耗时决定实际迭代次数
        let effective_iters = self.adapt_iterations(probe_elapsed);
        let mut durations = Vec::with_capacity(effective_iters as usize);

        for _ in 0..effective_iters {
            // 总超时检查
            if total_start.elapsed() > self.config.total_timeout {
                eprintln!(
                    "[bench] 总超时 {:?} 已到,已收集 {} 次采样",
                    self.config.total_timeout,
                    durations.len()
                );
                break;
            }
            // 单次迭代超时检查(在运行前检查剩余时间)
            let remaining = self.config.total_timeout.saturating_sub(total_start.elapsed());
            if remaining < probe_elapsed {
                eprintln!("[bench] 剩余时间不足以完成下一次迭代,已收集 {} 次采样", durations.len());
                break;
            }

            let mut s = self.engine.new_session_with(OutputSink::Null);
            let t = Instant::now();
            let _ = s.run(source)?;
            let elapsed = t.elapsed();

            // 单次迭代超时
            if elapsed > self.config.iter_timeout {
                eprintln!(
                    "[bench] 单次迭代超时 {:?},已收集 {} 次采样",
                    self.config.iter_timeout,
                    durations.len()
                );
                break;
            }
            durations.push(elapsed);
        }
        Self::stats(name, &durations)
    }

    pub fn run_custom<F: FnMut()>(self, name: &str, mut f: F) -> NuzoResult<BenchResult> {
        let total_start = Instant::now();

        // 自适应 warmup
        let probe = Instant::now();
        f();
        let probe_elapsed = probe.elapsed();

        let effective_warmup = if probe_elapsed > Duration::from_secs(5) {
            1
        } else if probe_elapsed > Duration::from_secs(1) {
            2
        } else {
            self.config.warmup
        };
        for _ in 0..effective_warmup {
            f();
        }

        let effective_iters = self.adapt_iterations(probe_elapsed);
        let mut durations = Vec::with_capacity(effective_iters as usize);

        for _ in 0..effective_iters {
            if total_start.elapsed() > self.config.total_timeout {
                break;
            }
            let t = Instant::now();
            f();
            let elapsed = t.elapsed();
            if elapsed > self.config.iter_timeout {
                break;
            }
            durations.push(elapsed);
        }
        Self::stats(name, &durations)
    }

    /// 根据首次运行耗时自适应调整迭代次数:
    ///   < 1ms  → 100 次 (默认)
    ///   < 10ms → 50 次
    ///   < 100ms → 20 次
    ///   < 1s   → 10 次
    ///   < 5s   → 5 次
    ///   >= 5s  → 3 次
    fn adapt_iterations(&self, probe: Duration) -> u32 {
        use std::cmp::min;
        let iters = match probe.as_millis() {
            0..=1 => self.config.iterations,
            2..=10 => min(self.config.iterations, 50),
            11..=100 => min(self.config.iterations, 20),
            101..=1000 => min(self.config.iterations, 10),
            1001..=5000 => min(self.config.iterations, 5),
            _ => min(self.config.iterations, 3),
        };
        iters.max(3) // 至少 3 次保证 p50/p99 可计算
    }

    /// Compute benchmark statistics over a sorted-by-caller sample set.
    ///
    /// Returns `Err(InternalError::EmptySamples)` when `durations` is empty,
    /// which would otherwise trigger divide-by-zero (`mean = total / n` with
    /// `n == 0`), index-out-of-bounds (`ns[0]`), and unsigned underflow
    /// (`n - 1` with `n == 0`).
    fn stats(name: &str, durations: &[Duration]) -> NuzoResult<BenchResult> {
        let mut ns: Vec<u64> = durations.iter().map(|d| d.as_nanos() as u64).collect();
        ns.sort();
        if ns.is_empty() {
            return Err(NuzoError::internal(InternalError::EmptySamples, None));
        }
        let n = ns.len() as u64;
        let total: u64 = ns.iter().sum();
        let mean = total / n;
        let p50 = ns[(n as f64 * 0.5) as usize];
        let p99 = ns[(n as f64 * 0.99).min((n - 1) as f64) as usize];
        let ops_per_sec = if mean > 0 { 1_000_000_000.0 / mean as f64 } else { f64::INFINITY };
        Ok(BenchResult {
            name: name.to_string(),
            mean_ns: mean,
            p50_ns: p50,
            p99_ns: p99,
            ops_per_sec,
        })
    }
}

impl BenchResult {
    pub fn format(&self) -> String {
        format!(
            "{:<20} mean={:>10.2}µs  p50={:>10.2}µs  p99={:>10.2}µs  {:>10.0} ops/s",
            self.name,
            self.mean_ns as f64 / 1000.0,
            self.p50_ns as f64 / 1000.0,
            self.p99_ns as f64 / 1000.0,
            self.ops_per_sec,
        )
    }
}

// ============================================================================
// 回归测试:C3 bench 空采样除零 bug
// ----------------------------------------------------------------------------
// 原始 bug:`stats` 在 `durations` 为空时会触发三类崩溃:
//   1. 除零:`mean = total / n` 中 `n == 0`
//   2. 越界:`ns[0]` 访问空切片
//   3. u64 下溢:`n - 1` 当 `n == 0`
// 修复:空切片提前返回 `Err(InternalError::EmptySamples)`。
// 本模块直接测试私有 `stats` 函数(子模块可访问父模块私有项,无需放宽可见性)。
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_core::NuzoErrorKind;
    use std::time::Duration;

    /// 1 秒对应的纳秒数,用于 ops_per_sec 期望值计算(科学常量,非魔法数字)
    const NANOS_PER_SEC: f64 = 1_000_000_000.0;

    /// 断言给定的 `NuzoError` 恰好是 `InternalError::EmptySamples`(无 diagnosis)。
    /// 使用显式 panic 而非 `unwrap`,便于失败时打印实际收到的变体。
    fn assert_empty_samples(err: NuzoError) {
        match err.kind {
            NuzoErrorKind::Internal(InternalError::EmptySamples, None) => {}
            other => {
                panic!("期望 NuzoErrorKind::Internal(EmptySamples, None),实际得到: {:?}", other)
            }
        }
    }

    /// 回归测试:空采样集必须返回 `EmptySamples` 错误,而非触发除零/越界/下溢。
    /// 这是 C3 bug 的核心修复点。
    #[test]
    fn test_stats_empty_returns_error() {
        let result = BenchHarness::stats("empty", &[]);
        assert!(result.is_err(), "空采样集应返回 Err,而非触发崩溃");
        assert_empty_samples(result.unwrap_err());
    }

    /// 单采样:mean = p50 = p99 = 该采样值;ops_per_sec = 1e9 / mean。
    /// 覆盖 `n == 1` 边界,验证 p50/p99 索引计算不会越界。
    #[test]
    fn test_stats_single_sample() {
        const SINGLE_SAMPLE_NS: u64 = 1_000;
        let durations = vec![Duration::from_nanos(SINGLE_SAMPLE_NS)];

        let result = BenchHarness::stats("single", &durations).expect("单采样应成功计算统计量");

        assert_eq!(result.name, "single", "name 应原样回传");
        assert_eq!(result.mean_ns, SINGLE_SAMPLE_NS, "单采样均值应等于采样值");
        assert_eq!(result.p50_ns, SINGLE_SAMPLE_NS, "单采样 p50 应等于采样值");
        assert_eq!(result.p99_ns, SINGLE_SAMPLE_NS, "单采样 p99 应等于采样值");
        assert_eq!(
            result.ops_per_sec,
            NANOS_PER_SEC / SINGLE_SAMPLE_NS as f64,
            "单采样 ops_per_sec 应为 NANOS_PER_SEC / mean"
        );
    }

    /// 多采样:验证 mean / p50 / p99 与 ops_per_sec 的精确计算。
    /// 样本数 n=5(奇数),确保 p50/p99 索引可精确推导:
    ///   - p50 idx = (5 * 0.5) as usize = 2
    ///   - p99 idx = (5 * 0.99).min(4) as usize = 4
    ///     期望值通过复现 `stats` 内部公式计算得出,而非硬编码结果。
    #[test]
    fn test_stats_multiple_samples() {
        const SAMPLE_NS: [u64; 5] = [100, 200, 300, 400, 500];
        let durations: Vec<Duration> =
            SAMPLE_NS.iter().map(|&ns| Duration::from_nanos(ns)).collect();

        let result = BenchHarness::stats("multi", &durations).expect("多采样应成功计算统计量");

        // 复现 stats 内部公式,作为期望值(控制变量:公式与实现一致)
        let n = SAMPLE_NS.len() as u64;
        let total: u64 = SAMPLE_NS.iter().sum();
        let expected_mean = total / n;
        let p50_index = (n as f64 * 0.5) as usize;
        let expected_p50 = SAMPLE_NS[p50_index];
        let p99_index = (n as f64 * 0.99).min((n - 1) as f64) as usize;
        let expected_p99 = SAMPLE_NS[p99_index];

        assert_eq!(result.mean_ns, expected_mean, "mean = total / n");
        assert_eq!(result.p50_ns, expected_p50, "p50 = ns[(n*0.5) as usize],索引 = {}", p50_index);
        assert_eq!(
            result.p99_ns, expected_p99,
            "p99 = ns[(n*0.99).min(n-1) as usize],索引 = {}",
            p99_index
        );
        assert_eq!(
            result.ops_per_sec,
            NANOS_PER_SEC / expected_mean as f64,
            "ops_per_sec = NANOS_PER_SEC / mean (mean > 0 分支)"
        );
    }

    /// 零时长采样:mean = 0 时 `ops_per_sec` 应为正无穷,验证 mean==0 保护分支
    /// 不触发除零。这是 ops_per_sec 计算路径的边界回归。
    #[test]
    fn test_stats_zero_duration_no_divide_by_zero() {
        let durations = vec![Duration::ZERO, Duration::ZERO, Duration::ZERO];

        let result =
            BenchHarness::stats("zero", &durations).expect("零时长采样应成功,而非除零崩溃");

        assert_eq!(result.mean_ns, 0, "全零时长均值应为 0");
        assert_eq!(result.p50_ns, 0, "全零时长 p50 应为 0");
        assert_eq!(result.p99_ns, 0, "全零时长 p99 应为 0");
        assert!(
            result.ops_per_sec.is_infinite() && result.ops_per_sec.is_sign_positive(),
            "mean == 0 时 ops_per_sec 应为正无穷 (f64::INFINITY),实际: {}",
            result.ops_per_sec
        );
    }
}
