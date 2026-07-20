//! # NaN 标记位布局常量定义
//!
//! 本模块定义了 [`Value`] 结构体的 **位模式编码方案**，利用 IEEE 754 双精度浮点数的
//! 静默 NaN (Quiet NaN) 载荷空间来编码非数值类型标签。
//!
//! **从 v0.5.0 起**，所有 NaN-tagging 位布局常量和纯位操作函数已下沉到
//! [`nuzo_core::tag`]，本模块通过 `pub use` 重导出以保持向后兼容。
//!
//! ## 设计原理
//!
//! IEEE 754 双精度浮点数的位布局为：`[符号(1) | 指数(11) | 尾数(52)]`
//! - 当指数部分全为 1 (0x7FF) 且尾数非零时，表示 NaN
//! - 我们利用尾数的高 15 位作为**类型标签 (Type Tag)**，剩余位存储数据
//!
//! ## 位空间分配图
//!
//! ```text
//! 64 位 Value 的完整位布局：
//! ┌──────┬─────────┬─────────────────────────────────────────────┐
//! │ 位域  │ 位范围    │ 用途                                      │
//! ├──────┼─────────┼─────────────────────────────────────────────┤
//! │ Tag  │ [63:49]  │ 类型标记（15 位）                           │
//! │ GC   │ [45]     │ GC 管理标志位（0=HEAP_POOL, 1=GC 堆）       │
//! │ Index│ [44:0]   │ 堆索引 / Smi 值 / 字符串池索引 / 指针地址    │
//! └──────┴─────────┴─────────────────────────────────────────────┘
//!
//! 类型标签 (Tag[63:49]) 分配：
//! ┌────────────────┬───────────┬────────────────────────────────┐
//! │ Tag 值         │ 类型      │ Payload 含义                    │
//! ├────────────────┼───────────┼────────────────────────────────┤
//! │ 0x7FF8_0       │ 特殊值     │ 低 2 位: 1=nil, 2=false, 3=true│
//! │ 0x7FF8_4       │ 堆对象     │ 低 46 位: HEAP_POOL/GC 堆索引   │
//! │ 0x7FF8_8       │ 字符串     │ 低 47 位: 全局字符串池索引      │
//! │ 0x7FF9_        │ Smi 整数   │ 低 48 位: 有符号小整数          │
//! │ 其他           │ Float     │ 完整 IEEE 754 f64 位模式        │
//! └────────────────┴───────────┴────────────────────────────────┘
//! ```
//!
//! ## 使用示例
//!
//! ```ignore
//! // 检测值类型（单条位与指令）
//! if (value.to_bits() & SMI_MASK) == SMI_TAG { /* 是 Smi 整数 */ }
//! if (value.to_bits() & HEAP_MASK) == HEAP_TAG { /* 是堆对象 */ }
//!
//! // 提取堆索引
//! let heap_idx = value.to_bits() & HEAP_INDEX_MASK;
//! ```

// ============================================================================
// 从 nuzo_core::tag 重导出 NaN-tagging 位布局常量
// ============================================================================

pub use nuzo_core::tag::HEAP_INDEX_MASK;
pub use nuzo_core::tag::HEAP_MASK;
pub use nuzo_core::tag::HEAP_TAG;

pub use nuzo_core::tag::SPECIAL_MASK;

pub use nuzo_core::tag::STRING_INDEX_MASK;
pub use nuzo_core::tag::STRING_MASK;
pub use nuzo_core::tag::STRING_TAG;

pub use nuzo_core::tag::SMI_MASK;
pub use nuzo_core::tag::SMI_MAX;
pub use nuzo_core::tag::SMI_MIN;
pub use nuzo_core::tag::SMI_TAG;
pub use nuzo_core::tag::SMI_VALUE_MASK;

pub use nuzo_core::tag::FALSE_VALUE;
pub use nuzo_core::tag::NIL_VALUE;
pub use nuzo_core::tag::TRUE_VALUE;

pub use nuzo_core::tag::CANONICAL_NAN;

pub use nuzo_core::tag::PTR_MASK;
pub use nuzo_core::tag::PTR_TAG;
pub use nuzo_core::tag::QNAN_MASK;

// ============================================================================
// 堆对象大小估算常量 (从 nuzo_core 重导出)
// ============================================================================

/// 数组对象的固定开销字节数（不含元素）
pub use nuzo_core::constants::ARRAY_OVERHEAD_BYTES;
/// Box 对象的大小字节数
pub use nuzo_core::constants::BOX_SIZE_BYTES;
/// 内建函数对象的固定大小字节数
pub use nuzo_core::constants::BUILTIN_FN_SIZE_BYTES;
/// 单个捕获变量的内存占用字节数
pub use nuzo_core::constants::CAPTURED_VAR_SIZE_BYTES;
/// 闭包对象的固定开销字节数（不含捕获列表）
pub use nuzo_core::constants::CLOSURE_OVERHEAD_BYTES;
/// Exception 异常对象的基础大小估算字节数（不含动态字段）
pub use nuzo_core::constants::EXCEPTION_SIZE_BYTES;
/// 范围对象的固定大小字节数
pub use nuzo_core::constants::RANGE_SIZE_BYTES;
/// StrBuilder (SliceChain) 对象的基础大小估算字节数
pub use nuzo_core::constants::STRBUILDER_SIZE_BYTES;

// ============================================================================
// GC 集成常量 (从 nuzo_core::tag 重导出)
// ============================================================================

pub use nuzo_core::tag::GC_MANAGED_BIT;
pub use nuzo_core::tag::HEAP_INDEX_MASK_NO_GC;
pub use nuzo_core::tag::HEAP_POOL_INDEX_LIMIT;
pub use nuzo_core::tag::SCRATCH_BASE;

// ============================================================================
// Arena (Region Allocator) 索引常量（nuzo_values 专属，不在 nuzo_core::tag）
// ============================================================================

/// Arena 索引区起始值（位于 GC Persistent 和 Scratch 之间）。
///
/// 编码空间: `[ARENA_BASE, SCRATCH_BASE)` = `[0x4000_0000, 0x8000_0000)` = 1GB
pub const ARENA_BASE: u32 = 0x4000_0000;

/// Arena 索引掩码（用于从完整索引中提取偏移量）。
///
/// 掩码值：`0x3FFF_FFFF`（低 30 位）
pub const ARENA_MASK: u32 = 0x3FFF_FFFF;

// ============================================================================
// 哈希常量 (从 nuzo_core::tag 重导出)
// ============================================================================

pub use nuzo_core::tag::FX_HASH_MULTIPLIER;
pub use nuzo_core::tag::GOLDEN_64;

// ============================================================================
// Smi 符号扩展常量 (从 nuzo_core::tag 重导出)
// ============================================================================

pub use nuzo_core::tag::SMI_SIGN_BIT;
pub use nuzo_core::tag::SMI_SIGN_EXTEND;
