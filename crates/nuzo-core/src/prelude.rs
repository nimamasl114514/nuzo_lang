//! # Nuzo Core Prelude
//!
//! 重新导出 `nuzo_core` 中最常用的类型、常量和函数。
//! 使用者只需 `use nuzo_core::prelude::*;` 即可获得核心 API。
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use nuzo_core::prelude::*;
//!
//! let v = Value::from_smi(42);
//! let err = NuzoError::new(NuzoErrorKind::TypeError, SourceLocation::new(0, 0, ""));
//! let map = XxHashMap::default();
//! ```

// ── Value 类型系统 ────────────────────────────────────────────────

pub use crate::value::FALSE;
pub use crate::value::NIL;
pub use crate::value::RangeValue;
pub use crate::value::TRUE;
pub use crate::value::Value;
pub use crate::value::ValueTag;

// ── 统一错误类型 ──────────────────────────────────────────────────

pub use crate::error::InternalError;
pub use crate::error::NuzoError;
pub use crate::error::NuzoErrorKind;
pub use crate::error::VmDiagnosis;

// ── 源码位置 ──────────────────────────────────────────────────────

pub use crate::source_location::SourceLocation;

// ── 编码工具 ──────────────────────────────────────────────────────

pub use crate::encoding::Encoding;
pub use crate::encoding::char_at;
pub use crate::encoding::char_len;
pub use crate::encoding::decode_from_bytes;

// ── xxHash3 高性能哈希容器 ────────────────────────────────────────

pub use crate::hash::XxHashMap;
pub use crate::hash::XxHashSet;
pub use crate::hash::xx_hash_map;
pub use crate::hash::xx_hash_map_new;
pub use crate::hash::xx_hash_set;
pub use crate::hash::xx_hash_set_new;
pub use crate::hash::xxh3_64;

// ── 常用常量 ──────────────────────────────────────────────────────

// VM 核心资源常量
pub use crate::constants::DEFAULT_MAX_CALL_FRAMES;
pub use crate::constants::DEFAULT_MAX_STACK_SIZE;
pub use crate::constants::DIAGNOSTIC_REGISTER_WINDOW;
pub use crate::constants::INITIAL_FRAME_CAPACITY;
pub use crate::constants::INITIAL_REGISTERS;

// GC 垃圾回收参数
pub use crate::constants::GC_DEFAULT_THRESHOLD;
pub use crate::constants::GC_MIN_THRESHOLD;
pub use crate::constants::GC_SURVIVAL_RATIO_THRESHOLD;
pub use crate::constants::GC_THRESHOLD_GROWTH_FACTOR;

// 编译器限制常量
pub use crate::constants::DEFAULT_FUNCTION_SOURCE_FILE;
pub use crate::constants::DEFAULT_SOURCE_FILE;
pub use crate::constants::MAX_FUNCTION_LOCALS;
pub use crate::constants::MAX_LOCALS;

// 指令/捕获常量
pub use crate::constants::CAPTURE_OUTER_FLAG;
pub use crate::constants::CAPTURE_OUTER_INDEX_MASK;

// 运行时类型码系统
pub use crate::constants::TYPE_CODE_BOOL;
pub use crate::constants::TYPE_CODE_NIL;
pub use crate::constants::TYPE_CODE_NUMBER;
pub use crate::constants::TYPE_CODE_OBJECT;
pub use crate::constants::TYPE_CODE_UNKNOWN;
