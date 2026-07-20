//! # Nuzo Values — Nuzo 基于 NaN 标记的高性能动态类型值系统
//!
//! **层级**: L2（语言核心层）—— 提供 Nuzo 运行时的统一动态值表示、堆对象模型与错误类型，是 VM 与编译器共享的值语义层。
//!
//! **主要入口**: [`Value`], [`ValueTag`], [`HeapObject`], [`NuzoError`], [`FunctionPrototype`], [`RuntimeContext`]
//!
//! 本 crate 为 Nuzo 运行时提供**核心动态类型值表示**，采用 IEEE 754 NaN 标记
//! (NaN Tagging) 技术结合小整数优化 (Smi Optimization)，实现接近原生的动态语言语义性能。
//!
//! ## 架构设计总览
//!
//! 本系统采用 **双层标记架构 (Dual-Track Tagging)** 将所有动态类型值编码到单个 64 位字中：
//!
//! ### 第一层：NaN 标记空间分配
//! 利用 IEEE 754 浮点数的 **静默 NaN (Quiet NaN)** 载荷空间来区分非数值类型
//! （nil、布尔、指针）与数值类型。合法的 IEEE 754 双精度浮点数不会被误判。
//!
//! ### 第二层：Smi 小整数编码（"归纳公式"）
//! 小整数通过位运算直接嵌入 NaN 空间，无需 FPU 参与即可完成算术运算：
//! ```text
//! SMI(i) = SMI_TAG | (i as u64)        // 编码
//! smi_add(a, b) ≡ a + b - SMI_TAG      // 加法（单条 CPU 指令！）
//! smi_sub(a, b) ≡ a - b + SMI_TAG      // 减法（单条 CPU 指令！）
//! ```
//!
//! ## 内存布局（8 字节 Value 结构）
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ 位模式范围                    │ 类型    │ 说明              │
//! ├─────────────────────────────────────────────────────────────┤
//! │ 0x7FF8_0000_0000_000[1-3]      │ 特殊值  │ nil, false, true  │
//! │ 0x7FF8_4000_XXXX_XXXX          │ 堆对象  │ 数组, 字典, 闭包  │
//! │ 0x7FF8_8000_XXXX_XXXX          │ 字符串  │ 池化字符串引用     │
//! │ 0x7FF9_XXXX_XXXX_XXXX          │ Smi    │ 小整数 [-2^47,2^47)│
//! │ 所有其他模式                   │ Float  │ 标准 IEEE 754 f64   │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 动态类型分派机制 (Tagged Union / Enum Dispatch)
//!
//! 值的类型检测通过 **位掩码测试** 实现，时间复杂度 O(1)，无需查表：
//! - [`Value::is_smi()`] -- 检测 `SMI_MASK` 位模式
//! - [`Value::is_number()`] -- 排除特殊标记后判定为浮点数
//! - [`Value::is_heap_object()`] -- 检测 `HEAP_MASK` 位模式
//! - [`Value::is_string()`] -- 检测 `STRING_MASK` 位模式
//!
//! ## 堆分配策略与 GC Roots 管理
//!
//! 堆对象（数组、字典、闭包等）存储在全局堆池中，通过 **索引间接引用**：
//! - **默认路径**：[`HEAP_POOL`] 全局静态池，使用 `Arc<HeapObject>` 引用计数
//! - **GC 路径**：通过可插拔的堆访问器函数 (`HEAP_ALLOC_FN`, `HEAP_GET_FN`, etc.)
//!   与 VM 的垃圾回收器集成，支持 GC-managed 标记位 (`GC_MANAGED_BIT`)
//! - **GC Roots 扫描**：通过 [`HeapRootsFn`] 回调函数收集所有根对象
//!
//! ## 模块结构
//!
//! | 模块 | 职责 | 核心类型 |
//! |------|------|----------|
//! | [`constants`] | NaN 标记位布局常量定义 | `HEAP_TAG`, `SMI_TAG`, `STRING_TAG` |
//! | [`value`] | 值类型系统核心实现 | [`Value`](value::Value), [`ValueTag`](value::ValueTag) |
//! | [`errors`] | 统一错误层次体系 | [`NuzoError`](errors::NuzoError), [`InternalError`](errors::InternalError) |
//! | [`heap`] | 堆对象类型与闭包捕获 | [`HeapObject`](heap::HeapObject), [`CaptureMode`](heap::CaptureMode) |
//! | [`function`] | 函数原型与调试信息 | [`FunctionPrototype`](function::FunctionPrototype), [`DebugInfo`](function::DebugInfo) |
//! | [`context`] | 运行时上下文封装 | [`RuntimeContext`](context::RuntimeContext) |
//! | [`nuzo_dict`] | 字典数据结构 | [`NuzoDict`](nuzo_dict::NuzoDict), [`SmallDict`](nuzo_dict::SmallDict), [`LargeDict`](nuzo_dict::LargeDict) |
//! | [`generic`] | 高级泛型基础设施 | HList, Functor/Monad, AnyMap, GenericArray |
//!
//! ## 类型转换规则
//!
//! - **数值转换**：[`Value::from_number()`] 自动选择 Smi 或 Float 编码
//! - **隐式转换**：算术运算时 Smi 与 Float 自动提升；字符串连接时自动调用 `add`
//! - **显式转换**：提供 `as_smi()`, `as_number()`, `as_bool()`, `as_string_opt()` 等方法
//! - **深比较**：[`Value::value_equals()`] 支持数值强制类型转换后的相等性判断

#![allow(clippy::result_large_err)]
#![allow(clippy::should_implement_trait)]

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(
    layer = 2,
    description = "NaN-tagged 值系统与 TurboSlab 分配器",
    entry_type = "Value"
)]
const _NUZO_CRATE_META_ANCHOR: () = ();

pub mod constants;
pub mod context;
pub mod errors;
pub mod function;
pub mod generic;
pub mod heap;
pub mod inspector;
pub mod layout;
pub mod nuzo_dict;
pub mod prelude;
pub mod tag_registry;
pub mod traits;
pub mod turboslab;
pub mod value;

// Re-exports for convenience
pub use constants::ARENA_BASE;
pub use constants::ARENA_MASK;
pub use constants::GC_MANAGED_BIT;
pub use constants::HEAP_INDEX_MASK_NO_GC;
pub use constants::HEAP_TAG;
pub use constants::SCRATCH_BASE;
pub use context::RuntimeContext;
pub use errors::{InternalError, NuzoError, NuzoErrorKind, SourceLocation, VmDiagnosis};
pub use function::DeadCodeReason;
pub use function::DeadCodeRecord;
pub use function::DebugInfo;
pub use function::FoldRecord;
pub use function::FunctionPrototype;
pub use function::InlineRecord;
pub use function::PrototypeDebugInfo;
pub use generic::{
    AnyMap, Applicative, Functor, GenericArray, HCons, HList, HListPrepend, HNil, Monad,
};
pub use heap::CaptureInfo;
pub use heap::CaptureMode;
pub use heap::CapturedVar;
pub use heap::HeapObject;
pub use heap::RangeEnd;
pub use nuzo_dict::NuzoDict;
pub use traits::NuzoType;
pub use value::RangeValue;
pub use value::Value;
pub use value::ValueExt;
pub use value::ValueTag;
pub use value::{FALSE, NIL, TRUE};
pub use value::{HeapAllocFn, HeapGetFn, HeapGetMutFn, HeapRootsFn};
pub use value::{get_heap_roots_fn, register_heap_accessors, reset_heap_accessors};
pub use value::{register_gc_heap_alloc, register_value_hooks, unregister_gc_heap_alloc};
