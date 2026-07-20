//! # 错误收集器 (Error Collector) - 诊断系统的核心引擎
//!
//! 本模块提供 [`ErrorCollector`] 结构体，是整个错误处理系统的**运行时核心**。
//! 负责错误的收集、存储、分析、报告和导出。
//!
//! ## 架构定位
//!
//! ```text
//! ┌──────────────┐     ┌─────────────────┐     ┌──────────────────┐
//! │   VM 执行循环  │ ──▶ │  ErrorCollector  │ ──▶ │   输出/导出      │
// │ │              │     │                 │     │                  │

// │ │ - execute()  │     │ - collect_*()   │     │ - print_report() │

// │ │ - run()      │     │ - analyze()     │     │ - export_json()  │

// │ └──────────────┘     └─────────────────┘     └──────────────────┘

//!                             │
//!                             ▼
//!              ┌──────────────────────────────┐
//!              │    智能诊断引擎 (Smart Dx)     │
// │              ├──────────────────────────────┤

// │              │ - smart_deduplicate()        │

// │              │ - detect_repeating_patterns() │

// │              │ - cluster_errors_simple()    │

// │              │ - get_practical_fix_priority()│

//!              └──────────────────────────────┘
//! ```
//!
//! ## 核心职责
//!
//! ### 1. 错误收集 (Collection)
//!
//! 提供三个层次的收集 API：
//!
//! - **`collect_error()`** - 旧 API，接受 RuntimeError（向后兼容）
//! - **`collect_nuzo_error()`** - 新 API，接受 NuzoError（推荐）
//! - **`handle_error_in_diagnostic_mode()`** - 高级 API，自动处理 InternalError 的诊断
//!
//! ### 2. 状态管理 (State Management)
//!
//! - 启用/禁用诊断模式
//! - 配置最大错误数量限制
//! - 配置遇到致命错误时的行为（停止/继续）
//! - 维护全局指令计数器
//!
//! ### 3. 智能分析 (Intelligent Analysis)
//!
//! - **去重**: 基于多维相似度的智能去重（>95% 视为重复）
//! - **模式检测**: 发现相同错误在不同位置的重复出现
//! - **聚类**: 按严重程度+类别分组
//! - **优先级排序**: 基于多因素评分的修复顺序建议
//!
//! ### 4. 报告生成 (Report Generation)
//!
//! - 控制台输出：格式化的完整诊断报告（带 emoji、表格、颜色）
//! - JSON 导出：支持美化/紧凑/完整报告三种格式
//! - 文件输出：直接写入 JSON 文件
//!
//! ## 使用模式
//!
//! ### 基础模式：收集 + 打印
//!
//! ```rust,ignore
//! let mut collector = ErrorCollector::new();
//! collector.enable();
//!
//! // 在 VM 主循环中：
//! loop {
//!     collector.record_instruction();
//!     match vm.execute() {
//!         Ok(value) => break Ok(value),
//!         Err(error) => {
//!             let should_continue = collector.collect_nuzo_error(
//!                 error, context, call_stack, None,
//!             );
//!             if !should_continue { break Err(...); }
//!         }
//!     }
//! }
//!
//! // 程序结束后：
//! collector.print_full_report();
//! collector.export_to_file("errors.json")?;
//! ```
//!
//! ### 高级模式：自动诊断 InternalError
//!
//! ```rust,ignore
//! // 使用 handle_error_in_diagnostic_mode() 自动生成诊断报告
//! let should_continue = collector.handle_error_in_diagnostic_mode(
//!     error,
//!     context,
//!     call_stack,
//!     |ie| vm.diagnose_internal_error(ie),  // 诊断闭包
//! );
//! ```
//!
//! ## 性能特征
//!
//! | 操作 | 时间复杂度 | 实际性能 |
//! |------|-----------|----------|
//! | `collect_error()` | O(1) 均摊 | < 1μs |
//! | `record_instruction()` | O(1) | < 10ns |
//! | `smart_deduplicate()` | O(N²) | N<1000 时 <100ms |
//! | `export_json()` | O(N) | N=1000 时 ~5ms |
//! | `print_full_report()` | O(N*M) | 取决于输出量 |
//!
//! # 设计原则
//!
//! 1. **非侵入式**: 未启用时开销接近零（仅布尔检查）
//! 2. **向后兼容**: 同时支持新旧两种错误类型 API
//! 3. **可配置**: 最大错误数、停止策略等均可调整
//! 4. **可扩展**: 分析算法独立封装，易于替换或增强

use nuzo_core::{XxHashMap, xx_hash_map_new};
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use std::fs::File;
use std::io::Write as IoWrite;
use std::mem;

use crossbeam_queue::SegQueue;

use super::diagnostic::DiagnosticError;
use super::formatter::DiagnosticFormatter;
use super::sink::{ErrorEvent, ErrorSink};
use super::smart_types::*;
use super::types::*;
use nuzo_core::SourceLocation;

use nuzo_core::Value;

#[cfg(test)]
use nuzo_bytecode::Opcode;
use nuzo_values::{InternalError, NuzoError, NuzoErrorKind as ErrorKind, VmDiagnosis};

// ============================================================================
// Scoring Constants (评分常量)
// ============================================================================

/// 风险评分 - 严重度因子
const RISK_SEVERITY_FATAL: f64 = 100.0;
const RISK_SEVERITY_ERROR: f64 = 75.0;
const RISK_SEVERITY_WARNING: f64 = 40.0;
const RISK_SEVERITY_INFO: f64 = 10.0;
const RISK_COUNT_MULTIPLIER: f64 = 10.0;
const RISK_COUNT_MAX: f64 = 50.0;

/// 优先级评分
const PRIORITY_SEVERITY_FATAL: f64 = 30.0;
const PRIORITY_SEVERITY_ERROR: f64 = 22.0;
const PRIORITY_SEVERITY_WARNING: f64 = 12.0;
const PRIORITY_SEVERITY_INFO: f64 = 5.0;
const PRIORITY_IMPACT_BASE: f64 = 15.0;
const PRIORITY_CONTEXT_BASE: f64 = 5.0;

/// 实用优先级
const PRACTICAL_SEVERITY_FATAL: f64 = 85.0;
const PRACTICAL_SEVERITY_ERROR: f64 = 65.0;
const PRACTICAL_SEVERITY_WARNING: f64 = 35.0;
const PRACTICAL_SEVERITY_INFO: f64 = 15.0;
const FREQ_GLOBAL_MAX_SCORE: f64 = 15.0;

/// 修复难度评分
const FIXABILITY_DIV_BY_ZERO: f64 = 18.0;
const FIXABILITY_INDEX_OOB: f64 = 14.0;
const FIXABILITY_TYPE_MISMATCH: f64 = 12.0;
const FIXABILITY_ARITH_OVERFLOW: f64 = 8.0;
const FIXABILITY_ASSERT_FAILED: f64 = 14.0;
const FIXABILITY_EXPECTED_NUMBER: f64 = 10.0;
const FIXABILITY_UNDEF_VAR: f64 = 1.5;
const FIXABILITY_INVALID_ARG_COUNT: f64 = 12.0;
const FIXABILITY_UNSUPPORTED_OP: f64 = 8.0;
const FIXABILITY_INTERNAL: f64 = 2.0;
const FIXABILITY_DEFAULT: f64 = 10.0;
const FIXABILITY_DEEP_CALL_THRESHOLD: usize = 2;
const FIXABILITY_MAX_SCORE: f64 = 20.0;

/// 其他
const SIMILARITY_SAMPLE_SIZE: usize = 10;
const DEFAULT_AVG_SIMILARITY: f64 = 0.8;

// ============================================================================
// Main Error Collector (主错误收集器)
// ============================================================================
//
// 核心数据结构，负责：
// 1. 错误的收集和存储
// 2. 统计信息的维护
// 3. 智能诊断分析的协调
// 4. 报告的生成和导出
//
// 设计特点：
// - 非线程安全（单线程 VM 环境）
// - 可重置（clear() 后可复用）
// - 可序列化（所有核心字段都实现了 Serialize）

/// 错误收集器 - Nuzo 运行时诊断系统的核心引擎
///
/// 负责在程序执行过程中**收集、分类、分析和报告**所有错误。
///
/// # 生命周期
///
/// ```text
/// 创建 → [启用] → 收集错误 → [分析] → [报告] → [导出] → [清理/复用]
///  new()   enable() collect_*() smart_*() print_*() export_*() clear()
/// ```
///
/// # 字段说明
///
/// | 字段 | 类型 | 用途 |
/// |------|------|------|
/// | `enabled` | `bool` | 是否启用诊断模式（默认 false） |
/// | `errors` | `Vec<DiagnosticError>` | 收集到的所有错误列表 |
/// | `error_counter` | `usize` | 已分配的错误 ID 计数器 |
/// | `instruction_counter` | `usize` | 全局指令执行计数器 |
/// | `max_errors` | `usize` | 最大错误数量限制（默认 1000） |
/// | `stop_on_fatal` | `bool` | 遇到 Fatal 是否停止（默认 true） |
/// | `stats` | `ErrorStatistics` | 实时统计信息 |
///
/// # 内存占用
///
/// 基础结构：~200B
/// 每个 DiagnosticError：~1-2KB
/// 1000 个错误上限：~2MB（完全可接受）
pub struct ErrorCollector {
    /// 是否启用诊断模式
    ///
    /// **未启用时**: 所有 collect_*() 方法立即返回 false，几乎零开销
    /// **启用后**: 开始收集错误并更新统计信息
    enabled: bool,

    /// 收集到的错误列表
    ///
    /// 按**时间顺序**存储（先发生的在前）。
    /// 每个元素都是完整的 `DiagnosticError`，包含上下文和建议。
    errors: Vec<DiagnosticError>,

    /// 错误 ID 分配计数器
    ///
    /// 从 0 开始递增，用于分配全局唯一的错误 ID。
    /// 在 clear() 时重置为 0。
    error_counter: usize,

    /// 全局指令执行计数器
    ///
    /// 每次 record_instruction() 调用时 +1。
    /// 用于时间线分析和时间局部性检测。
    instruction_counter: usize,

    /// 最大错误数量限制
    ///
    /// 达到此数量后，后续的 collect_*() 返回 false（停止执行）。
    /// 默认值：1000
    /// 设置为 0 表示无限制（不推荐，可能导致内存问题）。
    max_errors: usize,

    /// 遇到致命错误是否停止执行
    ///
    /// - true（默认）：遇到 Fatal 立即返回 false
    /// - false：继续收集所有错误（用于"扫描模式"）
    stop_on_fatal: bool,

    /// 实时统计信息
    stats: ErrorStatistics,

    /// 警告去重计数器：key = "{severity}:{msg}@IP={ip}", value = 重复次数
    ///
    /// 相同的 (严重级别, 错误消息, IP) 组合只打印一次，
    /// 后续重复仅递增计数器。在 print_summary 时统一汇报。
    warning_dedup: XxHashMap<String, u32>,

    /// 无锁错误事件队列 - 接收来自 VM 的 ErrorEvent
    ///
    /// 使用 `crossbeam_queue::SegQueue`（无界无锁 MPMC 队列）替代 `Mutex<Vec>`：
    /// - `sink_error(&self)` 在共享引用下无锁 push（O(1)，适合 VM 错误热路径）
    /// - `drain_sunk(&mut self)` 批量 pop 并转为 `DiagnosticError` 的原料
    /// - `SegQueue` 的 `push`/`pop` 均为 `&self` 方法，靠内部原子操作保证线程安全
    ///
    /// 设计权衡：`ErrorEvent` 仅含 `message` 等轻量字段，不携带 `NuzoError`/
    /// `ExecutionContext` 等重上下文（这些由 VM adapter 在 `drain_sunk` 后补充），
    /// 因此队列元素小、push 快，不会阻塞 VM 执行循环。
    sunk_events: SegQueue<ErrorEvent>,
}

/// 错误统计信息 - 实时聚合的诊断数据
///
/// 由 `ErrorCollector` 在每次 `collect_*()` 调用时自动更新。
/// 提供多维度的错误分布统计，用于报告生成和趋势分析。
///
/// # 字段说明
///
/// | 字段 | 类型 | 用途 | 更新时机 |
/// |------|------|------|----------|
/// | `total_errors` | `usize` | 总错误数 | 每次收集 |
/// | `severity_counts` | `XxHashMap<ErrorSeverity, usize>` | 各级别错误数 | 每次收集 |
/// | `category_counts` | `XxHashMap<ErrorCategory, usize>` | 各类别错误数 | 每次收集 |
/// | `error_prone_instructions` | `XxHashMap<String, usize>` | 高频出错指令 | 有 opcode 时 |
///
/// # 使用场景
///
/// 1. **摘要报告**: 显示"共发现 X 个错误，其中 Y 个致命"
/// 2. **热点分析**: 识别"哪条指令最容易出错"
/// 3. **趋势监控**: 对比多次运行的统计数据
/// 4. **JSON 导出**: 序列化后供外部工具分析
#[derive(Debug, Default, Serialize)]
pub struct ErrorStatistics {
    /// 总错误数
    pub total_errors: usize,
    /// 各级别错误数
    pub severity_counts: XxHashMap<ErrorSeverity, usize>,
    /// 各类别错误数
    pub category_counts: XxHashMap<ErrorCategory, usize>,
    /// 最常出错的指令
    pub error_prone_instructions: XxHashMap<String, usize>,
}

impl ErrorCollector {
    /// 创建新的错误收集器
    pub fn new() -> Self {
        ErrorCollector {
            enabled: false,
            errors: Vec::new(),
            error_counter: 0,
            instruction_counter: 0,
            max_errors: 1000,
            stop_on_fatal: true,
            stats: ErrorStatistics::default(),
            warning_dedup: xx_hash_map_new(),
            sunk_events: SegQueue::new(),
        }
    }

    /// 启用诊断模式
    pub fn enable(&mut self) {
        self.enabled = true;
        log::info!("[诊断模式] 已启用 - 将收集所有错误而不中断执行");
    }

    /// 禁用诊断模式
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// 检查是否已启用
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 设置最大错误数量
    pub fn max_errors(&mut self, max: usize) {
        self.max_errors = max;
    }

    /// 设置遇到致命错误时的行为
    pub fn stop_on_fatal(&mut self, stop: bool) {
        self.stop_on_fatal = stop;
    }

    /// 记录指令执行（更新计数器）
    #[inline]
    pub fn record_instruction(&mut self) {
        if self.enabled {
            self.instruction_counter += 1;
        }
    }

    /// 排空 `sunk_events` 队列，返回事件列表供调用方转为 `DiagnosticError`
    ///
    /// # 调用时机
    ///
    /// - VM 执行结束后，由调用方主动调用
    /// - 定期批量处理（避免每条错误都走 `collect_*` 路径）
    ///
    /// # 行为
    ///
    /// - 仅当 `enabled` 时更新 `stats.total_errors`（disabled 时不计入统计，
    ///   但事件仍会被排空返回，调用方可选择忽略）
    /// - 返回的 `Vec<ErrorEvent>` 保留 push 顺序（FIFO）
    ///
    /// # 为何不在内部直接构造 `DiagnosticError`
    ///
    /// `ErrorEvent` 仅含 `message: String`，而 `DiagnosticError::new` 需要
    /// `NuzoError` + `ExecutionContext` + `Vec<StackFrameInfo>`（见 `collect_error`）。
    /// 这些重上下文由 VM adapter 持有，故转换推迟到调用方完成，
    /// 本方法只负责"排空队列 + 更新统计"两项轻量职责。
    pub fn drain_sunk(&mut self) -> Vec<ErrorEvent> {
        let mut drained = Vec::new();
        while let Some(event) = self.sunk_events.pop() {
            if self.enabled {
                self.stats.total_errors += 1;
            }
            drained.push(event);
        }
        drained
    }

    /// 返回当前 `sunk_events` 队列中待处理事件数量（主要用于测试与监控）
    ///
    /// 注意：`SegQueue::len` 是近似值（无锁并发环境下不保证精确），
    /// 仅用于观测，不要用于关键控制流判断。
    pub fn sunk_pending(&self) -> usize {
        self.sunk_events.len()
    }

    /// 去重计数诊断信息（Warning/Info 级别不实时打印）
    ///
    /// 相同的 (severity, message, IP) 组合只记录一次，
    /// 所有警告在程序结束后通过 print_warning_summary() 统一展示。
    fn count_deduplicated(&mut self, label: &str, msg: &str, ip: usize) {
        let key = format!("{}:{}@IP={}", label, msg, ip);
        *self.warning_dedup.entry(key).or_insert(0) += 1;
    }

    /// 打印去重后的警告摘要（在程序结束后调用）
    ///
    /// 将所有被抑制的 Warning/Info 级别诊断信息按去重分组输出，
    /// 让程序正常输出在执行过程中保持清晰。
    pub fn print_warning_summary(&self) {
        if self.warning_dedup.is_empty() {
            return;
        }

        // 按 (label, msg) 分组聚合不同 IP 的同类错误
        let mut groups: XxHashMap<(&str, String), Vec<(usize, u32)>> = xx_hash_map_new();
        for (key, &count) in &self.warning_dedup {
            // key 格式: "{label}:{msg}@IP={ip}"
            if let Some(at_pos) = key.find("@IP=") {
                let prefix = &key[..at_pos];
                let ip: usize = key[at_pos + 4..].parse().unwrap_or(0);
                if let Some(colon_pos) = prefix.find(':') {
                    let label = &prefix[..colon_pos];
                    let msg = prefix[colon_pos + 1..].to_string();
                    groups.entry((label, msg)).or_default().push((ip, count));
                }
            }
        }

        if groups.is_empty() {
            return;
        }

        println!(
            "\n📋 [诊断] 警告摘要 (共 {} 条, {} 种模式):",
            self.warning_dedup.values().sum::<u32>(),
            groups.len()
        );

        // 按总出现次数降序排列
        let mut sorted: Vec<_> = groups.into_iter().collect();
        sorted.sort_by(|a, b| {
            let total_a: u32 = a.1.iter().map(|&(_, c)| c).sum();
            let total_b: u32 = b.1.iter().map(|&(_, c)| c).sum();
            total_b.cmp(&total_a)
        });

        for ((label, msg), occurrences) in &sorted {
            let total: u32 = occurrences.iter().map(|&(_, c)| c).sum();
            if occurrences.len() == 1 && occurrences[0].1 == 1 {
                println!("   • {}: {} @ IP={}", label, msg, occurrences[0].0);
            } else {
                println!("   • {}: {} (×{}次)", label, msg, total);
            }
        }
    }

    /// 统一的诊断信息收集逻辑
    fn collect_diagnostic(
        &mut self,
        diagnostic: DiagnosticError,
        print_fn: impl FnOnce(&DiagnosticError, &DiagnosticFormatter),
    ) -> bool {
        let fmt = DiagnosticFormatter::new();
        print_fn(&diagnostic, &fmt);

        self.update_stats(&diagnostic);
        self.errors.push(diagnostic);
        self.error_counter += 1;

        if self.errors.len() >= self.max_errors {
            println!("⛔  [诊断] 已达到最大错误数量限制 ({})，停止执行", self.max_errors);
            return false;
        }

        if self.stop_on_fatal && self.last_severity() == Some(&ErrorSeverity::Fatal) {
            println!("💥  [诊断] 遇到致命错误，按配置停止执行");
            return false;
        }

        true
    }

    /// 收集一个错误
    ///
    /// Returns `true` if execution should continue, `false` if should stop
    pub fn collect_error(
        &mut self,
        error: NuzoError,
        context: ExecutionContext,
        call_stack: Vec<StackFrameInfo>,
    ) -> bool {
        if !self.enabled {
            return false;
        }

        let diagnostic = DiagnosticError::new(
            self.error_counter,
            error,
            context,
            call_stack,
            self.instruction_counter,
        );

        // 预提取去重信息（闭包内不可借用 self）
        let dedup_info = match diagnostic.severity {
            ErrorSeverity::Fatal | ErrorSeverity::Error => None,
            _ => {
                let label = DiagnosticFormatter::new().severity_label(diagnostic.severity);
                let msg = diagnostic.error.to_string();
                let ip = diagnostic.context.ip;
                Some((label, msg, ip))
            }
        };

        let result = self.collect_diagnostic(diagnostic, |last, fmt| {
            let msg = last.error.to_string();
            let ip = last.context.ip;
            let id = last.id;
            let severity = last.severity;
            let emoji = fmt.severity_emoji(severity);
            let label = fmt.severity_label(severity);
            match severity {
                ErrorSeverity::Fatal | ErrorSeverity::Error => {
                    let styled = fmt
                        .severity_style(severity)
                        .apply_to(format!("[诊断] {} #{}: {} @ IP={}", label, id, msg, ip));
                    println!("{} {}", emoji, styled);
                }
                _ => {} // Warning/Info 级别不实时打印
            }
        });

        // 去重计数
        if let Some((label, msg, ip)) = dedup_info {
            self.count_deduplicated(label, &msg, ip);
        }

        result
    }

    /// 收集一个 NuzoError 错误（新 API - 推荐）
    pub fn collect_nuzo_error(
        &mut self,
        error: NuzoError,
        context: ExecutionContext,
        call_stack: Vec<StackFrameInfo>,
        diagnosis: Option<VmDiagnosis>,
    ) -> bool {
        if !self.enabled {
            return false;
        }

        let diagnostic = DiagnosticError::from_nuzo_error(
            self.error_counter,
            error,
            context,
            call_stack,
            self.instruction_counter,
            diagnosis,
        );

        // 预提取去重信息
        let dedup_info = match diagnostic.severity {
            ErrorSeverity::Fatal | ErrorSeverity::Error => None,
            _ => {
                let label = DiagnosticFormatter::new().severity_label(diagnostic.severity);
                let msg = if let Some(ref nuzo_err) = diagnostic.nuzo_error {
                    format!("{}", nuzo_err)
                } else {
                    format!("{}", diagnostic.error)
                };
                let ip = diagnostic.context.ip;
                Some((label, msg, ip))
            }
        };

        let result = self.collect_diagnostic(diagnostic, |last, fmt| {
            let error_msg = if let Some(ref nuzo_err) = last.nuzo_error {
                format!("{}", nuzo_err)
            } else {
                format!("{}", last.error)
            };
            let emoji = fmt.severity_emoji(last.severity);
            let label = fmt.severity_label(last.severity);
            let severity = last.severity;
            let ip = last.context.ip;
            let id = last.id;
            let is_internal = last.is_internal_error();
            let has_diagnosis = last.diagnosis.is_some();
            match severity {
                ErrorSeverity::Fatal | ErrorSeverity::Error => {
                    let styled = fmt
                        .severity_style(severity)
                        .apply_to(format!("[诊断] {} #{}: {} @ IP={}", label, id, error_msg, ip));
                    println!("{} {}", emoji, styled);
                }
                _ => {} // Warning/Info 级别不实时打印
            }

            if is_internal {
                println!(
                    "    {} ",
                    fmt.warning_style().apply_to("⚠️ 这是运行时内部错误 (InternalError)")
                );
                if has_diagnosis {
                    println!("    {}", fmt.info_style().apply_to("🔬 已生成 VM 诊断报告"));
                }
            }
        });

        // 去重计数
        if let Some((label, msg, ip)) = dedup_info {
            self.count_deduplicated(label, &msg, ip);
        }

        result
    }

    /// 处理诊断模式下的错误（统一入口）
    ///
    /// 此方法自动判断错误类型并调用适当的处理逻辑：
    /// - 对于 InternalError：自动尝试生成诊断报告
    /// - 对于 ProgramError：正常收集
    ///
    /// 注意：此方法需要 VM 引用来生成诊断报告。
    /// 如果不需要自动诊断，请直接使用 `collect_nuzo_error()`。
    pub fn handle_error_in_diagnostic_mode<F>(
        &mut self,
        error: NuzoError,
        context: ExecutionContext,
        call_stack: Vec<StackFrameInfo>,
        diagnose_fn: F,
    ) -> bool
    where
        F: FnOnce(&InternalError) -> Option<VmDiagnosis>,
    {
        // 对于 InternalError，自动调用诊断函数生成报告
        let diagnosis = if matches!(error.kind, ErrorKind::Internal(_, _)) {
            // Fatal errors are internal errors — generate diagnosis
            // Extract InternalError from ErrorKind for the diagnose_fn
            let internal_err = Self::extract_internal_error(&error.kind);
            if let Some(ie) = internal_err {
                println!("[诊断] 检测到 InternalError，正在生成诊断报告...");
                Some(diagnose_fn(&ie))
            } else {
                None
            }
        } else {
            None // Program-level errors don't have internal diagnostics
        };

        // 展平 Option<Option<VmDiagnosis>> -> Option<VmDiagnosis>
        let diagnosis = diagnosis.flatten();

        self.collect_nuzo_error(error, context, call_stack, diagnosis)
    }

    /// 更新统计信息
    fn update_stats(&mut self, error: &DiagnosticError) {
        self.stats.total_errors += 1;

        *self.stats.severity_counts.entry(error.severity).or_insert(0) += 1;
        *self.stats.category_counts.entry(error.category.clone()).or_insert(0) += 1;

        if let Some(ref op) = error.context.opcode {
            let op_name = format!("{:?}", op);
            *self.stats.error_prone_instructions.entry(op_name).or_insert(0) += 1;
        }
    }

    /// 获取最后一个错误的严重程度
    fn last_severity(&self) -> Option<&ErrorSeverity> {
        self.errors.last().map(|e| &e.severity)
    }

    /// 获取收集到的所有错误
    pub fn errors(&self) -> &[DiagnosticError] {
        &self.errors
    }

    /// 获取错误数量
    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// 是否有错误
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// 获取统计信息
    pub fn statistics(&self) -> &ErrorStatistics {
        &self.stats
    }

    /// 清空所有收集的错误
    pub fn clear(&mut self) {
        self.errors.clear();
        self.error_counter = 0;
        self.instruction_counter = 0;
        self.stats = ErrorStatistics::default();
    }

    // ========================================================================
    // Report Generation
    // ========================================================================

    /// 打印完整的诊断报告
    pub fn print_full_report(&self) {
        let fmt = DiagnosticFormatter::new();

        if !self.enabled {
            println!("{}", fmt.warning_style().apply_to("⚠️ 诊断模式未启用，无报告可生成"));
            return;
        }

        if self.errors.is_empty() {
            println!("\n{}", fmt.success_style().apply_to("✅ 诊断报告: 未发现任何错误！"));
            println!("   程序执行完美，共运行 {} 条指令", self.instruction_counter);
            return;
        }

        // 标题框
        println!("\n{}", fmt.top_border());
        let title = fmt.fatal_style().apply_to("📊 Nuzo 诊断报告 - 错误分析总览");
        println!("║  {}", title);
        println!("{}", fmt.bottom_border());

        // 统计摘要
        self.print_summary_with_fmt(&fmt);

        println!("{}", fmt.separator());

        // 详细错误列表
        println!("{}", fmt.section_header("📋", "详细错误列表"));

        for error in &self.errors {
            println!("{}", error);
        }

        println!("{}", fmt.separator());

        // 调用图分析
        self.print_call_graph_analysis_with_fmt(&fmt);

        println!("{}", fmt.separator());

        // 修复优先级建议
        self.print_fix_priority_with_fmt(&fmt);

        println!("{}", fmt.separator());

        // ===== 智能诊断部分 =====
        if !self.errors.is_empty() {
            println!("{}", fmt.section_header("🔍", "智能诊断结果"));

            // 1. 去重报告
            let dedup_report = self.smart_deduplicate();
            if !dedup_report.duplicate_groups.is_empty() {
                println!(
                    "\n📦 发现 {} 组重复错误 (可减少 {} 个冗余报告)",
                    dedup_report.duplicate_groups.len(),
                    dedup_report.original_count - dedup_report.remaining_after_dedup
                );
                for (i, group) in dedup_report.duplicate_groups.iter().enumerate() {
                    println!(
                        "  组 #{}: {:?} × {}",
                        i + 1,
                        self.errors[group[0]].error,
                        group.len()
                    );
                }
            }

            // 2. 循环模式检测
            let patterns = self.detect_repeating_patterns();
            if !patterns.is_empty() {
                println!("\n🔄 循环模式警告:");
                for pattern in &patterns {
                    let styled_pattern =
                        fmt.warning_style().apply_to(format!("'{}'", pattern.error_pattern));
                    println!("  ⚠️ {} 在 {} 个位置重复:", styled_pattern, pattern.locations.len());
                    for loc in &pattern.locations {
                        println!("     → {}:{}", loc.file, loc.line);
                    }
                }
            }

            // 3. 聚类摘要
            let clusters = self.cluster_errors_simple();
            if clusters.len() > 1 {
                println!("\n📊 错误聚类 ({} 组):", clusters.len());
                for cluster in &clusters {
                    println!(
                        "  📦 {}: {} 个错误 (风险分: {:.0})",
                        cluster.name, cluster.cluster_stats.size, cluster.cluster_stats.risk_score
                    );
                }
            }

            // 4. 推荐修复顺序
            let priority_queue = self.get_practical_fix_priority();
            if !priority_queue.is_empty() {
                println!("\n🎯 推荐修复顺序:");
                for item in priority_queue.iter().take(5) {
                    // 显示TOP 5
                    let error = &self.errors[item.error_id];
                    let priority_emoji = match item.priority {
                        1 => "🔴",
                        2 => "🟠",
                        3 => "🟡",
                        _ => "⚪",
                    };
                    let styled_severity = fmt
                        .severity_style(error.severity)
                        .apply_to(format!("{:?}", error.severity));
                    println!(
                        "  {}️⃣ #{} {} @ 指令{} (得分: {:.0})",
                        priority_emoji,
                        item.error_id,
                        styled_severity,
                        error.instruction_count,
                        item.score
                    );

                    // 显示增强建议
                    if let Some(first_sug) = item.enhanced_suggestions.first() {
                        println!(
                            "      {}",
                            fmt.success_style().apply_to(format!("💡 {}", first_sug.title))
                        );
                    }
                }
            }
        }

        // 总结框
        println!("\n{}", fmt.top_border());
        let worst_severity = if self.errors.iter().any(|e| e.severity == ErrorSeverity::Fatal) {
            ErrorSeverity::Fatal
        } else {
            ErrorSeverity::Error
        };
        let summary = fmt.severity_style(worst_severity).apply_to(format!(
            "🎯 总结: 共发现 {} 个错误，建议按优先级逐个修复",
            self.errors.len()
        ));
        println!("{}", summary);
        println!("{}", fmt.bottom_border());
    }

    /// 打印统计摘要（使用 DiagnosticFormatter 着色）
    fn print_summary_with_fmt(&self, fmt: &DiagnosticFormatter) {
        println!("\n{}", fmt.section_header("📈", "统计摘要"));
        println!("  总错误数: {}", fmt.error_style().apply_to(self.stats.total_errors.to_string()));
        println!("  执行指令数: {}", self.instruction_counter);
        println!(
            "  错误率: {:.2}%",
            self.stats.total_errors as f64 / self.instruction_counter as f64 * 100.0
        );

        println!("\n  按严重程度分布:");
        for severity in &[
            ErrorSeverity::Fatal,
            ErrorSeverity::Error,
            ErrorSeverity::Warning,
            ErrorSeverity::Info,
        ] {
            let count = self.stats.severity_counts.get(severity).copied().unwrap_or(0);
            if count > 0 {
                let emoji = fmt.severity_emoji(*severity);
                let styled_count = fmt.severity_style(*severity).apply_to(count.to_string());
                println!(
                    "    {} {}: {} ({:.0}%)",
                    emoji,
                    severity,
                    styled_count,
                    count as f64 / self.stats.total_errors as f64 * 100.0
                );
            }
        }

        println!("\n  按类别分布:");
        for category in &[
            ErrorCategory::Arithmetic,
            ErrorCategory::TypeMismatch,
            ErrorCategory::Memory,
            ErrorCategory::ControlFlow,
            ErrorCategory::UndefinedBehavior,
            ErrorCategory::Assertion,
            ErrorCategory::Internal,
            ErrorCategory::Other,
        ] {
            let count = self.stats.category_counts.get(category).copied().unwrap_or(0);
            if count > 0 {
                println!(
                    "    {}: {} ({:.0}%)",
                    category,
                    count,
                    count as f64 / self.stats.total_errors as f64 * 100.0
                );
            }
        }

        if !self.stats.error_prone_instructions.is_empty() {
            println!("\n  高频出错指令 TOP 5:");
            let mut sorted: Vec<_> = self.stats.error_prone_instructions.iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(a.1));
            for (instr, count) in sorted.iter().take(5) {
                println!(
                    "    {:20}: {} 次",
                    instr,
                    fmt.warning_style().apply_to(count.to_string())
                );
            }
        }
    }

    /// 打印调用图分析（使用 DiagnosticFormatter 着色，修复空循环体）
    fn print_call_graph_analysis_with_fmt(&self, fmt: &DiagnosticFormatter) {
        println!("{}", fmt.section_header("🔄", "调用图分析"));

        // 统计每个函数的错误数
        let mut function_errors: XxHashMap<String, usize> = xx_hash_map_new();

        for error in &self.errors {
            if let Some(top_frame) = error.call_stack.last() {
                *function_errors.entry(top_frame.function_name.clone()).or_insert(0) += 1;
            }
        }

        if function_errors.is_empty() {
            println!("  (无调用栈信息)");
            return;
        }

        println!("\n  按函数统计错误数:");
        let mut sorted: Vec<_> = function_errors.into_iter().collect();
        sorted.sort_by_key(|b| std::cmp::Reverse(b.1));

        for (func, count) in &sorted {
            // 柱状图：每个错误占2个字符宽度，上限30字符防止溢出
            let bar_len = (*count as f64 * 2.0).min(30.0) as usize;
            let bar = "█".repeat(bar_len);
            let styled_bar = fmt.warning_style().apply_to(&bar);
            println!("  {:30} │ {} ({})", func, styled_bar, count);
        }
    }

    /// 打印修复优先级建议（使用 DiagnosticFormatter 着色）
    fn print_fix_priority_with_fmt(&self, fmt: &DiagnosticFormatter) {
        println!("{}", fmt.section_header("🎯", "修复优先级建议"));

        // 按严重程度排序
        let mut prioritized: Vec<_> = self.errors.iter().collect();
        prioritized.sort_by_key(|a| a.severity);

        println!("\n  🔴 立即修复 (致命/严重错误):");
        let mut has_urgent = false;
        for error in &prioritized {
            if error.severity == ErrorSeverity::Fatal || error.severity == ErrorSeverity::Error {
                let styled_msg = fmt
                    .error_style()
                    .apply_to(format!("[{}] Error #{}: {}", error.category, error.id, error.error));
                println!("    • {}", styled_msg);
                has_urgent = true;
            }
        }
        if !has_urgent {
            println!("    (无)");
        }

        println!("\n  🟡 尽快修复 (警告):");
        let mut has_warning = false;
        // 按消息去重分组，避免刷屏
        let mut warning_groups: XxHashMap<String, (usize, usize)> = xx_hash_map_new();
        for error in &prioritized {
            if error.severity == ErrorSeverity::Warning {
                let msg = error.error.to_string();
                let entry = warning_groups.entry(msg).or_insert((error.context.ip, 0));
                entry.1 += 1;
                has_warning = true;
            }
        }
        if has_warning {
            let mut groups: Vec<_> = warning_groups.into_iter().collect();
            groups.sort_by_key(|&(_, (_, count))| std::cmp::Reverse(count));
            for (msg, (ip, count)) in &groups {
                let styled_msg = fmt.warning_style().apply_to(msg);
                if *count == 1 {
                    println!("    • [Warning] {} @ IP={}", styled_msg, ip);
                } else {
                    println!("    • [Warning] {} @ IP={} (×{}次)", styled_msg, ip, count);
                }
            }
            // 去重汇总
            let total_warnings: usize =
                prioritized.iter().filter(|e| e.severity == ErrorSeverity::Warning).count();
            let unique_groups = groups.len();
            if total_warnings > unique_groups {
                println!(
                    "    └─ 共 {} 条警告，合并为 {} 种不同模式 (降噪 {:.0}%)",
                    total_warnings,
                    unique_groups,
                    (1.0 - unique_groups as f64 / total_warnings as f64) * 100.0
                );
            }
        } else {
            println!("    (无)");
        }

        // 最佳实践建议
        println!("\n  💡 通用最佳实践建议:");
        let suggestions = [
            "所有算术运算前进行类型检查 (is_number())",
            "除法运算前检查除数是否为零",
            "使用 try_* 方法处理可能失败的操作",
            "启用单元测试覆盖边界条件",
            "使用此诊断模式定期扫描代码",
        ];
        for (i, sug) in suggestions.iter().enumerate() {
            println!("    {}", fmt.success_style().apply_to(format!("{}. {}", i + 1, sug)));
        }
    }

    /// 打印轻量诊断报告（仅摘要 + TOP 5 错误 + 建议）
    ///
    /// 相比 `print_full_report` 的完整输出，此方法只输出核心信息：
    /// - 3 行快速摘要（错误数/指令数/错误率）
    /// - TOP 5 错误（每个 2 行：错误 + 首条建议）
    /// - 1 行修复建议
    ///
    /// 适用于 CI/CD 管道或终端空间有限的场景。
    pub fn print_compact_report(&self) {
        let fmt = DiagnosticFormatter::new();

        if !self.enabled {
            println!("{}", fmt.warning_style().apply_to("⚠️ 诊断模式未启用，无报告可生成"));
            return;
        }

        if self.errors.is_empty() {
            println!("{}", fmt.success_style().apply_to("✅ 诊断报告: 未发现任何错误！"));
            return;
        }

        // 摘要（3行）
        println!("{}", fmt.section_header("📊", "快速诊断"));
        println!(
            "  错误: {} | 指令: {} | 错误率: {:.1}%",
            fmt.error_style().apply_to(self.stats.total_errors.to_string()),
            self.instruction_counter,
            self.stats.total_errors as f64 / self.instruction_counter as f64 * 100.0
        );

        // TOP 5 错误（每个2行）
        let priority = self.get_practical_fix_priority();
        println!("{}", fmt.section_header("🎯", "TOP 5 错误"));
        for item in priority.iter().take(5) {
            let error = &self.errors[item.error_id];
            let emoji = fmt.severity_emoji(error.severity);
            let label = fmt.severity_label(error.severity);
            let error_msg = error
                .nuzo_error
                .as_ref()
                .map(|e| e.to_string())
                .unwrap_or_else(|| error.error.to_string());
            let styled_msg = fmt
                .severity_style(error.severity)
                .apply_to(format!("#{} {}: {}", error.id, label, error_msg));
            println!("  {} {}", emoji, styled_msg);
            if let Some(first_sug) = item.enhanced_suggestions.first() {
                println!("    {}", fmt.success_style().apply_to(format!("💡 {}", first_sug.title)));
            }
        }

        // 修复建议（1行）
        let fatal_count =
            self.stats.severity_counts.get(&ErrorSeverity::Fatal).copied().unwrap_or(0);
        let error_count =
            self.stats.severity_counts.get(&ErrorSeverity::Error).copied().unwrap_or(0);
        if fatal_count > 0 {
            println!(
                "  {}",
                fmt.fatal_style()
                    .apply_to(format!("💥 发现 {} 个致命错误，必须立即修复！", fatal_count))
            );
        } else if error_count > 0 {
            println!(
                "  {}",
                fmt.error_style().apply_to(format!("❌ 发现 {} 个错误，建议优先修复", error_count))
            );
        }
    }

    // ========================================================================
    // JSON Export Functionality
    // ========================================================================

    /// 导出为美化的 JSON 格式（带缩进和换行）
    ///
    /// # Returns
    ///
    /// 包含所有错误信息的 JSON 字符串（美化格式）
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let collector = ErrorCollector::new();
    /// // ... 收集一些错误 ...
    /// let json = collector.export_json_pretty();
    /// println!("{}", json);
    /// ```
    pub fn export_json_pretty(&self) -> String {
        match serde_json::to_string_pretty(&self.errors) {
            Ok(json) => json,
            Err(e) => {
                log::error!("JSON 序列化失败: {}", e);
                String::from("{\"error\": \"序列化失败\"}")
            }
        }
    }

    /// 导出为紧凑的 JSON 格式（无多余空白）
    ///
    /// # Returns
    ///
    /// 包含所有错误信息的 JSON 字符串（紧凑格式）
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let collector = ErrorCollector::new();
    /// let json = collector.export_json_compact();
    /// // 适合网络传输或存储
    /// ```
    pub fn export_json_compact(&self) -> String {
        match serde_json::to_string(&self.errors) {
            Ok(json) => json,
            Err(e) => {
                log::error!("JSON 序列化失败: {}", e);
                String::from("{\"error\":\"序列化失败\"}")
            }
        }
    }

    /// 导出为 JSON 格式（默认使用美化输出）
    ///
    /// 此方法为了向后兼容而保留，建议使用 `export_json_pretty()` 或 `export_json_compact()`
    ///
    /// # Returns
    ///
    /// 包含所有错误信息的 JSON 字符串
    pub fn export_json(&self) -> String {
        self.export_json_pretty()
    }

    /// 将错误数据导出到文件
    ///
    /// # Arguments
    ///
    /// * `path` - 输出文件路径
    ///
    /// # Returns
    ///
    /// - `Ok(())` - 导出成功
    /// - `Err(std::io::Error)` - 文件写入失败
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut collector = ErrorCollector::new();
    /// collector.enable();
    /// // ... 收集错误 ...
    /// collector.export_to_file("errors.json")?;
    /// ```
    pub fn export_to_file(&self, path: &str) -> Result<(), std::io::Error> {
        let json = self.export_json_pretty();
        let mut file = File::create(path)?;
        file.write_all(json.as_bytes())?;
        file.write_all(b"\n")?; // 添加换行符
        Ok(())
    }

    /// 获取统计信息的 JSON 表示
    ///
    /// # Returns
    ///
    /// 包含统计信息的 `serde_json::Value` 对象
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let stats_value = collector.get_json_stats();
    /// let stats_string = serde_json::to_string_pretty(&stats_value).unwrap();
    /// println!("{}", stats_string);
    /// ```
    pub fn get_json_stats(&self) -> JsonValue {
        serde_json::to_value(&self.stats)
            .unwrap_or_else(|_| JsonValue::Object(serde_json::Map::new()))
    }

    /// 导出完整报告（包含错误列表和统计信息）为 JSON
    ///
    /// 这是一个高级方法，将所有诊断信息整合到一个完整的 JSON 报告中
    ///
    /// # Returns
    ///
    /// 完整的 JSON 报告字符串
    ///
    /// # JSON 结构
    ///
    /// ```text
    /// {
    ///   "metadata": {
    ///     "generated_at": "ISO8601 timestamp",
    ///     "total_errors": number,
    ///     "instruction_count": number,
    ///     "diagnostic_enabled": bool
    ///   },
    ///   "statistics": { ... },
    ///   "errors": [ ... ]
    /// }
    /// ```
    pub fn export_full_report(&self) -> String {
        let mut report = serde_json::Map::new();

        // 元数据
        let mut metadata = serde_json::Map::new();
        metadata.insert("total_errors".to_string(), JsonValue::Number(self.errors.len().into()));
        metadata.insert(
            "instruction_count".to_string(),
            JsonValue::Number(self.instruction_counter.into()),
        );
        metadata.insert("diagnostic_enabled".to_string(), JsonValue::Bool(self.enabled));

        report.insert("metadata".to_string(), JsonValue::Object(metadata));

        // 统计信息
        report.insert("statistics".to_string(), self.get_json_stats());

        // 错误列表
        let errors_json: Result<JsonValue, _> = serde_json::to_value(&self.errors);
        match errors_json {
            Ok(errors) => {
                report.insert("errors".to_string(), errors);
            }
            Err(e) => {
                log::warn!("错误列表序列化失败: {}", e);
                report.insert("errors".to_string(), JsonValue::Array(vec![]));
            }
        }

        // 序列化整个报告
        match serde_json::to_string_pretty(&JsonValue::Object(report)) {
            Ok(json) => json,
            Err(e) => {
                log::error!("完整报告序列化失败: {}", e);
                String::from("{\"error\": \"报告生成失败\"}")
            }
        }
    }

    // ========================================================================
    // Multi-dimensional Weighted Similarity Algorithm
    // ========================================================================

    /// 计算两个错误的相似度 (0.0 - 1.0)
    pub fn calculate_similarity(
        &self,
        a: &DiagnosticError,
        b: &DiagnosticError,
        config: &SimilarityConfig,
    ) -> f64 {
        let type_sim = self.type_similarity(a, b);
        let loc_sim = self.location_similarity(a, b);
        let ctx_sim = self.context_similarity(a, b);
        let temp_sim = self.temporal_similarity(a, b, config.temporal_window);

        config.type_weight * type_sim
            + config.location_weight * loc_sim
            + config.context_weight * ctx_sim
            + config.temporal_weight * temp_sim
    }

    /// 计算错误类型相似度
    fn type_similarity(&self, a: &DiagnosticError, b: &DiagnosticError) -> f64 {
        if mem::discriminant(&a.error.kind) == mem::discriminant(&b.error.kind) {
            1.0
        } else if a.category == b.category {
            0.7
        } else {
            0.0
        }
    }

    /// 计算源码位置相似度
    fn location_similarity(&self, a: &DiagnosticError, b: &DiagnosticError) -> f64 {
        match (&a.context.source_location, &b.context.source_location) {
            (Some(la), Some(lb)) if la == lb => 1.0,
            (Some(la), Some(lb))
                if la.file == lb.file && (la.line as i64 - lb.line as i64).abs() <= 5 =>
            {
                0.9
            }
            (Some(la), Some(lb)) if la.file == lb.file => 0.7,
            (Some(_), Some(_)) => 0.3,
            _ => 0.5,
        }
    }

    /// 计算执行上下文相似度
    fn context_similarity(&self, a: &DiagnosticError, b: &DiagnosticError) -> f64 {
        let opcode_match = a.context.opcode == b.context.opcode;
        let reg_overlap =
            Self::calc_register_overlap(&a.context.register_snapshot, &b.context.register_snapshot);

        if opcode_match && reg_overlap > 0.8 {
            1.0
        } else if opcode_match || reg_overlap > 0.5 {
            0.7
        } else {
            0.3
        }
    }

    /// 计算时间相似度
    fn temporal_similarity(&self, a: &DiagnosticError, b: &DiagnosticError, window: usize) -> f64 {
        let gap = (a.instruction_count as i64 - b.instruction_count as i64).unsigned_abs() as usize;
        if gap == 0 {
            1.0
        } else if gap < window {
            1.0 - (gap as f64 / window as f64)
        } else {
            0.0
        }
    }

    /// 计算寄存器重叠度（Jaccard 相似系数）
    fn calc_register_overlap(a: &[(usize, Value)], b: &[(usize, Value)]) -> f64 {
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }

        let set_a: HashSet<_> = a.iter().map(|(idx, _)| idx).collect();
        let set_b: HashSet<_> = b.iter().map(|(idx, _)| idx).collect();

        let intersection = set_a.intersection(&set_b).count();
        let union = set_a.union(&set_b).count();

        if union == 0 { 0.0 } else { intersection as f64 / union as f64 }
    }

    // ========================================================================
    // Smart Deduplication
    // ========================================================================

    /// 合并高度相似的错误 (>95%相似度视为重复)
    ///
    /// # Performance
    ///
    /// **时间复杂度**: O(N²) where N = number of errors
    /// - 实际性能：N ≤ 100 时 < 1ms, N ≤ 1000 时 < 100ms
    /// - 适用场景：单次运行错误数通常 < 500，完全可接受
    ///
    /// # Future Optimization
    ///
    /// 如果未来需要处理 > 10000 个错误，可考虑：
    /// - **LSH (Locality-Sensitive Hashing)**: 近似最近邻搜索，O(N) 预处理 + O(log N) 查询
    /// - **空间索引**: KD-Tree 或 Ball Tree 对错误向量建立索引
    /// - **分块处理**: 先按 ErrorCategory 分桶，再在桶内计算相似度
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let report = collector.smart_deduplicate();
    /// println!("去重: {} → {}", report.original_count, report.remaining_after_dedup);
    /// ```
    pub fn smart_deduplicate(&self) -> DeduplicationReport {
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut processed = std::collections::HashSet::new();

        for i in 0..self.errors.len() {
            if processed.contains(&i) {
                continue;
            }

            let mut group = vec![i];
            processed.insert(i);

            for j in (i + 1)..self.errors.len() {
                if processed.contains(&j) {
                    continue;
                }

                let sim = self.calculate_similarity(
                    &self.errors[i],
                    &self.errors[j],
                    &SimilarityConfig::default(),
                );

                if sim > 0.95 {
                    group.push(j);
                    processed.insert(j);
                }
            }

            if group.len() > 1 {
                groups.push(group);
            }
        }

        // 先计算剩余数量，避免所有权冲突
        let duplicates_count = groups.iter().map(|g| g.len() - 1).sum::<usize>();
        let remaining = self.errors.len() - duplicates_count;

        DeduplicationReport {
            original_count: self.errors.len(),
            duplicate_groups: groups,
            remaining_after_dedup: remaining,
        }
    }

    // ========================================================================
    // Repeating Pattern Detection
    // ========================================================================

    /// 检测相同类型错误在不同位置的重复出现
    #[allow(clippy::type_complexity)] // 内部局部变量类型复杂，提取为 type alias 不值得（仅此一处使用）
    pub fn detect_repeating_patterns(&self) -> Vec<ErrorPattern> {
        // 使用 discriminant 进行粗粒度分组（只区分类别，不区分参数）
        let mut type_groups: XxHashMap<
            (std::mem::Discriminant<nuzo_values::errors::NuzoErrorKind>, ErrorCategory),
            Vec<(usize, String)>,
        > = xx_hash_map_new();

        // 按错误类型分组（使用 discriminant 作为主键）
        for (idx, error) in self.errors.iter().enumerate() {
            // 使用 discriminant 进行粗粒度分组（只区分类别，不区分参数）
            let discriminant = std::mem::discriminant(&error.error.kind);
            let type_name = format!("{:?}", error.error.kind); // 只提取 kind 名称用于显示

            // 组合键：discriminant (主) + ErrorCategory (辅)
            type_groups
                .entry((discriminant, error.category.clone()))
                .or_default()
                .push((idx, type_name));
        }

        // 过滤出出现>=2次的模式并构建结构化信息
        type_groups
            .into_iter()
            .filter(|(_, items)| items.len() >= 2)
            .enumerate()
            .map(|(pattern_id, (_key, items))| {
                let ids: Vec<usize> = items.iter().map(|(idx, _)| *idx).collect();
                let variants: Vec<String> = items.iter().map(|(_, name)| name.clone()).collect();

                // 使用出现频率最高的具体类型名作为 error_pattern（或第一个）
                let error_pattern =
                    variants.first().cloned().unwrap_or_else(|| "Unknown".to_string());

                let locations: Vec<SourceLocation> = ids
                    .iter()
                    .filter_map(|&i| self.errors[i].context.source_location.clone())
                    .collect();

                let functions: Vec<String> = ids
                    .iter()
                    .filter_map(|&i| {
                        self.errors[i].call_stack.last().map(|f| f.function_name.clone())
                    })
                    .collect();

                ErrorPattern {
                    pattern_id,
                    error_pattern,
                    occurrence_count: ids.len(),
                    locations,
                    affected_functions: functions,
                    time_range: (
                        ids.first().map(|&i| self.errors[i].instruction_count).unwrap_or(0),
                        ids.last().map(|&i| self.errors[i].instruction_count).unwrap_or(0),
                    ),
                    pattern_severity: Self::calculate_pattern_severity(
                        &ids.iter().map(|&i| &self.errors[i]).collect::<Vec<_>>(),
                    ),
                    variants,
                }
            })
            .collect()
    }

    /// 计算模式的严重程度（取所有相关错误中的最高严重程度）
    fn calculate_pattern_severity(errors: &[&DiagnosticError]) -> ErrorSeverity {
        if errors.iter().any(|e| e.severity == ErrorSeverity::Fatal) {
            ErrorSeverity::Fatal
        } else if errors.iter().any(|e| e.severity == ErrorSeverity::Error) {
            ErrorSeverity::Error
        } else {
            ErrorSeverity::Warning
        }
    }

    // ========================================================================
    // Error Clustering and Priority System
    // ========================================================================

    /// 基于类别+严重程度的简单聚类
    pub fn cluster_errors_simple(&self) -> Vec<ErrorCluster> {
        let mut clusters_map: XxHashMap<(ErrorSeverity, ErrorCategory), Vec<usize>> =
            xx_hash_map_new();

        for (idx, error) in self.errors.iter().enumerate() {
            let key = (error.severity, error.category.clone());
            clusters_map.entry(key).or_default().push(idx);
        }

        clusters_map
            .into_iter()
            .enumerate()
            .map(|(cluster_id, ((severity, category), error_ids))| {
                let representative = error_ids[0];

                ErrorCluster {
                    cluster_id,
                    name: format!("{:?}-{:?}组", severity, category),
                    error_ids: error_ids.clone(),
                    representative_error: representative,
                    cluster_stats: {
                        // 计算严重程度分布
                        let mut severity_dist: XxHashMap<ErrorSeverity, usize> = xx_hash_map_new();
                        for &error_id in &error_ids {
                            let sev = self.errors[error_id].severity;
                            *severity_dist.entry(sev).or_insert(0) += 1;
                        }

                        // 计算真实的平均内部相似度（抽样计算，避免O(n²)全量计算）
                        let avg_sim = if error_ids.len() > 1 {
                            // 抽样前10对（或全部如果<=10）
                            let sample_size = error_ids.len().min(SIMILARITY_SAMPLE_SIZE);
                            let mut total_sim = 0.0_f64;
                            let mut pair_count = 0_usize;

                            for i in 0..sample_size {
                                for j in (i + 1)..sample_size {
                                    if j < error_ids.len() {
                                        let sim = self.calculate_similarity(
                                            &self.errors[error_ids[i]],
                                            &self.errors[error_ids[j]],
                                            &SimilarityConfig::default(),
                                        );
                                        total_sim += sim;
                                        pair_count += 1;
                                    }
                                }
                            }

                            if pair_count > 0 {
                                total_sim / pair_count as f64
                            } else {
                                DEFAULT_AVG_SIMILARITY
                            } // 默认值
                        } else {
                            1.0 // 单元素聚类完全相似
                        };

                        ClusterStatistics {
                            size: error_ids.len(),
                            severity_distribution: severity_dist,
                            avg_internal_similarity: avg_sim,
                            risk_score: Self::calculate_risk_score(severity, error_ids.len()),
                        }
                    },
                }
            })
            .collect()
    }

    fn calculate_risk_score(severity: ErrorSeverity, count: usize) -> f64 {
        let severity_factor = match severity {
            ErrorSeverity::Fatal => RISK_SEVERITY_FATAL,
            ErrorSeverity::Error => RISK_SEVERITY_ERROR,
            ErrorSeverity::Warning => RISK_SEVERITY_WARNING,
            ErrorSeverity::Info => RISK_SEVERITY_INFO,
        };
        let count_factor = (count as f64 * RISK_COUNT_MULTIPLIER).min(RISK_COUNT_MAX);
        severity_factor + count_factor
    }

    /// 基于简单规则的修复优先级排序
    pub fn get_practical_fix_priority(&self) -> Vec<PrioritizedError> {
        let mut prioritized: Vec<PrioritizedError> = self
            .errors
            .iter()
            .enumerate()
            .map(|(id, error)| {
                let score = Self::calculate_practical_priority(error);

                PrioritizedError {
                    error_id: id,
                    priority: 0,
                    score,
                    score_breakdown: PriorityScoreBreakdown {
                        severity_score: match error.severity {
                            ErrorSeverity::Fatal => PRIORITY_SEVERITY_FATAL,
                            ErrorSeverity::Error => PRIORITY_SEVERITY_ERROR,
                            ErrorSeverity::Warning => PRIORITY_SEVERITY_WARNING,
                            ErrorSeverity::Info => PRIORITY_SEVERITY_INFO,
                        },
                        impact_score: PRIORITY_IMPACT_BASE,
                        frequency_score: self.estimate_frequency(error),
                        fixability_score: Self::estimate_fixability(&error.error, &error.context),
                        context_importance: PRIORITY_CONTEXT_BASE,
                    },
                    enhanced_suggestions: Self::generate_enhanced_suggestions(error),
                }
            })
            .collect();

        prioritized
            .sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        for (i, item) in prioritized.iter_mut().enumerate() {
            item.priority = i + 1;
        }

        prioritized
    }

    fn calculate_practical_priority(error: &DiagnosticError) -> f64 {
        match error.severity {
            ErrorSeverity::Fatal => PRACTICAL_SEVERITY_FATAL,
            ErrorSeverity::Error => PRACTICAL_SEVERITY_ERROR,
            ErrorSeverity::Warning => PRACTICAL_SEVERITY_WARNING,
            ErrorSeverity::Info => PRACTICAL_SEVERITY_INFO,
        }
    }

    fn estimate_frequency(&self, error: &DiagnosticError) -> f64 {
        // 基础频率：全局占比 (0-15分)
        let global_freq = if self.stats.total_errors > 0 {
            let cat_count = self.stats.category_counts.get(&error.category).copied().unwrap_or(0);
            (cat_count as f64 / self.stats.total_errors as f64) * FREQ_GLOBAL_MAX_SCORE
        } else {
            0.0
        };

        // 时间局部性：检查最近50条指令内是否有同类错误 (0-5分)
        let time_locality = {
            let recent_window = 50_usize;
            let current_ip = error.instruction_count;

            // 查找在 [current_ip - window, current_ip] 范围内的同类错误数量
            let recent_same_category = self
                .errors
                .iter()
                .filter(|e| {
                    e.id != error.id &&  // 排除自身
                    e.category == error.category &&
                    e.instruction_count >= current_ip.saturating_sub(recent_window) &&
                    e.instruction_count <= current_ip
                })
                .count();

            // 最近有同类错误 → 更紧急（可能是爆发式问题）
            match recent_same_category {
                0 => 0.0,
                1 => 2.0,
                2..=3 => 4.0,
                _ => 5.0, // 3个以上同类错误密集出现
            }
        };

        global_freq + time_locality
    }

    fn estimate_fixability(error: &NuzoError, context: &ExecutionContext) -> f64 {
        let base_score = match &error.kind {
            ErrorKind::DivisionByZero => FIXABILITY_DIV_BY_ZERO, // 简单：添加零值检查
            ErrorKind::IndexOutOfBounds { .. } => FIXABILITY_INDEX_OOB, // 中等：边界检查
            ErrorKind::TypeMismatch { .. } => FIXABILITY_TYPE_MISMATCH, // 中等：类型转换
            ErrorKind::ArithmeticOverflow => FIXABILITY_ARITH_OVERFLOW, // 较难：需要checked math
            ErrorKind::AssertFailed { .. } => FIXABILITY_ASSERT_FAILED, // 简单：检查条件
            ErrorKind::ExpectedNumber { .. } => FIXABILITY_EXPECTED_NUMBER,
            ErrorKind::UndefinedVariable { .. } => FIXABILITY_UNDEF_VAR,
            ErrorKind::InvalidArgumentCount { .. } => FIXABILITY_INVALID_ARG_COUNT, // 中等：检查参数数量
            ErrorKind::UnsupportedOperation { .. } => FIXABILITY_UNSUPPORTED_OP, // 中等：检查类型支持的操作
            // Internal errors — hard to fix
            _ if matches!(error.kind, ErrorKind::Internal(_, _)) => FIXABILITY_INTERNAL,
            // Other errors
            _ => FIXABILITY_DEFAULT,
        };

        // 上下文加成（最多±3分）
        let context_bonus: f64 = {
            // 如果错误发生在循环/函数内部 → 修复影响范围大，难度略增 (-1)
            if context.call_depth > FIXABILITY_DEEP_CALL_THRESHOLD {
                -1.0_f64
            }
            // 如果有明确的操作码提示 → 更容易定位 (+1)
            // 如果寄存器快照丰富 → 调试信息充足 (+1)
            else if context.opcode.is_some() || !context.register_snapshot.is_empty() {
                1.0_f64
            } else {
                0.0_f64
            }
        };

        (base_score + context_bonus).clamp(0.0_f64, FIXABILITY_MAX_SCORE)
    }

    fn generate_enhanced_suggestions(error: &DiagnosticError) -> Vec<EnhancedFixSuggestion> {
        let mut suggestions = Vec::new();

        match &error.error.kind {
            ErrorKind::DivisionByZero => {
                suggestions.push(EnhancedFixSuggestion {
                    title: "添加除数零值检查".to_string(),
                    description: "在除法操作前验证除数不为零".to_string(),
                    difficulty: FixDifficulty::Easy,
                    estimated_effort: EffortLevel::Trivial,
                    has_code_example: true,
                    related_chains: vec![],
                });
            }
            ErrorKind::TypeMismatch { expected, actual } => {
                suggestions.push(EnhancedFixSuggestion {
                    title: format!("类型转换: {} → {}", actual, expected),

                    description: format!("使用显式转换函数将 {} 转换为 {}", actual, expected)
                        .to_string(),
                    difficulty: FixDifficulty::Medium,
                    estimated_effort: EffortLevel::Small,
                    has_code_example: false,
                    related_chains: vec![],
                });
            }
            _ => {
                suggestions.push(EnhancedFixSuggestion {
                    title: "检查操作数类型和范围".to_string(),
                    description: "确保所有操作数符合预期类型且在有效范围内".to_string(),
                    difficulty: FixDifficulty::Medium,
                    estimated_effort: EffortLevel::Medium,
                    has_code_example: false,
                    related_chains: vec![],
                });
            }
        }

        suggestions
    }

    /// 从 ErrorKind 提取 InternalError（用于诊断函数调用）
    ///
    /// 将 Fatal 级别的 ErrorKind::Internal 变体中提取 InternalError，
    /// 以便传递给需要 `&InternalError` 参数的诊断闭包。
    fn extract_internal_error(kind: &ErrorKind) -> Option<InternalError> {
        match kind {
            ErrorKind::Internal(err, _) => Some(err.clone()),
            _ => None,
        }
    }

    /// 设置遇到致命错误时的行为（Builder 风格）
    ///
    /// 使用 `with_` 前缀表示配置方法，返回 `&mut Self` 以支持链式调用。
    /// 注意：当前返回 `()` 以保持向后兼容，未来版本可能改为返回 `&mut Self`。
    pub fn with_stop_on_fatal(&mut self, stop: bool) {
        self.stop_on_fatal(stop);
    }
}

impl std::fmt::Debug for ErrorCollector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ErrorCollector")
            .field("enabled", &self.enabled)
            .field("errors", &self.errors)
            .field("error_counter", &self.error_counter)
            .field("instruction_counter", &self.instruction_counter)
            .field("max_errors", &self.max_errors)
            .field("stop_on_fatal", &self.stop_on_fatal)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl Default for ErrorCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// ErrorSink trait 实现 —— VM → Collector 反向事件流入口
// ============================================================================
//
// 设计要点：
// - `sink_error(&self)` 是 `&self` 方法（非 `&mut self`），靠 SegQueue 的内部
//   可变性实现无锁 push，允许 VM 在共享 `Arc<ErrorCollector>` 下并发上报
// - 本 impl 是"接收"阶段，不做任何重活（不构造 DiagnosticError、不格式化、
//   不打印），保证 VM 热路径零阻塞
// - "处理"阶段推迟到 `drain_sunk`，由调用方在合适时机批量执行
impl ErrorSink for ErrorCollector {
    /// 接收一个错误事件，无锁 push 到 `sunk_events` 队列
    ///
    /// **O(1) 操作**：`SegQueue::push` 仅做原子 CAS，不持锁、不分配锁结构，
    /// 适合 VM 错误热路径。`SegQueue` 是无界队列，`push` 不会失败。
    ///
    /// 注意：本方法不检查 `enabled` 标志——接收阶段总是入队，
    /// 是否计入统计由 `drain_sunk` 根据 `enabled` 决定。
    /// 这样设计的原因：sink 调用方（VM）不应承担检查诊断开关的职责，
    /// 且无锁 push 的开销极低，即便 disabled 时入队也无显著成本。
    #[inline]
    fn sink_error(&self, event: ErrorEvent) {
        self.sunk_events.push(event);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_bytecode::Opcode;
    use nuzo_core::Value;
    use std::fs;

    /// 创建测试用的执行上下文
    fn create_test_context() -> ExecutionContext {
        let mut ctx = ExecutionContext::new(42, Some(Opcode::Div), 3);
        ctx.add_register(0, Value::from_number(10.0));
        ctx.add_register(1, Value::from_number(0.0));
        ctx.operands(vec![0, 1]);
        ctx.source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 10,
            column: 5,
            source_line: Some("let x = a / b".to_string()),
            function_name: None,
        });
        ctx
    }

    /// 创建测试用的调用栈
    fn create_test_call_stack() -> Vec<StackFrameInfo> {
        let mut frame1 = StackFrameInfo::new("main".to_string(), 0);
        frame1.ip_range(0, 100);
        frame1.source("main.nu".to_string(), 1);

        let mut frame2 = StackFrameInfo::new("divide".to_string(), 8);
        frame2.ip_range(50, 60);
        frame2.source("math.nu".to_string(), 15);
        frame2.call_site(SourceLocation {
            file: "main.nu".to_string(),
            line: 10,
            column: 5,
            source_line: None,
            function_name: None,
        });

        vec![frame1, frame2]
    }

    #[test]
    fn test_empty_collector_json_export() {
        let collector = ErrorCollector::new();

        // 测试空错误集合的导出
        let json = collector.export_json_pretty();
        assert!(json.starts_with('['), "JSON 应该以数组开头");
        assert!(json.ends_with(']'), "JSON 应该以数组结尾");
        assert_eq!(json, "[]", "空收集器应该导出空数组");

        // 测试紧凑格式
        let compact = collector.export_json_compact();
        assert_eq!(compact, "[]", "空收集器的紧凑格式应该是 []");

        // 测试默认方法
        let default = collector.export_json();
        assert_eq!(default, "[]", "默认方法应该返回空数组");
    }

    #[test]
    fn test_json_export_valid() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 收集一个错误
        let context = create_test_context();
        let call_stack = create_test_call_stack();
        collector.collect_error(NuzoError::division_by_zero(), context, call_stack);

        // 导出为 JSON
        let json = collector.export_json_pretty();

        // 验证 JSON 格式有效
        let parsed = serde_json::from_str::<Vec<serde_json::Value>>(&json);
        assert!(parsed.is_ok(), "导出的 JSON 应该是有效的");

        // 验证可以解析回错误列表
        let errors = parsed.unwrap();
        assert_eq!(errors.len(), 1, "应该有 1 个错误");

        // 验证错误的基本结构
        if let Some(first_error) = errors.first() {
            assert!(first_error.get("id").is_some(), "错误应该包含 id 字段");
            assert!(first_error.get("severity").is_some(), "错误应该包含 severity 字段");
            assert!(first_error.get("category").is_some(), "错误应该包含 category 字段");
            assert!(first_error.get("context").is_some(), "错误应该包含 context 字段");
        }
    }

    #[test]
    fn test_json_compact_vs_pretty() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let context = create_test_context();
        let call_stack = create_test_call_stack();
        collector.collect_error(NuzoError::division_by_zero(), context, call_stack);

        let pretty = collector.export_json_pretty();
        let compact = collector.export_json_compact();

        // 紧凑格式应该更短
        assert!(compact.len() < pretty.len(), "紧凑格式应该比美化格式短");

        // 两者都应该能被解析
        let pretty_parsed = serde_json::from_str::<serde_json::Value>(&pretty);
        let compact_parsed = serde_json::from_str::<serde_json::Value>(&compact);

        assert!(pretty_parsed.is_ok(), "美化 JSON 应该有效");
        assert!(compact_parsed.is_ok(), "紧凑 JSON 应该有效");

        // 解析后的内容应该相同（忽略空白）
        assert_eq!(
            pretty_parsed.unwrap(),
            compact_parsed.unwrap(),
            "美化和紧凑格式应该表示相同的数据"
        );
    }

    #[test]
    fn test_export_to_file() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let context = create_test_context();
        let call_stack = create_test_call_stack();
        collector.collect_error(NuzoError::division_by_zero(), context, call_stack);

        // 导出到临时文件
        let test_file = "test_errors.json";
        let result = collector.export_to_file(test_file);
        assert!(result.is_ok(), "导出到文件应该成功");

        // 验证文件存在且内容正确
        assert!(fs::metadata(test_file).is_ok(), "导出文件应该存在");

        let content = fs::read_to_string(test_file).expect("读取文件失败");
        let parsed = serde_json::from_str::<serde_json::Value>(&content);
        assert!(parsed.is_ok(), "文件中的 JSON 应该有效");

        // 清理
        let _ = fs::remove_file(test_file);
    }

    #[test]
    fn test_get_json_stats() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 收集多个不同类型的错误
        let context1 = create_test_context();
        let call_stack1 = create_test_call_stack();
        collector.collect_error(NuzoError::division_by_zero(), context1.clone(), call_stack1);

        let context2 = ExecutionContext::new(50, Some(Opcode::Add), 2);
        collector.collect_error(NuzoError::arithmetic_overflow(), context2, vec![]);

        // 获取统计信息
        let stats = collector.get_json_stats();

        // 验证统计信息结构
        assert!(stats.get("total_errors").is_some(), "统计信息应包含 total_errors");
        assert!(stats.get("severity_counts").is_some(), "统计信息应包含 severity_counts");
        assert!(stats.get("category_counts").is_some(), "统计信息应包含 category_counts");

        // 验证值
        if let Some(total) = stats.get("total_errors").and_then(|v| v.as_i64()) {
            assert_eq!(total, 2, "总错误数应为 2");
        }

        // 统计信息应该能序列化为字符串
        let stats_string = serde_json::to_string_pretty(&stats);
        assert!(stats_string.is_ok(), "统计信息应该能序列化");
    }

    #[test]
    fn test_export_full_report() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let context = create_test_context();
        let call_stack = create_test_call_stack();
        collector.collect_error(NuzoError::division_by_zero(), context, call_stack);

        // 导出完整报告
        let report = collector.export_full_report();

        // 验证报告是有效的 JSON
        let parsed = serde_json::from_str::<serde_json::Value>(&report);
        assert!(parsed.is_ok(), "完整报告应该是有效的 JSON");

        let report_obj = parsed.unwrap();

        // 验证报告结构
        assert!(report_obj.get("metadata").is_some(), "报告应包含 metadata");
        assert!(report_obj.get("statistics").is_some(), "报告应包含 statistics");
        assert!(report_obj.get("errors").is_some(), "报告应包含 errors 数组");

        // 验证元数据
        if let Some(metadata) = report_obj.get("metadata") {
            assert!(metadata.get("total_errors").is_some(), "元数据应包含 total_errors");
            assert!(metadata.get("instruction_count").is_some(), "元数据应包含 instruction_count");
            assert!(
                metadata.get("diagnostic_enabled").is_some(),
                "元数据应包含 diagnostic_enabled"
            );
        }
    }

    #[test]
    fn test_large_number_of_errors_export() {
        let mut collector = ErrorCollector::new();
        collector.enable();
        collector.max_errors(500); // 允许更多错误

        // 收集大量错误
        for i in 0..100 {
            let context = ExecutionContext::new(i * 10, Some(Opcode::Div), i % 5 + 1);
            let call_stack = vec![StackFrameInfo::new(format!("func_{}", i % 10), 0)];
            collector.collect_error(NuzoError::division_by_zero(), context, call_stack);
        }

        // 导出并验证
        let json = collector.export_json_compact();
        let parsed = serde_json::from_str::<Vec<serde_json::Value>>(&json);

        assert!(parsed.is_ok(), "大量错误的 JSON 导出应该成功");
        let errors = parsed.unwrap();
        assert_eq!(errors.len(), 100, "应该导出所有 100 个错误");
    }

    #[test]
    fn test_all_error_types_serialization() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 测试所有类型的错误
        let error_types = [
            NuzoError::division_by_zero(),
            NuzoError::arithmetic_overflow(),
            NuzoError::expected_number("string".to_string()),
            NuzoError::type_mismatch("number".to_string(), "string".to_string()),
            NuzoError::index_out_of_bounds("10".to_string(), "5".to_string()),
        ];

        for (i, error) in error_types.iter().enumerate() {
            let context = ExecutionContext::new(i, None, 1);
            collector.collect_error(error.clone(), context, vec![]);
        }

        // 所有类型都应该能成功序列化
        let json = collector.export_json_pretty();
        let parsed = serde_json::from_str::<Vec<serde_json::Value>>(&json);
        assert!(parsed.is_ok(), "所有错误类型都应该能序列化");
        assert_eq!(parsed.unwrap().len(), 5, "应该有 5 个错误");
    }

    #[test]
    fn test_json_contains_fix_suggestions() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let context = create_test_context();
        let call_stack = create_test_call_stack();
        collector.collect_error(NuzoError::division_by_zero(), context, call_stack);

        let json = collector.export_json_pretty();
        let parsed = serde_json::from_str::<Vec<serde_json::Value>>(&json);
        let errors = parsed.unwrap();

        // 错误应该包含修复建议
        if let Some(first) = errors.first() {
            if let Some(suggestions) = first.get("fix_suggestions").and_then(|v| v.as_array()) {
                assert!(!suggestions.is_empty(), "错误应该包含修复建议");
            } else {
                panic!("fix_suggestions 应该是一个数组");
            }
        }
    }

    #[test]
    fn test_execution_context_serialization_details() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let context = create_test_context();
        let call_stack = create_test_call_stack();
        collector.collect_error(NuzoError::division_by_zero(), context, call_stack);

        let json = collector.export_json_pretty();
        let parsed = serde_json::from_str::<Vec<serde_json::Value>>(&json);
        let errors = parsed.unwrap();

        if let Some(error) = errors.first()
            && let Some(ctx) = error.get("context")
        {
            // 验证上下文字段
            assert!(ctx.get("ip").is_some(), "上下文应包含 ip");
            assert!(ctx.get("opcode").is_some(), "上下文应包含 opcode");
            assert!(ctx.get("call_depth").is_some(), "上下文应包含 call_depth");
            assert!(ctx.get("register_snapshot").is_some(), "上下文应包含 register_snapshot");
            assert!(ctx.get("operand_registers").is_some(), "上下文应包含 operand_registers");
            assert!(ctx.get("source_location").is_some(), "上下文应包含 source_location");

            // 验证寄存器快照已转换为字符串
            if let Some(snapshot) = ctx.get("register_snapshot").and_then(|v| v.as_array()) {
                assert!(!snapshot.is_empty(), "寄存器快照不应为空");
                // 每个元素应该是 [index, string_value] 格式
                if let Some(first_reg) = snapshot.first().and_then(|v| v.as_array()) {
                    assert_eq!(first_reg.len(), 2, "每个寄存器条目应有 2 个元素");
                    // 第二个元素应该是字符串
                    assert!(first_reg[1].is_string(), "寄存器值应该被序列化为字符串");
                }
            }
        }
    }

    #[test]
    fn test_disabled_collector_export() {
        // 未启用的收集器也应该能导出（只是没有数据）
        let collector = ErrorCollector::new(); // 默认未启用

        let json = collector.export_json_pretty();
        assert_eq!(json, "[]", "未启用的收集器应该返回空数组");

        let full_report = collector.export_full_report();
        let parsed = serde_json::from_str::<serde_json::Value>(&full_report);
        assert!(parsed.is_ok(), "完整报告应该即使在没有错误时也能生成");
    }
}

#[cfg(test)]
mod smart_diagnostics_tests {
    use super::*;

    #[test]
    fn test_smart_deduplicate_identical_errors() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 收集3个完全相同的 DivisionByZero 错误（包括完全相同的上下文）
        for _ in 0..3 {
            let mut ctx = ExecutionContext::new(100, Some(Opcode::Div), 1);
            ctx.add_register(0, Value::from_number(10.0));
            ctx.add_register(1, Value::from_number(0.0));
            ctx.operands(vec![0, 1]);
            ctx.source_location(SourceLocation {
                file: "test.nu".to_string(),
                line: 10,
                column: 5,
                source_line: Some("x = a / b".to_string()),
                function_name: None,
            });
            collector.collect_error(NuzoError::division_by_zero(), ctx, vec![]);
        }

        let report = collector.smart_deduplicate();
        assert_eq!(report.original_count, 3);
        assert_eq!(report.duplicate_groups.len(), 1); // 应该检测到1组重复
        assert_eq!(report.remaining_after_dedup, 1); // 去重后剩1个
    }

    #[test]
    fn test_smart_deduplicate_different_errors() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 收集不同类型的错误
        collector.collect_error(
            NuzoError::division_by_zero(),
            ExecutionContext::new(0, None, 0),
            vec![],
        );
        collector.collect_error(
            NuzoError::type_mismatch("number".to_string(), "string".to_string()),
            ExecutionContext::new(10, None, 0),
            vec![],
        );

        let report = collector.smart_deduplicate();
        assert_eq!(report.duplicate_groups.len(), 0); // 不应检测到重复
        assert_eq!(report.remaining_after_dedup, 2);
    }

    #[test]
    fn test_detect_repeating_division_by_zero() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 在不同位置收集 DivisionByZero
        let ctx1 = {
            let mut c = ExecutionContext::new(100, Some(Opcode::Div), 1);
            c.source_location(SourceLocation {
                file: "main.nu".to_string(),
                line: 42,
                column: 5,
                source_line: Some("x = a / b".to_string()),
                function_name: None,
            });
            c
        };

        let ctx2 = {
            let mut c = ExecutionContext::new(200, Some(Opcode::Div), 2);
            c.source_location(SourceLocation {
                file: "utils.nu".to_string(),
                line: 128,
                column: 10,
                source_line: Some("y = c / d".to_string()),
                function_name: None,
            });
            c
        };

        collector.collect_error(NuzoError::division_by_zero(), ctx1.clone(), vec![]);
        collector.collect_error(
            NuzoError::type_mismatch("num".to_string(), "str".to_string()),
            ctx1,
            vec![],
        );
        collector.collect_error(NuzoError::division_by_zero(), ctx2, vec![]);

        let patterns = collector.detect_repeating_patterns();

        assert_eq!(patterns.len(), 1); // 应检测到1个重复模式
        assert_eq!(patterns[0].error_pattern, "DivisionByZero");
        assert_eq!(patterns[0].occurrence_count, 2); // 出现2次
        assert_eq!(patterns[0].locations.len(), 2); // 2个位置
    }

    #[test]
    fn test_cluster_errors_by_category() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 收集不同类别的错误
        collector.collect_error(
            NuzoError::division_by_zero(),
            ExecutionContext::new(0, None, 0),
            vec![],
        );
        collector.collect_error(
            NuzoError::arithmetic_overflow(),
            ExecutionContext::new(10, None, 0),
            vec![],
        );
        collector.collect_error(
            NuzoError::type_mismatch("num".to_string(), "str".to_string()),
            ExecutionContext::new(20, None, 0),
            vec![],
        );

        let clusters = collector.cluster_errors_simple();

        assert!(!clusters.is_empty());
        assert!(clusters.iter().any(|c| c.name.contains("Arithmetic")));
    }

    #[test]
    fn test_fix_priority_ordering() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // Fatal 级别错误应该排在 Error 前面
        // New unified system: ArithmeticOverflow is Error (not Warning), so use InternalError for Fatal
        collector.collect_error(
            NuzoError::division_by_zero(),
            ExecutionContext::new(0, None, 0),
            vec![],
        );
        collector.collect_nuzo_error(
            NuzoError::internal(InternalError::NoChunkLoaded, None),
            ExecutionContext::new(50, None, 0),
            vec![],
            None,
        );

        let queue = collector.get_practical_fix_priority();

        assert_eq!(queue.len(), 2);
        // InternalError (Fatal, 85分) should rank higher than DivisionByZero (Error, 65分)
        assert!(queue[0].score > queue[1].score, "Fatal级别应该比Error级别得分高");
        assert_eq!(queue[0].priority, 1);
    }

    #[test]
    fn test_full_smart_diagnostic_workflow() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 模拟真实场景：收集多种错误（带完整的上下文信息）

        // 创建带丰富上下文的执行上下文（模拟真实的除法操作）
        let create_div_context = |ip: usize| -> ExecutionContext {
            let mut ctx = ExecutionContext::new(ip, Some(Opcode::Div), 1);
            ctx.add_register(0, Value::from_number(10.0));
            ctx.add_register(1, Value::from_number(0.0));
            ctx.operands(vec![0, 1]);
            ctx.source_location(SourceLocation {
                file: "test.nu".to_string(),
                line: 10,
                column: 5,
                source_line: Some("x = a / b".to_string()),
                function_name: None,
            });
            ctx
        };

        // 场景1: 重复的除零错误（2个完全相同 + 1个在不同位置）
        // 前2个完全相同（包括所有上下文），应该被去重
        for _ in 0..2 {
            let ctx = create_div_context(100);
            collector.collect_error(NuzoError::division_by_zero(), ctx, vec![]);
        }
        // 第3个在不同位置，不会被去重但会被模式检测发现
        let ctx_diff_pos = create_div_context(200);
        collector.collect_error(NuzoError::division_by_zero(), ctx_diff_pos, vec![]);

        // 场景2: 相关的类型错误
        let base_ctx = create_div_context(0);
        collector.collect_error(
            NuzoError::type_mismatch("num".to_string(), "nil".to_string()),
            base_ctx.clone(),
            vec![],
        );

        // 场景3: 致命的断言失败
        collector.collect_error(
            NuzoError::assert_failed("value must be positive".to_string()),
            base_ctx,
            vec![],
        );

        // 执行所有智能诊断
        let dedup_report = collector.smart_deduplicate();
        let patterns = collector.detect_repeating_patterns();
        let clusters = collector.cluster_errors_simple();
        let priority_queue = collector.get_practical_fix_priority();

        // 验证结果
        assert_eq!(dedup_report.original_count, 5); // 总共5个错误
        assert!(dedup_report.remaining_after_dedup < 5); // 去重后减少
        // 注意：实际去重数量取决于相似度算法，我们只验证确实发生了去重

        assert_eq!(patterns.len(), 1); // DivisionByZero 重复3次，应检测到1个模式
        assert_eq!(patterns[0].occurrence_count, 3); // 3个DivisionByZero

        assert!(clusters.len() >= 2); // 至少分为2组（算术+其他）

        assert_eq!(priority_queue.len(), 5); // 优先级队列包含所有5个原始错误
        assert_eq!(priority_queue[0].priority, 1); // 第一个优先级最高

        // Error/Fatal级别错误应该排在最前面（高优先级）
        let high_severity_idx = priority_queue
            .iter()
            .position(|p| {
                let sev = collector.errors[p.error_id].severity;
                sev == ErrorSeverity::Fatal || sev == ErrorSeverity::Error
            })
            .expect("应该至少有一个Error或Fatal级别的错误");
        assert!(high_severity_idx < 3); // 高严重程度错误应该在TOP 3内
    }

    #[test]
    fn test_type_mismatch_grouping() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let ctx = ExecutionContext::new(0, None, 0);

        // 两个不同参数的 TypeMismatch 应该合并为同一模式
        collector.collect_error(
            NuzoError::type_mismatch("num".to_string(), "nil".to_string()),
            ctx.clone(),
            vec![],
        );
        collector.collect_error(
            NuzoError::type_mismatch("num".to_string(), "str".to_string()),
            ctx,
            vec![],
        );

        let patterns = collector.detect_repeating_patterns();

        assert_eq!(patterns.len(), 1); // 应该只有1个模式（不是2个）
        assert!(patterns[0].error_pattern.contains("TypeMismatch"));
        assert_eq!(patterns[0].occurrence_count, 2); // 出现2次
        assert_eq!(patterns[0].variants.len(), 2); // 有2个变体
    }
}

// ============================================================================
// NuzoError Support Tests
// ============================================================================

#[cfg(test)]
mod nuzo_error_tests {
    use super::*;
    use nuzo_values::{InternalError, NuzoError, VmDiagnosis};

    /// 创建测试用的 VmDiagnosis
    fn create_test_diagnosis() -> VmDiagnosis {
        VmDiagnosis {
            disassembly: "0000  LoadK     0     0    ; load constant\n0042  ???       ??"
                .to_string(),
            error_ip: Some(0x0042),
            register_snapshot: vec![(0, "42 (Smi)".to_string()), (1, "nil".to_string())],
            call_stack_depth: 1,
            root_cause_analysis: "Invalid opcode found in bytecode. This indicates a compiler bug."
                .to_string(),
        }
    }

    // ========================================================================
    // Test DiagnosticError::from_nuzo_error()
    // ========================================================================

    #[test]
    fn test_from_nuzo_error_program_division_by_zero() {
        let nuzo_err = NuzoError::division_by_zero();
        let ctx = ExecutionContext::new(100, Some(Opcode::Div), 1);

        let diagnostic = DiagnosticError::from_nuzo_error(0, nuzo_err, ctx, vec![], 100, None);

        // 验证基本字段
        assert_eq!(diagnostic.id, 0);
        assert_eq!(diagnostic.severity, ErrorSeverity::Error);
        assert_eq!(diagnostic.category, ErrorCategory::Arithmetic);
        assert_eq!(diagnostic.instruction_count, 100);

        // 验证 NuzoError 字段已设置
        assert!(diagnostic.is_nuzo_error(), "应该标记为 NuzoError");
        assert!(!diagnostic.is_internal_error(), "不应该标记为 InternalError");
        assert!(diagnostic.as_nuzo_error().is_some());
        assert!(diagnostic.diagnosis().is_none()); // ProgramError 无诊断

        // 验证 NuzoError 内容
        match &diagnostic.as_nuzo_error().unwrap().kind {
            ErrorKind::DivisionByZero => {} // Expected
            other => panic!("Expected DivisionByError, got {:?}", other),
        }
    }

    #[test]
    fn test_from_nuzo_error_internal_with_diagnosis() {
        let nuzo_err = NuzoError::internal(InternalError::InvalidOpcode { opcode: 0xFF }, None);
        let diagnosis = create_test_diagnosis();
        let ctx = ExecutionContext::new(0x42, None, 0);

        let diagnostic = DiagnosticError::from_nuzo_error(
            1,
            nuzo_err,
            ctx,
            vec![],
            0x42,
            Some(diagnosis.clone()),
        );

        // 验证 InternalError 特有属性
        assert_eq!(diagnostic.severity, ErrorSeverity::Fatal);
        // New unified system: InvalidOpcode maps to ErrorCategory::Internal
        assert_eq!(diagnostic.category, ErrorCategory::Internal);
        assert!(diagnostic.is_internal_error(), "应该标记为 InternalError");

        // 验证诊断报告已设置
        assert!(diagnostic.diagnosis().is_some(), "应该有诊断报告");
        let diag = diagnostic.diagnosis().unwrap();
        assert_eq!(diag.error_ip, Some(0x0042));
        assert!(!diag.register_snapshot.is_empty());

        // 验证修复建议不为空（InternalError 也应有建议）
        assert!(!diagnostic.fix_suggestions.is_empty(), "InternalError 应该有修复建议");
    }

    #[test]
    fn test_from_nuzo_error_internal_without_diagnosis() {
        let nuzo_err = NuzoError::internal(InternalError::NoChunkLoaded, None);
        let ctx = ExecutionContext::new(0, None, 0);

        let diagnostic = DiagnosticError::from_nuzo_error(
            2,
            nuzo_err,
            ctx,
            vec![],
            0,
            None, // 不提供诊断报告
        );

        assert!(diagnostic.is_internal_error());
        assert!(diagnostic.diagnosis().is_none(), "未提供诊断时应该是 None");
    }

    #[test]
    fn test_backward_compatible_new_still_works() {
        // 验证旧的 API 仍然可以工作
        let ctx = ExecutionContext::new(50, Some(Opcode::Add), 2);
        let diagnostic =
            DiagnosticError::new(10, NuzoError::arithmetic_overflow(), ctx, vec![], 50);

        assert_eq!(diagnostic.id, 10);
        assert!(!diagnostic.is_nuzo_error(), "旧 API 创建的错误不应有 NuzoError");
        assert!(diagnostic.nuzo_error.is_none());
        assert!(diagnostic.diagnosis.is_none());
    }

    // ========================================================================
    // Test Display output for NuzoError
    // ========================================================================

    #[test]
    fn test_display_nuzo_error_program() {
        let nuzo_err = NuzoError::type_mismatch("number".to_string(), "string".to_string());
        let ctx = ExecutionContext::new(10, None, 1);

        let diagnostic = DiagnosticError::from_nuzo_error(0, nuzo_err, ctx, vec![], 10, None);
        let display_str = format!("{}", diagnostic);

        // 验证显示内容包含关键信息
        // Display 格式: "❌ 错误 #N (Error)" 或 "⚠️ 警告 #N (Warning)" 等
        assert!(display_str.contains("#0"), "应包含错误 ID, 实际: {}", display_str);
        assert!(display_str.contains("type mismatch"), "应包含错误消息");
        assert!(!display_str.contains("内部错误"), "TypeMismatch 不应显示内部错误标记");
        assert!(!display_str.contains("VM 诊断报告"), "无诊断时不显示诊断报告");
    }

    #[test]
    fn test_display_nuzo_error_internal_with_diagnosis() {
        let diagnosis = create_test_diagnosis();
        let nuzo_err =
            NuzoError::internal(InternalError::StackOverflow { depth: 256, max_depth: 255 }, None);
        let ctx = ExecutionContext::new(200, None, 5);

        let diagnostic =
            DiagnosticError::from_nuzo_error(5, nuzo_err, ctx, vec![], 200, Some(diagnosis));
        let display_str = format!("{}", diagnostic);

        // 验证显示内容包含关键信息
        assert!(display_str.contains("错误 #5"));
        // New unified system: InternalError Display no longer has "internal error" prefix
        // Instead it shows the specific error kind message (e.g. "stack overflow")
        assert!(display_str.contains("stack overflow"), "应包含 'stack overflow' 错误消息");
        assert!(display_str.contains("内部错误"), "应标记为内部错误");
        assert!(display_str.contains("VM 诊断报告"), "应显示诊断报告部分");
        assert!(display_str.contains("INTERNAL ERROR DIAGNOSIS"), "应包含诊断报告标题");
    }

    // ========================================================================
    // Test ErrorCollector::collect_nuzo_error()
    // ========================================================================

    #[test]
    fn test_collect_nuzo_error_program() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let nuzo_err = NuzoError::division_by_zero();
        let ctx = ExecutionContext::new(100, Some(Opcode::Div), 1);

        let should_continue = collector.collect_nuzo_error(nuzo_err, ctx, vec![], None);

        assert!(should_continue, "非致命错误应该继续执行");
        assert_eq!(collector.error_count(), 1);

        let error = &collector.errors()[0];
        assert!(error.is_nuzo_error());
        assert!(!error.is_internal_error());
        assert_eq!(error.severity, ErrorSeverity::Error);
        assert_eq!(error.category, ErrorCategory::Arithmetic);
    }

    #[test]
    fn test_collect_nuzo_error_internal_fatal() {
        let mut collector = ErrorCollector::new();
        collector.enable();
        collector.stop_on_fatal(true); // 配置为遇到致命错误停止

        let nuzo_err = NuzoError::internal(InternalError::NoChunkLoaded, None);
        let ctx = ExecutionContext::new(0, None, 0);

        let should_continue = collector.collect_nuzo_error(nuzo_err, ctx, vec![], None);

        assert!(!should_continue, "InternalError 是致命错误，应该停止执行");
        assert_eq!(collector.error_count(), 1);

        let error = &collector.errors()[0];
        assert!(error.is_internal_error());
        assert_eq!(error.severity, ErrorSeverity::Fatal);
        // New unified system: NoChunkLoaded maps to ErrorCategory::Internal
        assert_eq!(error.category, ErrorCategory::Internal);
    }

    #[test]
    fn test_collect_nuzo_error_with_diagnosis() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let diagnosis = create_test_diagnosis();
        let nuzo_err = NuzoError::internal(InternalError::InvalidOpcode { opcode: 0xFE }, None);
        let ctx = ExecutionContext::new(0x42, None, 0);

        collector.collect_nuzo_error(nuzo_err, ctx, vec![], Some(diagnosis));

        let error = &collector.errors()[0];
        assert!(error.diagnosis().is_some(), "应该存储诊断报告");

        let diag = error.diagnosis().unwrap();
        assert_eq!(diag.register_snapshot.len(), 2);
        assert!(diag.root_cause_analysis.contains("compiler bug"));
    }

    #[test]
    fn test_collect_mixed_runtime_and_nuzo_errors() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 收集一个旧的 RuntimeError
        collector.collect_error(
            NuzoError::arithmetic_overflow(),
            ExecutionContext::new(10, None, 0),
            vec![],
        );

        // 收集一个 NuzoError::Program
        collector.collect_nuzo_error(
            NuzoError::division_by_zero(),
            ExecutionContext::new(20, None, 0),
            vec![],
            None,
        );

        // 收集一个 NuzoError::Internal
        collector.collect_nuzo_error(
            NuzoError::internal(
                InternalError::StackUnderflow { operation: "ADD".to_string() },
                None,
            ),
            ExecutionContext::new(30, None, 0),
            vec![],
            None,
        );

        assert_eq!(collector.error_count(), 3);

        // 验证每个错误的类型
        let errors = collector.errors();
        assert!(!errors[0].is_nuzo_error(), "第一个错误是旧 API 创建的");
        assert!(
            errors[1].is_nuzo_error() && !errors[1].is_internal_error(),
            "第二个错误是 ProgramError"
        );
        assert!(errors[2].is_internal_error(), "第三个错误是 InternalError");
    }

    // ========================================================================
    // Test handle_error_in_diagnostic_mode()
    // ========================================================================

    #[test]
    fn test_handle_error_in_diagnostic_mode_program_error() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let nuzo_err = NuzoError::assert_failed("test failed".to_string());
        let ctx = ExecutionContext::new(50, None, 1);

        // 对于 ProgramError，diagnose_fn 不应该被调用
        let should_continue =
            collector.handle_error_in_diagnostic_mode(nuzo_err, ctx, vec![], |_ie| {
                panic!("diagnose_fn 不应该对 ProgramError 被调用");
            });

        assert!(should_continue);
        assert_eq!(collector.error_count(), 1);

        let error = &collector.errors()[0];
        assert!(error.is_nuzo_error());
        assert!(!error.is_internal_error());
        assert!(error.diagnosis().is_none(), "ProgramError 不应有诊断报告");
    }

    #[test]
    fn test_handle_error_in_diagnostic_mode_internal_error() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let nuzo_err =
            NuzoError::internal(InternalError::RegisterOutOfBounds { reg: 16, available: 8 }, None);
        let ctx = ExecutionContext::new(75, None, 2);

        let should_continue =
            collector.handle_error_in_diagnostic_mode(nuzo_err, ctx, vec![], |ie| {
                // 模拟 VM 的诊断函数
                assert!(matches!(ie, InternalError::RegisterOutOfBounds { .. }));
                Some(create_test_diagnosis())
            });

        assert!(!should_continue); // Fatal 错误且默认 stop_on_fatal=true
        assert_eq!(collector.error_count(), 1);

        let error = &collector.errors()[0];
        assert!(error.is_internal_error());
        assert!(error.diagnosis().is_some(), "InternalError 应该自动生成诊断报告");
    }

    #[test]
    fn test_handle_error_in_diagnostic_mode_returns_none() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        let nuzo_err = NuzoError::internal(
            InternalError::CompilerBug { message: "bad code".to_string() },
            None,
        );
        let ctx = ExecutionContext::new(0, None, 0);

        // diagnose_fn 返回 None（模拟无法诊断的情况）
        collector.handle_error_in_diagnostic_mode(
            nuzo_err,
            ctx,
            vec![],
            |_ie| None, // 无法生成诊断
        );

        let error = &collector.errors()[0];
        assert!(error.is_internal_error());
        assert!(error.diagnosis().is_none(), "如果 diagnose_fn 返回 None，诊断应为 None");
    }

    // ========================================================================
    // Test JSON serialization with NuzoError fields
    // ========================================================================

    #[test]
    fn test_json_serialization_nuzo_error_fields() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 收集一个带诊断的 NuzoError
        let diagnosis = create_test_diagnosis();
        collector.collect_nuzo_error(
            NuzoError::internal(InternalError::BytecodeOutOfBounds { ip: 100, code_len: 50 }, None),
            ExecutionContext::new(100, None, 0),
            vec![],
            Some(diagnosis),
        );

        // 导出为 JSON 并验证
        let json = collector.export_json_pretty();
        let parsed = serde_json::from_str::<Vec<serde_json::Value>>(&json);
        assert!(parsed.is_ok(), "JSON 序列化应该成功");

        let errors = parsed.unwrap();
        assert_eq!(errors.len(), 1);

        if let Some(error) = errors.first() {
            // 验证新字段存在（即使值为 null 也会被 skip_serializing_if 跳过）
            // 但我们至少验证基本的序列化没有崩溃
            assert!(error.get("id").is_some());
            assert!(error.get("severity").is_some());
            assert!(error.get("category").is_some());
            assert!(error.get("nuzo_error").is_some(), "nuzo_error 字段应该在 JSON 中");
        }
    }

    // ========================================================================
    // Test statistics with mixed error types
    // ========================================================================

    #[test]
    fn test_statistics_with_nuzo_errors() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // 收集多种类型的错误
        collector.collect_error(
            NuzoError::division_by_zero(),
            ExecutionContext::new(0, None, 0),
            vec![],
        );
        collector.collect_nuzo_error(
            NuzoError::arithmetic_overflow(),
            ExecutionContext::new(10, None, 0),
            vec![],
            None,
        );
        collector.collect_nuzo_error(
            NuzoError::internal(InternalError::NoChunkLoaded, None),
            ExecutionContext::new(20, None, 0),
            vec![],
            None,
        );

        let stats = collector.statistics();

        assert_eq!(stats.total_errors, 3);

        // 验证严重程度统计：1 Error + 1 Error + 1 Fatal = 3
        // New unified system: ArithmeticOverflow is Error (not Warning)
        assert_eq!(*stats.severity_counts.get(&ErrorSeverity::Error).unwrap_or(&0), 2);
        assert_eq!(*stats.severity_counts.get(&ErrorSeverity::Fatal).unwrap_or(&0), 1);

        // 验证类别统计
        // New unified system: NoChunkLoaded maps to ErrorCategory::Internal (not UndefinedBehavior)
        assert_eq!(*stats.category_counts.get(&ErrorCategory::Arithmetic).unwrap_or(&0), 2);
        assert_eq!(*stats.category_counts.get(&ErrorCategory::Internal).unwrap_or(&0), 1);
    }
}

// ============================================================================
// ErrorSink trait 实现测试
// ============================================================================
#[cfg(test)]
mod sink_tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_error_collector_implements_error_sink() {
        // 编译期验证：ErrorCollector 实现了 ErrorSink trait
        fn _assert_impl<T: ErrorSink>() {}
        _assert_impl::<ErrorCollector>();

        // 运行时验证：通过 trait 对象引用调用 sink_error
        let collector = ErrorCollector::new();
        let sink: &dyn ErrorSink = &collector;
        sink.sink_error(ErrorEvent::new("compile-time check".to_string()));

        // 不 panic 即通过；队列应含 1 个事件
        assert_eq!(collector.sunk_pending(), 1);
    }

    #[test]
    fn test_sink_error_drains_correctly() {
        let mut collector = ErrorCollector::new();
        collector.enable();

        // sink 3 个事件
        collector.sink_error(ErrorEvent::new("err1".to_string()).with_ip(10));
        collector.sink_error(ErrorEvent::new("err2".to_string()).with_ip(20));
        collector.sink_error(ErrorEvent::new("err3".to_string()).with_ip(30));

        assert_eq!(collector.sunk_pending(), 3, "sink 后队列应有 3 个待处理事件");

        let drained = collector.drain_sunk();

        assert_eq!(drained.len(), 3, "drain 应返回 3 个事件");
        assert_eq!(collector.sunk_pending(), 0, "drain 后队列应为空");
        assert_eq!(
            collector.statistics().total_errors,
            3,
            "enabled 时 drain 应更新 stats.total_errors 为 3"
        );

        // 验证 FIFO 顺序保留
        assert_eq!(drained[0].message, "err1");
        assert_eq!(drained[1].message, "err2");
        assert_eq!(drained[2].message, "err3");
    }

    #[test]
    fn test_sink_error_when_disabled() {
        let mut collector = ErrorCollector::new();
        // 默认 disabled，不 enable

        collector.sink_error(ErrorEvent::new("err1".to_string()));
        collector.sink_error(ErrorEvent::new("err2".to_string()));
        collector.sink_error(ErrorEvent::new("err3".to_string()));

        assert_eq!(collector.sunk_pending(), 3, "sink 总是入队，不论 enabled");

        let drained = collector.drain_sunk();

        assert_eq!(drained.len(), 3, "事件仍被排空返回");
        assert_eq!(
            collector.statistics().total_errors,
            0,
            "disabled 时 drain 不应更新 stats.total_errors"
        );
    }

    #[test]
    fn test_sink_error_concurrent() {
        // 验证多线程并发 sink 无数据竞争：不 panic、不丢事件
        let collector = Arc::new(ErrorCollector::new());
        let num_threads = 4;
        let per_thread = 25;
        let total = num_threads * per_thread;

        let mut handles = Vec::new();
        for t in 0..num_threads {
            let c = Arc::clone(&collector);
            handles.push(thread::spawn(move || {
                for i in 0..per_thread {
                    c.sink_error(ErrorEvent::new(format!("t{}-{}", t, i)).with_ip(i));
                }
            }));
        }
        for h in handles {
            h.join().expect("worker thread 不应 panic");
        }

        // 无锁队列应无丢失：所有事件都入队
        assert_eq!(collector.sunk_pending(), total, "并发 sink 后队列应有 {} 个事件", total);

        // 所有 worker 已 join，Arc 引用唯一，try_unwrap 取回独占所有权调用 drain_sunk
        let mut owned = Arc::try_unwrap(collector).expect("所有 worker 已 join，Arc 应唯一");
        let drained = owned.drain_sunk();
        assert_eq!(drained.len(), total, "drain 应返回全部 {} 个事件", total);
    }
}
