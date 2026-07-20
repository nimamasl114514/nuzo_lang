//! # Nuzo Core — Nuzo 核心基础库
//!
//! **层级**: L1（基础基础设施层）—— 为整个 Nuzo 语言栈提供编码、哈希、源码位置、常量与统一错误类型等无状态基础能力。
//!
//! **主要入口**: [`Value`], [`SourceLocation`], [`XxHashMap`], [`Encoding`], [`NuzoError`]
//!
//! ## 模块职责
//!
//! | 模块 | 职责 | 核心类型 |
//! |------|------|----------|
//! | [`constants`] | 全局常量与系统限制（BOM/版本/阈值） | `GC_MIN_THRESHOLD`, `INITIAL_FRAME_CAPACITY` 等 |
//! | [`encoding`] | 多编码支持与检测（UTF-8/GBK/Shift-JIS/Big5） | [`Encoding`](encoding::Encoding), `char_at`, `char_len` |
//! | [`hash`] | xxHash3 高性能哈希容器 | [`XxHashMap`](hash::XxHashMap), [`XxHashSet`](hash::XxHashSet) |
//! | [`source_location`] | 源码位置追踪（行:列 + 源行文本） | [`SourceLocation`](source_location::SourceLocation) |
//!
//! ## 设计约束
//!
//! - **核心零外部依赖**（serde/xxhash-rust 为可选或必要优化依赖）
//! - 所有系统限制为**编译期常量**（允许 LLVM 优化）
//! - 编码检测基于字节特征（无第三方依赖）
//! - 哈希容器使用 xxHash3 替代 SipHash（5-10x 性能提升）
//!
//! ## 开发者速查：常见任务 → 代码位置
//!
//! | 任务 | 位置 |
//! |------|------|
//! | "改 GC 阈值默认值" | `constants.rs: GC_* 常量` |
//! | "加新编码支持" | `encoding.rs: Encoding 枚举 + detect()` |
//! | "用高性能 HashMap" | `hash.rs: XxHashMap / xx_hash_map()` |
//! | "改源位置格式" | `source_location.rs: SourceLocation Display impl` |

#![allow(clippy::result_large_err)]
#![allow(clippy::should_implement_trait)]

// Crate 元数据——使用外层属性形式，因为 `#![inner_attr]` 在 stable Rust 不稳定
#[nuzo_proc::crate_meta(layer = 1, description = "核心值类型与错误码", entry_type = "NuzoValue")]
const _NUZO_CRATE_META_ANCHOR: () = ();

pub mod constants;
pub mod encoding;
pub mod error;
pub mod hash;
pub mod prelude;
pub mod source_location;
pub mod tag;
pub mod value;

// ── constants 模块导出（逐条显式，禁止 glob re-export）────────
// VM 核心资源常量
pub use constants::DEFAULT_MAX_CALL_FRAMES;
pub use constants::DEFAULT_MAX_STACK_SIZE;
pub use constants::DIAGNOSTIC_REGISTER_WINDOW;
pub use constants::INITIAL_FRAME_CAPACITY;
pub use constants::INITIAL_REGISTERS;

// GC 垃圾回收参数
pub use constants::GC_DEFAULT_THRESHOLD;
pub use constants::GC_MIN_THRESHOLD;
pub use constants::GC_SURVIVAL_RATIO_THRESHOLD;
pub use constants::GC_THRESHOLD_GROWTH_FACTOR;

// 编译器限制常量
pub use constants::DEFAULT_FUNCTION_SOURCE_FILE;
pub use constants::DEFAULT_SOURCE_FILE;
pub use constants::MAX_FUNCTION_LOCALS;
pub use constants::MAX_LOCALS;

// 指令/捕获常量
pub use constants::CAPTURE_OUTER_FLAG;
pub use constants::CAPTURE_OUTER_INDEX_MASK;

// 堆对象大小估算常量
pub use constants::ARRAY_OVERHEAD_BYTES;
pub use constants::BOX_SIZE_BYTES;
pub use constants::BUILTIN_FN_SIZE_BYTES;
pub use constants::CAPTURED_VAR_SIZE_BYTES;
pub use constants::CLOSURE_OVERHEAD_BYTES;
pub use constants::RANGE_SIZE_BYTES;

// 运行时类型码系统
pub use constants::TYPE_CODE_BOOL;
pub use constants::TYPE_CODE_NIL;
pub use constants::TYPE_CODE_NUMBER;
pub use constants::TYPE_CODE_OBJECT;
pub use constants::TYPE_CODE_UNKNOWN;

// 版本与标题常量
pub use constants::APP_VERSION;
pub use constants::REPL_TITLE;
pub use constants::RUNNER_TITLE;

// 编码检测用 BOM 字节序标记
pub use constants::UTF8_BOM_0;
pub use constants::UTF8_BOM_1;
pub use constants::UTF8_BOM_2;
pub use constants::UTF16_BE_BOM_0;
pub use constants::UTF16_BE_BOM_1;
pub use constants::UTF16_LE_BOM_0;
pub use constants::UTF16_LE_BOM_1;

// --- encoding: 编码工具 ---
pub use encoding::{Encoding, decode_from_bytes};
pub use encoding::{char_at, char_len};

// --- source_location: 源码位置 ---
pub use source_location::SourceLocation;

// --- hash: xxHash3 高性能哈希容器 ---
pub use hash::{XxHashMap, XxHashSet};
pub use hash::{xx_hash_map, xx_hash_map_new, xx_hash_set, xx_hash_set_new, xxh3_64};

// --- error: 统一错误类型 ---
pub use error::{InternalError, LangMode, NuzoError, NuzoErrorKind, VmDiagnosis};

// --- value: NaN-tagged 动态值类型 ---
pub use value::{FALSE, NIL, RangeValue, TRUE, Value, ValueTag};
pub use value::{ValueDisplayHook, ValueSerializeHook, set_display_hook, set_serialize_hook};
