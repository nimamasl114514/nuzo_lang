//! 核心配置结构 — 统一管理 Nuzo 运行时所有可配置项
//!
//! # 配置分组
//!
//! | 分组 | 结构体 | 说明 |
//! |------|--------|------|
//! | VM | `VmConfig` | 栈大小、调用帧上限、寄存器预分配 |
//! | GC | `GcConfig` | 阈值、增长因子、存活率、分代参数 |
//! | Compiler | `CompilerConfig` | 局部变量上限 |
//! | Arena | `ArenaConfig` | 帧级 Arena 容量、Region 上限 |
//! | FramePaging | `FramePagingConfig` | 帧换页容量、水位线、批量换出 |
//!
//! # 默认值来源
//!
//! 所有默认值与 `nuzo_core::constants` 和各模块的硬编码值保持一致。
//! 迁移后，各模块从 `Config` 读取，而非硬编码常量。

use std::path::PathBuf;

use crate::error::ConfigResult;
use crate::source::ConfigSource;

/// 从 TOML 表中提取字段并赋值给配置项。
///
/// 用法：`apply_config_field!(&mut config.field, table, "section.key", TYPE)`
///
/// 行为与原 `apply_*` 函数完全一致：
/// - key 不存在：保留默认值（静默）
/// - key 存在但类型不匹配：`log::warn!` 输出 key、实际类型、当前默认值，并保留默认值
/// - key 存在且类型匹配：覆盖默认值
macro_rules! apply_config_field {
    ($target:expr, $table:expr, $key:literal, usize) => {
        if let Some(val) = $table.get($key) {
            if let Some(v) = val.as_usize() {
                *$target = v;
            } else {
                log::warn!(
                    "config key {:?} expects usize, but value type is {:?}; using default {}",
                    $key,
                    val.type_name(),
                    $target
                );
            }
        }
    };
    ($target:expr, $table:expr, $key:literal, u32) => {
        if let Some(val) = $table.get($key) {
            if let Some(v) = val.as_u32() {
                *$target = v;
            } else {
                log::warn!(
                    "config key {:?} expects u32, but value type is {:?}; using default {}",
                    $key,
                    val.type_name(),
                    $target
                );
            }
        }
    };
    ($target:expr, $table:expr, $key:literal, u16) => {
        if let Some(val) = $table.get($key) {
            if let Some(v) = val.as_u16() {
                *$target = v;
            } else {
                log::warn!(
                    "config key {:?} expects u16, but value type is {:?}; using default {}",
                    $key,
                    val.type_name(),
                    $target
                );
            }
        }
    };
    ($target:expr, $table:expr, $key:literal, u8) => {
        if let Some(val) = $table.get($key) {
            if let Some(v) = val.as_u8() {
                *$target = v;
            } else {
                log::warn!(
                    "config key {:?} expects u8, but value type is {:?}; using default {}",
                    $key,
                    val.type_name(),
                    $target
                );
            }
        }
    };
    ($target:expr, $table:expr, $key:literal, f64) => {
        if let Some(val) = $table.get($key) {
            if let Some(v) = val.as_f64() {
                *$target = v;
            } else {
                log::warn!(
                    "config key {:?} expects f64, but value type is {:?}; using default {}",
                    $key,
                    val.type_name(),
                    $target
                );
            }
        }
    };
    ($target:expr, $table:expr, $key:literal, bool) => {
        if let Some(val) = $table.get($key) {
            if let Some(v) = val.as_bool() {
                *$target = v;
            } else {
                log::warn!(
                    "config key {:?} expects bool, but value type is {:?}; using default {}",
                    $key,
                    val.type_name(),
                    $target
                );
            }
        }
    };
}

/// VM 核心配置
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// 最大栈大小（寄存器文件大小）
    pub max_stack_size: usize,
    /// 最大调用帧数
    pub max_call_frames: usize,
    /// 初始寄存器预分配数量
    pub initial_registers: usize,
    /// 帧预分配容量
    pub initial_frame_capacity: usize,
    /// 诊断模式寄存器窗口大小
    pub diagnostic_register_window: usize,
    /// 单次 run() 执行超时（毫秒）。
    ///
    /// - `Some(ms)`: 超时后 VM 返回 `NuzoErrorKind::ExecutionTimeout`
    /// - `None`: 无超时限制（默认，向后兼容）
    ///
    /// 检查点在每个 opcode 执行后（`run_inner` 循环顶部），
    /// 对于死循环或耗时过长的脚本提供安全防护。
    pub execution_timeout_ms: Option<u64>,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            max_stack_size: 32 * 1024 * 1024,
            max_call_frames: 1_000_000,
            initial_registers: 256,
            initial_frame_capacity: 64,
            diagnostic_register_window: 8,
            execution_timeout_ms: Some(30_000),
        }
    }
}

/// GC 垃圾回收配置
#[derive(Debug, Clone)]
pub struct GcConfig {
    /// GC 最小阈值（字节）
    pub min_threshold: usize,
    /// GC 默认阈值（字节）
    pub threshold: usize,
    /// GC 存活率阈值
    pub survival_ratio_threshold: f64,
    /// GC 阈值增长倍数
    pub growth_factor: usize,
    /// 标记速率
    pub mark_rate: usize,
    /// 清扫速率
    pub sweep_rate: usize,
    /// Chunk 位移
    pub chunk_shift: u32,
    /// 新生代阈值（字节）
    pub nursery_threshold: usize,
    /// 老年代倍数
    pub tenured_multiplier: usize,
    /// 晋升存活率
    pub promote_survival_ratio: f64,
    /// 冷区晋升年龄
    pub cold_age_threshold: u8,
    /// 深度 GC 频率
    pub deep_gc_interval: u8,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            min_threshold: 1024,
            threshold: 10 * 1024 * 1024,
            survival_ratio_threshold: 0.5,
            growth_factor: 2,
            mark_rate: 8,
            sweep_rate: 16,
            chunk_shift: 10,
            nursery_threshold: 1024 * 1024,
            tenured_multiplier: 8,
            promote_survival_ratio: 0.4,
            cold_age_threshold: 3,
            deep_gc_interval: 10,
        }
    }
}

/// 编译器配置
#[derive(Debug, Clone)]
pub struct CompilerConfig {
    /// 最大局部变量/寄存器数
    pub max_locals: u16,
    /// 函数内最大局部变量数
    pub max_function_locals: u16,
}

impl Default for CompilerConfig {
    fn default() -> Self {
        Self { max_locals: 65535, max_function_locals: 4096 }
    }
}

/// Arena/Region 内存分配器配置
#[derive(Debug, Clone, Copy)]
pub struct ArenaConfig {
    /// 单帧 Arena 最大容量（字节）
    pub max_frame_arena_size: usize,
    /// Region 总容量上限（字节）
    pub max_region_size: usize,
    /// 是否启用 Arena
    pub enabled: bool,
}

impl Default for ArenaConfig {
    fn default() -> Self {
        Self { max_frame_arena_size: 64 * 1024, max_region_size: 16 * 1024 * 1024, enabled: true }
    }
}

/// 帧换页配置
#[derive(Debug, Clone, Copy)]
pub struct FramePagingConfig {
    /// 帧栈容量上限
    pub capacity: usize,
    /// 低水位线
    pub low_watermark: usize,
    /// 每次换出的帧数
    pub spill_batch: usize,
}

impl Default for FramePagingConfig {
    fn default() -> Self {
        Self { capacity: 200, low_watermark: 50, spill_batch: 100 }
    }
}

/// Nuzo 统一配置
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub vm: VmConfig,
    pub gc: GcConfig,
    pub compiler: CompilerConfig,
    pub arena: ArenaConfig,
    pub frame_paging: FramePagingConfig,
    /// 标准库搜索路径。当 `import` 路径以 `std/` 开头时，以此路径为基准解析。
    /// 设为 `None` 时，`std/` 前缀的 import 按普通相对路径处理。
    ///
    /// 默认值：`None`
    pub std_path: Option<PathBuf>,
}

impl Config {
    /// 创建配置 Builder
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::new()
    }

    /// 从 TOML 字符串构建
    pub fn from_toml_str(input: &str) -> ConfigResult<Self> {
        ConfigBuilder::new().with_toml_str(input).build()
    }

    /// 从 TOML 文件构建
    pub fn from_toml_file(path: &std::path::Path) -> ConfigResult<Self> {
        ConfigBuilder::new().with_toml_file(path).build()
    }

    /// 使用默认值 + 环境变量构建
    pub fn from_env() -> Self {
        ConfigBuilder::new().with_env().build().unwrap_or_default()
    }
}

/// 配置构建器
///
/// 支持链式调用，按优先级合并多个配置源：
/// 默认值 → TOML 文件 → 环境变量
pub struct ConfigBuilder {
    sources: Vec<ConfigSource>,
}

impl ConfigBuilder {
    /// 创建 Builder（默认值）
    pub fn new() -> Self {
        Self { sources: Vec::new() }
    }

    /// 添加 TOML 字符串配置源
    pub fn with_toml_str(mut self, input: &str) -> Self {
        if let Ok(source) = ConfigSource::from_toml_str(input) {
            self.sources.push(source);
        }
        self
    }

    /// 添加 TOML 文件配置源
    ///
    /// 文件不存在时静默跳过。
    pub fn with_toml_file(mut self, path: impl AsRef<std::path::Path>) -> Self {
        if let Ok(source) = ConfigSource::from_toml_file(path.as_ref()) {
            self.sources.push(source);
        }
        self
    }

    /// 添加环境变量配置源
    pub fn with_env(mut self) -> Self {
        self.sources.push(ConfigSource::from_env());
        self
    }

    /// 构建最终配置
    ///
    /// 合并所有源，后者覆盖前者，然后填充 Config 结构体。
    pub fn build(self) -> ConfigResult<Config> {
        let mut merged = ConfigSource::new();
        for source in &self.sources {
            merged.merge(source);
        }

        let mut config = Config::default();
        let table = merged.as_table();

        apply_config_field!(&mut config.vm.max_stack_size, table, "vm.max_stack_size", usize);
        apply_config_field!(&mut config.vm.max_call_frames, table, "vm.max_call_frames", usize);
        apply_config_field!(&mut config.vm.initial_registers, table, "vm.initial_registers", usize);
        apply_config_field!(
            &mut config.vm.initial_frame_capacity,
            table,
            "vm.initial_frame_capacity",
            usize
        );
        apply_config_field!(
            &mut config.vm.diagnostic_register_window,
            table,
            "vm.diagnostic_register_window",
            usize
        );

        apply_config_field!(&mut config.gc.min_threshold, table, "gc.min_threshold", usize);
        apply_config_field!(&mut config.gc.threshold, table, "gc.threshold", usize);
        apply_config_field!(
            &mut config.gc.survival_ratio_threshold,
            table,
            "gc.survival_ratio_threshold",
            f64
        );
        apply_config_field!(&mut config.gc.growth_factor, table, "gc.growth_factor", usize);
        apply_config_field!(&mut config.gc.mark_rate, table, "gc.mark_rate", usize);
        apply_config_field!(&mut config.gc.sweep_rate, table, "gc.sweep_rate", usize);
        apply_config_field!(&mut config.gc.chunk_shift, table, "gc.chunk_shift", u32);
        apply_config_field!(&mut config.gc.nursery_threshold, table, "gc.nursery_threshold", usize);
        apply_config_field!(
            &mut config.gc.tenured_multiplier,
            table,
            "gc.tenured_multiplier",
            usize
        );
        apply_config_field!(
            &mut config.gc.promote_survival_ratio,
            table,
            "gc.promote_survival_ratio",
            f64
        );
        apply_config_field!(&mut config.gc.cold_age_threshold, table, "gc.cold_age_threshold", u8);
        apply_config_field!(&mut config.gc.deep_gc_interval, table, "gc.deep_gc_interval", u8);

        apply_config_field!(&mut config.compiler.max_locals, table, "compiler.max_locals", u16);
        apply_config_field!(
            &mut config.compiler.max_function_locals,
            table,
            "compiler.max_function_locals",
            u16
        );

        apply_config_field!(
            &mut config.arena.max_frame_arena_size,
            table,
            "arena.max_frame_arena_size",
            usize
        );
        apply_config_field!(
            &mut config.arena.max_region_size,
            table,
            "arena.max_region_size",
            usize
        );
        apply_config_field!(&mut config.arena.enabled, table, "arena.enabled", bool);

        apply_config_field!(
            &mut config.frame_paging.capacity,
            table,
            "frame_paging.capacity",
            usize
        );
        apply_config_field!(
            &mut config.frame_paging.low_watermark,
            table,
            "frame_paging.low_watermark",
            usize
        );
        apply_config_field!(
            &mut config.frame_paging.spill_batch,
            table,
            "frame_paging.spill_batch",
            usize
        );

        Ok(config)
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = Config::default();
        assert_eq!(config.vm.max_stack_size, 32 * 1024 * 1024);
        assert_eq!(config.gc.threshold, 10 * 1024 * 1024);
        assert_eq!(config.compiler.max_function_locals, 4096);
        assert_eq!(config.arena.max_frame_arena_size, 64 * 1024);
        assert_eq!(config.frame_paging.capacity, 200);
    }

    #[test]
    fn from_toml_str() {
        let input = r#"
[vm]
max_stack_size = 131072

[gc]
threshold = 20_000_000
survival_ratio_threshold = 0.7

[arena]
enabled = false
max_frame_arena_size = 128_000

[frame_paging]
capacity = 400
"#;
        let config = Config::from_toml_str(input).unwrap();
        assert_eq!(config.vm.max_stack_size, 131072);
        assert_eq!(config.gc.threshold, 20_000_000);
        assert!((config.gc.survival_ratio_threshold - 0.7).abs() < 1e-10);
        assert!(!config.arena.enabled);
        assert_eq!(config.arena.max_frame_arena_size, 128_000);
        assert_eq!(config.frame_paging.capacity, 400);
        // 未覆盖的保持默认
        assert_eq!(config.vm.max_call_frames, 1_000_000);
        assert_eq!(config.compiler.max_locals, 65535);
    }

    #[test]
    fn builder_chain() {
        let config = Config::builder().with_toml_str("[gc]\nthreshold = 999").build().unwrap();
        assert_eq!(config.gc.threshold, 999);
        assert_eq!(config.vm.max_stack_size, 32 * 1024 * 1024); // 默认值保留
    }

    #[test]
    fn missing_file_keeps_defaults() {
        let config = Config::builder().with_toml_file("/nonexistent/nuzo.toml").build().unwrap();
        assert_eq!(config.vm.max_stack_size, 32 * 1024 * 1024);
    }

    #[test]
    fn partial_override() {
        let input = "[gc]\nmark_rate = 16\n";
        let config = Config::from_toml_str(input).unwrap();
        assert_eq!(config.gc.mark_rate, 16);
        assert_eq!(config.gc.sweep_rate, 16); // 默认值
        assert_eq!(config.gc.threshold, 10 * 1024 * 1024); // 默认值
    }

    #[test]
    fn config_clone_eq() {
        let a = Config::default();
        let b = a.clone();
        assert_eq!(a.vm.max_stack_size, b.vm.max_stack_size);
        assert_eq!(a.gc.threshold, b.gc.threshold);
    }

    #[test]
    fn type_mismatch_uses_default() {
        // threshold 期望 usize，给字符串 → 静默回退默认值
        let input = "[gc]\nthreshold = \"not_a_number\"\n";
        let config = Config::from_toml_str(input).unwrap();
        assert_eq!(config.gc.threshold, 10 * 1024 * 1024); // 默认值
    }
}
