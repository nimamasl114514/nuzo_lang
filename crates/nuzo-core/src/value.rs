//! # Value 类型系统 — 基于 NaN 标记的动态值表示
//!
//! 本模块定义 [`Value`] 结构体及其纯位操作方法。
//! 从 v0.5.0 起，Value 定义从 `nuzo_values` 下沉到 `nuzo_core` (L1)，
//! 使 `nuzo_vm` (L5) 可以直接使用而无需依赖 `nuzo_values` (L2)。
//!
//! 依赖 HeapObject 或 RuntimeContext 的方法通过
//! `nuzo_values::ValueExt` 扩展 trait 提供。

use serde::Serializer;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

use crate::error::{InternalError, NuzoError};
use crate::tag::*;

// ============================================================================
// Display / Serialize 钩子（供 nuzo_values 注入完整实现）
// ============================================================================

/// Display 格式化钩子函数类型。
///
/// 返回 `Some(string)` 表示钩子已处理格式化，使用该字符串；
/// 返回 `None` 表示钩子未处理，回退到基本实现。
pub type ValueDisplayHook = fn(&Value) -> Option<String>;

/// Serialize 序列化钩子函数类型。
///
/// 返回 `Some(json_string)` 表示钩子已处理序列化，使用该 JSON 字符串；
/// 返回 `None` 表示钩子未处理，回退到基本实现。
pub type ValueSerializeHook = fn(&Value) -> Option<String>;

static DISPLAY_HOOK: OnceLock<ValueDisplayHook> = OnceLock::new();
static SERIALIZE_HOOK: OnceLock<ValueSerializeHook> = OnceLock::new();

/// 设置 Display 格式化钩子（由 `nuzo_values` 在初始化时调用）。
pub fn set_display_hook(hook: ValueDisplayHook) {
    let _ = DISPLAY_HOOK.set(hook);
}

/// 设置 Serialize 序列化钩子（由 `nuzo_values` 在初始化时调用）。
pub fn set_serialize_hook(hook: ValueSerializeHook) {
    let _ = SERIALIZE_HOOK.set(hook);
}

// ============================================================================
// ValueTag -- 统一类型分类枚举
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, nuzo_proc::MatchSync)]
pub enum ValueTag {
    Nil,
    Bool,
    Smi,
    Float,
    String,
    Pointer,
    Unknown,
}

impl fmt::Display for ValueTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueTag::Nil => write!(f, "Nil"),
            ValueTag::Bool => write!(f, "Bool"),
            ValueTag::Smi => write!(f, "Smi"),
            ValueTag::Float => write!(f, "Float"),
            ValueTag::String => write!(f, "String"),
            ValueTag::Pointer => write!(f, "Pointer"),
            ValueTag::Unknown => write!(f, "Unknown"),
        }
    }
}

// ============================================================================
// RangeValue -- Range 解构结果
// ============================================================================

/// Range 值的解构结果。
///
/// 从 v0.5.0 起，使用 `inclusive: bool` 替代 `bound: RangeEnd`，
/// 因为 `RangeEnd` 定义在 `nuzo_values::heap` (L2) 中，而本模块位于 L1。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeValue {
    pub start: f64,
    pub end: f64,
    pub inclusive: bool,
}

// ============================================================================
// Value -- 基于 NaN 标记的 8 字节动态值
// ============================================================================

/// NaN-tagged value representation — the core value type of Nuzo Lang.
///
/// The inner `u64` is **private** (visible only within `nuzo_core`) to prevent
/// construction of invalid bit patterns from downstream crates. Use type-safe
/// constructors (`from_number`, `from_bool`, `from_string`, etc.) or the
/// bit-access methods [`Value::from_bits`] / [`Value::to_bits`] for
/// serialization / FFI. Prefer the safe [`Value::try_from_raw_bits`] alternative
/// when the bit pattern may be untrusted.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Value(pub(crate) u64);

// ============================================================================
// 单例常量 (Singleton Constants)
// ============================================================================

pub const NIL: Value = Value(NIL_VALUE);
pub const FALSE: Value = Value(FALSE_VALUE);
pub const TRUE: Value = Value(TRUE_VALUE);

// ============================================================================
// Trait 实现
// ============================================================================

impl Default for Value {
    #[inline(always)]
    fn default() -> Self {
        NIL
    }
}

/// Hash implementation for Value — uses raw bits for deterministic hashing.
///
/// # Design rationale
///
/// Value is `#[repr(transparent)]` wrapping a u64 with NaN-tagging encoding.
/// Each distinct value has a unique bit pattern (strings are interned, so
/// identical strings share the same index and thus the same bits).
/// Hashing the raw u64 is therefore correct, fast, and collision-free
/// for all value types that can appear in the constant pool.
///
/// # Note on +0.0 vs -0.0
///
/// `+0.0` and `-0.0` have different bit patterns but compare equal via `==`.
/// For the constant pool deduplication use case, treating them as different
/// is acceptable — they ARE different bit patterns and the VM treats them
/// identically at runtime anyway.
impl Hash for Value {
    #[inline(always)]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_nil() {
            write!(f, "nil")
        } else if self.is_bool() {
            write!(f, "{}", self.as_bool())
        } else if self.is_smi() {
            write!(f, "Smi({})", self.as_smi())
        } else if self.is_float() {
            let n = f64::from_bits(self.0);
            if n.fract() == 0.0 && n.abs() < 1e15 {
                write!(f, "Float({:.1})", n)
            } else {
                write!(f, "Float({:e})", n)
            }
        } else if self.is_string() {
            write!(f, "String(<..>)")
        } else if self.is_heap_object() {
            write!(f, "HeapObject(<..>)")
        } else {
            write!(f, "Value(0x{:016X})", self.0)
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 优先使用钩子（由 nuzo_values 注入）
        if let Some(hook) = DISPLAY_HOOK.get()
            && let Some(s) = hook(self)
        {
            return write!(f, "{}", s);
        }
        // 基本回退实现
        if self.is_nil() {
            write!(f, "nil")
        } else if self.is_bool() {
            write!(f, "{}", self.as_bool())
        } else if self.is_smi() {
            write!(f, "{}", self.as_smi())
        } else if self.is_float() {
            let n = f64::from_bits(self.0);
            if n.fract() == 0.0 && n.abs() < 1e15 {
                write!(f, "{}", n as i64)
            } else {
                write!(f, "{}", n)
            }
        } else if self.is_string() {
            write!(f, "<string>")
        } else if self.is_heap_object() {
            write!(f, "<heap>")
        } else if self.is_ptr() {
            write!(f, "<ptr:{:#x}>", self.as_ptr() as usize)
        } else {
            write!(f, "<unknown:{:#x}>", self.0)
        }
    }
}

impl serde::Serialize for Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 优先使用钩子（由 nuzo_values 注入）
        if let Some(hook) = SERIALIZE_HOOK.get()
            && let Some(json_str) = hook(self)
        {
            return serializer.serialize_str(&json_str);
        }
        // 基本回退实现
        if self.is_nil() {
            serializer.serialize_none()
        } else if self.is_bool() {
            serializer.serialize_bool(self.as_bool())
        } else if self.is_smi() {
            serializer.serialize_i64(self.as_smi())
        } else if self.is_float() {
            serializer.serialize_f64(self.as_number())
        } else {
            serializer.serialize_none()
        }
    }
}

// ============================================================================
// Value 实现 -- 构造器与位操作
// ============================================================================

impl Value {
    /// Construct a `Value` from raw bits (serialization / FFI / low-level interop).
    ///
    /// # Safety
    /// The caller must ensure the bit pattern is a valid NaN-tagged encoding.
    /// Passing an invalid bit pattern may cause undefined behavior when the value
    /// is later interpreted (e.g., wrong tag → wrong accessor → UB).
    /// Prefer type-safe constructors (`from_number`, `from_bool`, etc.) or the
    /// safe alternative [`Value::try_from_raw_bits`] whenever possible.
    #[inline(always)]
    pub const unsafe fn from_raw_bits(bits: u64) -> Self {
        Value(bits)
    }

    /// Construct a `Value` from its raw bit representation.
    ///
    /// This is the safe, greppable replacement for the historical
    /// `Value(bits)` direct constructor. The caller is responsible for ensuring
    /// `bits` is a valid NaN-tagged encoding; methods called on the resulting
    /// `Value` will follow the encoding discipline of whatever tag the bits
    /// happen to carry (no UB, but logic errors are possible if the bits are
    /// nonsense). For untrusted input, prefer [`Value::try_from_raw_bits`].
    #[inline(always)]
    pub const fn from_bits(bits: u64) -> Self {
        Value(bits)
    }

    /// Try to construct a Value from raw bits safely.
    /// Returns `None` if the bit pattern is not a valid NaN-tagged value.
    pub fn try_from_raw_bits(bits: u64) -> Option<Self> {
        let value = Value(bits);
        if value.tag() != ValueTag::Unknown { Some(value) } else { None }
    }

    /// Extract the raw bit representation (serialization / FFI / cache fingerprinting).
    #[inline(always)]
    pub const fn into_raw_bits(self) -> u64 {
        self.0
    }

    /// Extract the raw bit representation (alias of [`Value::into_raw_bits`]).
    ///
    /// Provided as the symmetric counterpart of [`Value::from_bits`] so callers
    /// can read and write bits through a single named API pair.
    #[inline(always)]
    pub const fn to_bits(self) -> u64 {
        self.0
    }
}

// ============================================================================
// Value 实现 -- 类型检测方法 (Type Detection Methods)
// ============================================================================

impl Value {
    #[inline(always)]
    pub fn is_smi(self) -> bool {
        (self.0 & SMI_MASK) == SMI_TAG
    }

    #[inline(always)]
    pub fn is_float(self) -> bool {
        !self.is_smi() && self.is_number()
    }

    #[inline(always)]
    pub fn is_number(self) -> bool {
        if self.is_smi() {
            return true;
        }
        let bits = self.0;
        (bits & SPECIAL_MASK) != SPECIAL_MASK || bits == CANONICAL_NAN
    }

    #[inline(always)]
    pub fn is_bool(self) -> bool {
        self == TRUE || self == FALSE
    }
    #[inline(always)]
    pub fn is_nil(self) -> bool {
        self == NIL
    }

    pub fn is_truthy(self) -> bool {
        !(self.is_nil()
            || (self.is_bool() && !self.as_bool())
            || (self.is_number() && self.as_number() == 0.0))
    }

    /// 驻留恒等快径：字符串已全局驻留，位模式相同即内容相同。零提取、零分配、零 memcmp。
    pub fn value_equals(self, other: &Value) -> bool {
        if self.0 == other.0 {
            return true;
        }
        if self.is_number() && other.is_number() {
            self.as_number() == other.as_number()
        } else {
            false
        }
    }

    #[inline(always)]
    pub fn is_special(self) -> bool {
        !self.is_number()
    }

    #[inline(always)]
    pub fn is_ptr(self) -> bool {
        (self.0 & QNAN_MASK) == PTR_TAG
            && !self.is_bool()
            && !self.is_nil()
            && !self.is_smi()
            && !self.is_string()
            && !self.is_heap_object()
    }

    #[inline(always)]
    pub fn is_heap_object(self) -> bool {
        (self.0 & HEAP_MASK) == HEAP_TAG
    }

    #[inline(always)]
    pub fn is_string(self) -> bool {
        (self.0 & STRING_MASK) == STRING_TAG
    }
}

// ============================================================================
// Value 实现 -- Smi 编解码
// ============================================================================

impl Value {
    #[inline(always)]
    pub fn from_smi(i: i64) -> Value {
        assert!(
            (SMI_MIN..=SMI_MAX).contains(&i),
            "Smi overflow: {} outside range [{}, {}]",
            i,
            SMI_MAX,
            SMI_MIN
        );
        Value(SMI_TAG | (i as u64 & SMI_VALUE_MASK))
    }

    #[inline(always)]
    pub fn try_from_smi(i: i64) -> Option<Value> {
        if (SMI_MIN..=SMI_MAX).contains(&i) {
            Some(Value(SMI_TAG | (i as u64 & SMI_VALUE_MASK)))
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn as_smi(self) -> i64 {
        debug_assert!(self.is_smi(), "called as_smi() on non-smi value: {:?}", self);
        let bits = self.0 & SMI_VALUE_MASK;
        if bits & SMI_SIGN_BIT != 0 { (bits as i64) - SMI_SIGN_EXTEND } else { bits as i64 }
    }
}

// ============================================================================
// Value 实现 -- 数值转换
// ============================================================================

impl Value {
    /// 位运算整数检测：零 FPU 调用，~1ns vs fract() ~15ns
    /// IEEE 754 原理：f64 尾数的低 (52 - exponent) 位必须全为零才是精确整数
    #[inline(always)]
    fn is_integer_f64(n: f64) -> bool {
        let bits = n.to_bits();
        let exp = ((bits >> 52) & 0x7FF) as i32;
        if exp == 0x7FF {
            return false;
        }
        let biased = exp - 1023;
        if biased < 0 {
            return bits & 0x7FFF_FFFF_FFFF_FFFF == 0;
        }
        if biased >= 52 {
            return true;
        }
        let mask = (1u64 << (52 - biased)) - 1;
        (bits & mask) == 0
    }

    #[inline(always)]
    pub fn from_number(n: f64) -> Value {
        if n.is_nan() {
            return Value(CANONICAL_NAN);
        }
        if n == 0.0 && n.is_sign_negative() {
            return Value(f64::to_bits(-0.0f64));
        }
        if Self::is_integer_f64(n) {
            let i = n as i64;
            if let Some(smi) = Self::try_from_smi(i) {
                return smi;
            }
        }
        Value(n.to_bits())
    }

    #[inline(always)]
    pub fn as_number(self) -> f64 {
        if self.is_smi() {
            self.as_smi() as f64
        } else {
            debug_assert!(self.is_number(), "called as_number() on non-number");
            f64::from_bits(self.0)
        }
    }

    #[inline(always)]
    pub fn try_number(self) -> Option<f64> {
        if self.is_number() { Some(self.as_number()) } else { None }
    }
}

// ============================================================================
// Value 实现 -- 布尔 & 指针
// ============================================================================

impl Value {
    #[inline(always)]
    pub fn from_bool(b: bool) -> Value {
        if b { TRUE } else { FALSE }
    }
    #[inline(always)]
    pub fn as_bool(self) -> bool {
        debug_assert!(self.is_bool());
        self == TRUE
    }

    /// # Safety
    ///
    /// The pointer must be a valid, non-null address that fits within 46 bits
    /// (the width of [`PTR_MASK`]). The caller must ensure the pointer remains
    /// valid for the lifetime of the returned `Value`.
    ///
    /// # Panics
    ///
    /// Panics in both debug and release builds if the address exceeds 46 bits,
    /// matching the strategy of [`Value::from_gc_index`] and
    /// [`Value::from_heap_object_gc`] (release-build panic is preferable to
    /// silent high-bit truncation, which would produce a dangling pointer tag).
    #[inline(always)]
    pub unsafe fn from_ptr(ptr: *const u8) -> Value {
        let addr = ptr as u64;
        assert!(
            addr & !PTR_MASK == 0,
            "pointer address exceeds 46 bits: {:#x} (mask = {:#x})",
            addr,
            PTR_MASK,
        );
        Value(PTR_TAG | addr)
    }

    #[inline(always)]
    pub fn as_ptr(self) -> *const u8 {
        debug_assert!(self.is_ptr(), "called as_ptr() on non-ptr");
        (self.0 & PTR_MASK) as *const u8
    }
}

// ============================================================================
// Value 实现 -- 索引空间 (Index Space)
// ============================================================================

impl Value {
    #[inline(always)]
    pub fn heap_index(self) -> Option<u32> {
        if self.is_heap_object() { Some((self.0 & HEAP_INDEX_MASK_NO_GC) as u32) } else { None }
    }

    /// 返回堆索引，非堆对象时返回 `Err`。
    #[inline(always)]
    pub fn heap_idx_or_err(self) -> Result<u32, NuzoError> {
        self.heap_index().ok_or_else(|| {
            NuzoError::internal(
                InternalError::CompilerBug {
                    message: "heap_index required: value is not a heap object".to_string(),
                },
                None,
            )
        })
    }

    #[inline(always)]
    pub fn is_gc_managed(self) -> bool {
        self.is_heap_object() && (self.0 & GC_MANAGED_BIT) != 0
    }

    /// 判断此 Value 的堆索引是否属于划痕区（Scratch Arena）。
    #[inline(always)]
    pub fn is_scratch_index(idx: u32) -> bool {
        idx >= SCRATCH_BASE
    }

    /// 从已有的 GC 持久区索引构造 Value（零分配，纯位打包）。
    #[inline(always)]
    pub fn from_gc_index(idx: u32) -> Value {
        assert!(idx as u64 <= HEAP_INDEX_MASK_NO_GC, "heap index exceeds 45-bit limit: {}", idx);
        Value(HEAP_TAG | GC_MANAGED_BIT | (idx as u64 & HEAP_INDEX_MASK_NO_GC))
    }

    /// 从划痕区索引构造 Value（ERSA 专用）。
    #[inline(always)]
    pub fn from_scratch_index(idx: u32) -> Value {
        assert!(
            Self::is_scratch_index(idx),
            "scratch index must have high bit set (>= SCRATCH_BASE = {:#x}), got {:#x} ({})",
            SCRATCH_BASE,
            idx,
            idx
        );
        Value(HEAP_TAG | GC_MANAGED_BIT | (idx as u64 & HEAP_INDEX_MASK_NO_GC))
    }

    /// 从 Arena 索引构造 Value（Region Allocator 专用）。
    #[inline(always)]
    pub fn from_arena_index(offset: u32) -> Value {
        let idx = ARENA_BASE | (offset & ARENA_MASK);
        assert!(
            Self::is_arena_index(idx),
            "arena index must be in [{:#x}, {:#x}), got {:#x} ({})",
            ARENA_BASE,
            SCRATCH_BASE,
            idx,
            idx
        );
        Value(HEAP_TAG | GC_MANAGED_BIT | (idx as u64 & HEAP_INDEX_MASK_NO_GC))
    }

    /// 判断给定索引是否属于 Arena 区。
    #[inline(always)]
    pub fn is_arena_index(idx: u32) -> bool {
        (ARENA_BASE..SCRATCH_BASE).contains(&idx)
    }

    /// 若此 Value 持有 Arena 区索引，返回纯偏移量（去掉 ARENA_BASE）；否则返回 None。
    #[inline(always)]
    pub fn try_arena_offset(&self) -> Option<u32> {
        if !self.is_heap_object() || !self.is_gc_managed() {
            return None;
        }
        let idx = (self.0 & HEAP_INDEX_MASK_NO_GC) as u32;
        if Self::is_arena_index(idx) { Some(idx & ARENA_MASK) } else { None }
    }

    /// ERSA remap：若此 Value 持有划痕区索引且在 remap 表中找到映射，
    /// 则原地替换为持久区索引。返回是否发生了重映射。
    #[inline(always)]
    pub fn try_remap(&mut self, remap: &[(u32, u32)]) -> bool {
        if !self.is_heap_object() || !self.is_gc_managed() {
            return false;
        }
        let Some(old_idx) = self.heap_index() else {
            return false;
        };
        if old_idx < SCRATCH_BASE {
            return false;
        }
        if let Ok(pos) = remap.binary_search_by_key(&old_idx, |(o, _)| *o) {
            let new_idx = remap[pos].1;
            debug_assert!(
                new_idx < SCRATCH_BASE,
                "remap target must be persistent index (not scratch)"
            );
            *self = Self::from_gc_index(new_idx);
            true
        } else {
            false
        }
    }
}

// ============================================================================
// Value 实现 -- 字符串索引（纯位操作，不访问字符串池）
// ============================================================================

impl Value {
    #[inline(always)]
    pub fn string_index(self) -> Option<u32> {
        if self.is_string() { Some((self.0 & STRING_INDEX_MASK) as u32) } else { None }
    }

    #[inline(always)]
    pub fn from_string_index(idx: u32) -> Value {
        Value(STRING_TAG | (idx as u64 & STRING_INDEX_MASK))
    }
}

// ============================================================================
// Value 实现 -- 反射
// ============================================================================

impl Value {
    #[inline(always)]
    pub fn tag(self) -> ValueTag {
        if self.is_nil() {
            ValueTag::Nil
        } else if self.is_bool() {
            ValueTag::Bool
        } else if self.is_smi() {
            ValueTag::Smi
        } else if self.is_float() {
            ValueTag::Float
        } else if self.is_string() {
            ValueTag::String
        } else if self.is_heap_object() || self.is_ptr() {
            ValueTag::Pointer
        } else {
            ValueTag::Unknown
        }
    }

    pub fn to_string_repr(self) -> String {
        format!("{}", self)
    }
}

// ============================================================================
// Value 实现 -- 算术运算 (冷热分离)
// ============================================================================

impl Value {
    // --- add ---

    #[cold]
    #[inline(never)]
    fn add_float_fallback(self, other: Value) -> Result<Value, NuzoError> {
        // NOTE: 字符串拼接路径已移至 nuzo_values::ValueExt trait，
        // 此处仅处理纯数值加法。nuzo_core 不依赖字符串池。
        if !self.is_number() || !other.is_number() {
            let bad = if !self.is_number() { self.tag() } else { other.tag() };
            return Err(NuzoError::expected_number(format!("{}", bad)));
        }
        Ok(Value::from_number(self.as_number() + other.as_number()))
    }

    pub fn add(self, other: Value) -> Result<Value, NuzoError> {
        if self.is_smi() && other.is_smi() {
            let result = self.0.wrapping_add(other.0).wrapping_sub(SMI_TAG);
            if (result & SMI_MASK) == SMI_TAG {
                return Ok(Value(result));
            }
        }
        self.add_float_fallback(other)
    }

    // --- sub ---

    #[cold]
    #[inline(never)]
    fn sub_float_fallback(self, other: Value) -> Result<Value, NuzoError> {
        if !self.is_number() || !other.is_number() {
            let bad = if !self.is_number() { self.tag() } else { other.tag() };
            return Err(NuzoError::expected_number(format!("{}", bad)));
        }
        Ok(Value::from_number(self.as_number() - other.as_number()))
    }

    pub fn sub(self, other: Value) -> Result<Value, NuzoError> {
        if self.is_smi() && other.is_smi() {
            let result = self.0.wrapping_sub(other.0).wrapping_add(SMI_TAG);
            if (result & SMI_MASK) == SMI_TAG {
                return Ok(Value(result));
            }
        }
        self.sub_float_fallback(other)
    }

    // --- mul ---

    #[cold]
    #[inline(never)]
    fn mul_float_fallback(self, other: Value) -> Result<Value, NuzoError> {
        if !self.is_number() || !other.is_number() {
            let bad = if !self.is_number() { self.tag() } else { other.tag() };
            return Err(NuzoError::expected_number(format!("{}", bad)));
        }
        Ok(Value::from_number(self.as_number() * other.as_number()))
    }

    pub fn mul(self, other: Value) -> Result<Value, NuzoError> {
        if self.is_smi() && other.is_smi() {
            let a = self.as_smi();
            let b = other.as_smi();
            if let Some(product) = a.checked_mul(b)
                && let Some(result) = Self::try_from_smi(product)
            {
                return Ok(result);
            }
        }
        self.mul_float_fallback(other)
    }

    // --- div ---

    #[cold]
    #[inline(never)]
    fn div_float_fallback(self, other: Value) -> Result<Value, NuzoError> {
        if !self.is_number() || !other.is_number() {
            let bad = if !self.is_number() { self.tag() } else { other.tag() };
            return Err(NuzoError::expected_number(format!("{}", bad)));
        }
        if other.as_number() == 0.0 {
            return Err(NuzoError::division_by_zero());
        }
        Ok(Value::from_number(self.as_number() / other.as_number()))
    }

    pub fn div(self, other: Value) -> Result<Value, NuzoError> {
        if self.is_smi() && other.is_smi() {
            let a = self.as_smi();
            let b = other.as_smi();
            if b != 0
                && a % b == 0
                && let Some(result) = Self::try_from_smi(a / b)
            {
                return Ok(result);
            }
        }
        self.div_float_fallback(other)
    }

    // --- rem ---

    pub fn rem(self, other: Value) -> Result<Value, NuzoError> {
        if !self.is_number() || !other.is_number() {
            let bad = if !self.is_number() { self.tag() } else { other.tag() };
            return Err(NuzoError::expected_number(format!("{}", bad)));
        }
        if other.as_number() == 0.0 {
            return Err(NuzoError::division_by_zero());
        }
        if self.is_smi()
            && other.is_smi()
            && let Some(result) = Self::try_from_smi(self.as_smi() % other.as_smi())
        {
            return Ok(result);
        }
        Ok(Value::from_number(self.as_number() % other.as_number()))
    }

    // --- modulo ---

    pub fn modulo(self, other: Value) -> Result<Value, NuzoError> {
        if !self.is_number() || !other.is_number() {
            // NOTE: 原实现在 nuzo_values 中使用 type_name()（heap-dependent），
            // 此处改用 tag() 保持纯位操作语义，错误信息略有差异但功能等价。
            return Err(NuzoError::type_mismatch(
                "number",
                format!("{}", if !self.is_number() { self.tag() } else { other.tag() }),
            ));
        }
        if other.as_number() == 0.0 {
            return Err(NuzoError::division_by_zero());
        }
        Ok(Value::from_number(self.as_number() % other.as_number()))
    }

    // --- pow ---

    pub fn pow(self, other: Value) -> Result<Value, NuzoError> {
        if !self.is_number() || !other.is_number() {
            return Err(NuzoError::type_mismatch(
                "numbers for exponentiation",
                format!("{}", if !self.is_number() { self.tag() } else { other.tag() }),
            ));
        }
        Ok(Value::from_number(self.as_number().powf(other.as_number())))
    }

    // --- neg ---

    pub fn neg(self) -> Result<Value, NuzoError> {
        if !self.is_number() {
            return Err(NuzoError::expected_number(format!("got {}", self.tag())));
        }
        if self.is_smi()
            && let Some(result) = Self::try_from_smi(-self.as_smi())
        {
            return Ok(result);
        }
        Ok(Value::from_number(-self.as_number()))
    }
}

// ============================================================================
// 单元测试（纯位操作，不涉及堆对象）
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn test_constant_bit_patterns() {
        assert_eq!(NIL.0, NIL_VALUE);
        assert_eq!(FALSE.0, FALSE_VALUE);
        assert_eq!(TRUE.0, TRUE_VALUE);
    }
    #[test]
    fn test_constants_are_special() {
        assert!(NIL.is_special());
        assert!(FALSE.is_special());
        assert!(TRUE.is_special());
        assert!(!NIL.is_number());
        assert!(!FALSE.is_number());
        assert!(!TRUE.is_number());
    }
    #[test]
    fn test_number_roundtrip_simple() {
        let v = Value::from_number(42.0);
        assert_eq!(v.as_number(), 42.0);
    }
    #[test]
    fn test_number_roundtrip_negative() {
        let v = Value::from_number(-17.5);
        assert_eq!(v.as_number(), -17.5);
    }
    #[test]
    fn test_number_roundtrip_zero() {
        let p = Value::from_number(0.0);
        let n = Value::from_number(-0.0);
        assert_eq!(p.as_number(), 0.0);
        assert_eq!(n.as_number(), -0.0);
        assert_ne!(p, n);
    }
    #[test]
    fn test_from_number_creates_valid_number() {
        let v = Value::from_number(123.456);
        assert!(v.is_number());
        assert!(!v.is_special());
        assert!(!v.is_bool());
        assert!(!v.is_nil());
        assert!(!v.is_ptr());
    }
    #[test]
    fn test_try_number_success() {
        assert_eq!(Value::from_number(99.99).try_number(), Some(99.99));
    }
    #[test]
    fn test_try_number_failure_for_special_values() {
        assert_eq!(NIL.try_number(), None);
        assert_eq!(FALSE.try_number(), None);
        assert_eq!(TRUE.try_number(), None);
    }
    #[test]
    fn test_infinity() {
        let p = Value::from_number(f64::INFINITY);
        let n = Value::from_number(f64::NEG_INFINITY);
        assert!(p.is_number());
        assert_eq!(p.as_number(), f64::INFINITY);
        assert!(n.is_number());
        assert_eq!(n.as_number(), f64::NEG_INFINITY);
    }
    #[test]
    fn test_max_min_f64() {
        assert_eq!(Value::from_number(f64::MAX).as_number(), f64::MAX);
        assert_eq!(Value::from_number(f64::MIN).as_number(), f64::MIN);
    }
    #[test]
    fn test_from_bool_true() {
        let v = Value::from_bool(true);
        assert_eq!(v, TRUE);
        assert!(v.is_bool());
    }
    #[test]
    fn test_from_bool_false() {
        let v = Value::from_bool(false);
        assert_eq!(v, FALSE);
        assert!(v.is_bool());
    }
    #[test]
    fn test_as_bool_true() {
        assert!(TRUE.as_bool());
    }
    #[test]
    fn test_as_bool_false() {
        assert!(!FALSE.as_bool());
    }
    #[test]
    fn test_nil_detection() {
        assert!(NIL.is_nil());
        assert!(!FALSE.is_nil());
        assert!(!TRUE.is_nil());
        assert!(!Value::from_number(0.0).is_nil());
    }
    #[test]
    fn test_pointer_roundtrip() {
        // PTR_MASK is 46 bits (bits 46-47 collide with HEAP_TAG/STRING_TAG),
        // so use a dummy address that fits.
        let p = 0x1234_5678_9ABC as *const u8;
        unsafe {
            let v = Value::from_ptr(p);
            assert!(v.is_ptr());
            assert_eq!(v.as_ptr(), p);
        }
    }
    #[test]
    fn test_null_pointer_encoding() {
        unsafe {
            let v = Value::from_ptr(ptr::null());
            assert!(v.is_ptr());
            assert_eq!(v.as_ptr(), ptr::null());
        }
    }
    #[test]
    fn test_display_nil() {
        assert_eq!(format!("{}", NIL), "nil");
    }
    #[test]
    fn test_display_bools() {
        assert_eq!(format!("{}", TRUE), "true");
        assert_eq!(format!("{}", FALSE), "false");
    }
    #[test]
    fn test_display_integers() {
        assert_eq!(format!("{}", Value::from_number(42.0)), "42");
        assert_eq!(format!("{}", Value::from_number(0.0)), "0");
        assert_eq!(format!("{}", Value::from_number(-17.0)), "-17");
    }
    #[test]
    fn test_display_floats() {
        assert_eq!(format!("{}", Value::from_number(2.5)), "2.5");
        assert_eq!(format!("{}", Value::from_number(-0.5)), "-0.5");
    }
    #[test]
    fn test_add_two_numbers() {
        assert_eq!(
            Value::from_number(10.0).add(Value::from_number(32.0)).unwrap().as_number(),
            42.0
        );
    }
    #[test]
    fn test_sub_two_numbers() {
        assert_eq!(
            Value::from_number(50.0).sub(Value::from_number(8.0)).unwrap().as_number(),
            42.0
        );
    }
    #[test]
    fn test_mul_two_numbers() {
        assert_eq!(Value::from_number(6.0).mul(Value::from_number(7.0)).unwrap().as_number(), 42.0);
    }
    #[test]
    fn test_div_two_numbers() {
        assert_eq!(
            Value::from_number(84.0).div(Value::from_number(2.0)).unwrap().as_number(),
            42.0
        );
    }
    #[test]
    fn test_rem_two_numbers() {
        assert_eq!(
            Value::from_number(43.0).rem(Value::from_number(10.0)).unwrap().as_number(),
            3.0
        );
    }
    #[test]
    fn test_neg_number() {
        assert_eq!(Value::from_number(42.0).neg().unwrap().as_number(), -42.0);
    }
    #[test]
    fn test_add_non_number_returns_error() {
        assert!(Value::from_number(1.0).add(NIL).is_err());
    }
    #[test]
    fn test_div_by_zero_returns_error() {
        assert!(Value::from_number(42.0).div(Value::from_number(0.0)).is_err());
    }
    #[test]
    fn test_rem_by_zero_returns_error() {
        assert!(Value::from_number(42.0).rem(Value::from_number(0.0)).is_err());
    }
    #[test]
    fn test_modulo_negative_truncated_remainder() {
        assert_eq!(
            Value::from_number(-7.0).modulo(Value::from_number(3.0)).unwrap().as_number(),
            -1.0
        );
    }
    #[test]
    fn test_modulo_positive_remainder() {
        assert_eq!(
            Value::from_number(7.0).modulo(Value::from_number(3.0)).unwrap().as_number(),
            1.0
        );
    }
    #[test]
    fn test_modulo_negative_dividend_and_divisor() {
        assert_eq!(
            Value::from_number(-7.0).modulo(Value::from_number(-3.0)).unwrap().as_number(),
            -1.0
        );
    }
    #[test]
    fn test_modulo_by_zero_returns_error() {
        assert!(Value::from_number(42.0).modulo(Value::from_number(0.0)).is_err());
    }
    #[test]
    fn test_neg_non_number_returns_error() {
        assert!(NIL.neg().is_err());
    }
    #[test]
    fn test_is_truthy_basic() {
        assert!(!NIL.is_truthy());
        assert!(!FALSE.is_truthy());
        assert!(TRUE.is_truthy());
        assert!(!Value::from_number(0.0).is_truthy());
    }
    #[test]
    fn test_value_equals_number_coercion() {
        assert!(Value::from_number(42.0).value_equals(&Value::from_number(42.0)));
    }
    #[test]
    fn test_value_equals_different_types() {
        assert!(!Value::from_number(1.0).value_equals(&TRUE));
    }
    #[test]
    fn test_default_is_nil() {
        assert!(Value::default().is_nil());
    }
    #[test]
    fn test_debug_output_contains_info() {
        assert!(format!("{:?}", NIL).contains("nil"));
        let n = format!("{:?}", Value::from_number(42.0));
        assert!(n.contains("Smi") || n.contains("Float"));
    }
    #[test]
    fn test_complex_arithmetic_chain() {
        let r = Value::from_number(10.0)
            .add(Value::from_number(20.0))
            .unwrap()
            .mul(Value::from_number(3.0))
            .unwrap()
            .sub(Value::from_number(5.0))
            .unwrap();
        assert_eq!(r.as_number(), 85.0);
    }
    #[test]
    fn test_mixed_type_operations_fail_gracefully() {
        for (a, b) in [(NIL, Value::from_number(1.0)), (TRUE, Value::from_number(1.0))] {
            assert!(a.add(b).is_err());
            assert!(a.sub(b).is_err());
        }
    }
    #[test]
    fn test_pointer_does_not_collide_with_constants() {
        assert!(!NIL.is_ptr());
        assert!(!FALSE.is_ptr());
        assert!(!TRUE.is_ptr());
    }

    // ─── is_bool / is_float / is_heap_object 直接测试 ───
    #[test]
    fn test_is_bool_direct() {
        assert!(TRUE.is_bool());
        assert!(FALSE.is_bool());
        assert!(!NIL.is_bool());
        assert!(!Value::from_number(1.0).is_bool());
    }
    #[test]
    fn test_is_float_direct() {
        assert!(Value::from_number(2.5).is_float());
        assert!(!Value::from_smi(42).is_float());
        assert!(!NIL.is_float());
        assert!(!TRUE.is_float());
    }

    // ─── as_smi / try_from_smi 直接测试 ───
    #[test]
    fn test_as_smi_positive() {
        assert_eq!(Value::from_smi(42).as_smi(), 42);
    }
    #[test]
    fn test_as_smi_negative() {
        assert_eq!(Value::from_smi(-100).as_smi(), -100);
    }
    #[test]
    fn test_as_smi_zero() {
        assert_eq!(Value::from_smi(0).as_smi(), 0);
    }
    #[test]
    fn test_try_from_smi_in_range() {
        assert!(Value::try_from_smi(0).is_some());
        assert!(Value::try_from_smi(SMI_MAX).is_some());
        assert!(Value::try_from_smi(SMI_MIN).is_some());
    }
    #[test]
    fn test_try_from_smi_out_of_range() {
        assert!(Value::try_from_smi(SMI_MAX + 1).is_none());
        assert!(Value::try_from_smi(SMI_MIN - 1).is_none());
    }

    // ─── to_string_repr ───
    #[test]
    fn test_to_string_repr_nil() {
        assert_eq!(NIL.to_string_repr(), "nil");
    }
    #[test]
    fn test_to_string_repr_number() {
        assert_eq!(Value::from_number(42.0).to_string_repr(), "42");
    }

    // ─── from_string_index ───
    #[test]
    fn test_from_string_index() {
        let v = Value::from_string_index(42);
        assert!(v.is_string());
        assert_eq!(v.string_index(), Some(42));
    }

    // ─── heap_idx_or_err ───
    #[test]
    fn test_heap_idx_or_err_non_heap() {
        assert!(NIL.heap_idx_or_err().is_err());
        assert!(Value::from_number(42.0).heap_idx_or_err().is_err());
    }

    // ─── from_gc_index / from_scratch_index / from_arena_index ───
    #[test]
    fn test_from_gc_index() {
        let v = Value::from_gc_index(42);
        assert!(v.is_heap_object());
        assert!(v.is_gc_managed());
        assert_eq!(v.heap_index(), Some(42));
    }
    #[test]
    fn test_from_scratch_index() {
        let v = Value::from_scratch_index(SCRATCH_BASE);
        assert!(v.is_heap_object());
        assert!(v.is_gc_managed());
        assert!(Value::is_scratch_index(v.heap_index().unwrap()));
    }
    #[test]
    fn test_from_arena_index() {
        let v = Value::from_arena_index(0);
        assert!(v.is_heap_object());
        assert!(v.is_gc_managed());
        assert!(Value::is_arena_index(v.heap_index().unwrap()));
    }
    #[test]
    fn test_from_arena_index_max_offset() {
        let v = Value::from_arena_index(ARENA_MASK);
        let idx = v.heap_index().unwrap();
        assert!(Value::is_arena_index(idx));
        assert_eq!(v.try_arena_offset(), Some(ARENA_MASK));
    }

    // ─── is_scratch_index / is_arena_index ───
    #[test]
    fn test_is_scratch_index() {
        assert!(Value::is_scratch_index(SCRATCH_BASE));
        assert!(Value::is_scratch_index(u32::MAX));
        assert!(!Value::is_scratch_index(0));
        assert!(!Value::is_scratch_index(SCRATCH_BASE - 1));
    }
    #[test]
    fn test_is_arena_index() {
        assert!(Value::is_arena_index(ARENA_BASE));
        assert!(Value::is_arena_index(SCRATCH_BASE - 1));
        assert!(!Value::is_arena_index(0));
        assert!(!Value::is_arena_index(SCRATCH_BASE));
        assert!(!Value::is_arena_index(ARENA_BASE - 1));
    }

    // ─── try_arena_offset ───
    #[test]
    fn test_try_arena_offset_arena_value() {
        let v = Value::from_arena_index(100);
        assert_eq!(v.try_arena_offset(), Some(100));
    }
    #[test]
    fn test_try_arena_offset_gc_value() {
        let v = Value::from_gc_index(42);
        assert_eq!(v.try_arena_offset(), None);
    }
    #[test]
    fn test_try_arena_offset_non_heap() {
        assert_eq!(NIL.try_arena_offset(), None);
        assert_eq!(Value::from_number(1.0).try_arena_offset(), None);
    }

    // ─── try_remap ───
    #[test]
    fn test_try_remap_scratch_to_persistent() {
        let mut v = Value::from_scratch_index(SCRATCH_BASE);
        let remap = vec![(SCRATCH_BASE, 10u32)];
        assert!(v.try_remap(&remap));
        assert_eq!(v.heap_index(), Some(10));
        assert!(!Value::is_scratch_index(v.heap_index().unwrap()));
    }
    #[test]
    fn test_try_remap_no_match() {
        let mut v = Value::from_scratch_index(SCRATCH_BASE);
        let remap = vec![(SCRATCH_BASE + 1, 10u32)];
        assert!(!v.try_remap(&remap));
    }
    #[test]
    fn test_try_remap_persistent_value() {
        let mut v = Value::from_gc_index(42);
        let remap = vec![(42, 100u32)];
        assert!(!v.try_remap(&remap));
    }
    #[test]
    fn test_try_remap_non_heap() {
        let mut v = NIL;
        let remap = vec![(SCRATCH_BASE, 10u32)];
        assert!(!v.try_remap(&remap));
    }
    #[test]
    fn test_try_remap_binary_search() {
        let mut v = Value::from_scratch_index(SCRATCH_BASE + 5);
        let remap =
            vec![(SCRATCH_BASE, 1u32), (SCRATCH_BASE + 5, 99u32), (SCRATCH_BASE + 10, 200u32)];
        assert!(v.try_remap(&remap));
        assert_eq!(v.heap_index(), Some(99));
    }

    // ─── tag 分类 ───
    #[test]
    fn test_tag_classification() {
        assert_eq!(NIL.tag(), ValueTag::Nil);
        assert_eq!(TRUE.tag(), ValueTag::Bool);
        assert_eq!(Value::from_smi(42).tag(), ValueTag::Smi);
        assert_eq!(Value::from_number(2.5).tag(), ValueTag::Float);
    }

    // ─── gc_managed ───
    #[test]
    fn test_gc_managed_bit_distinguishes_heaps() {
        assert!(!unsafe { Value::from_raw_bits(HEAP_TAG | 42) }.is_gc_managed());
        assert!(unsafe { Value::from_raw_bits(HEAP_TAG | GC_MANAGED_BIT | 42) }.is_gc_managed());
    }
    #[test]
    fn test_heap_index_excludes_gc_bit() {
        assert_eq!(
            unsafe { Value::from_raw_bits(HEAP_TAG | GC_MANAGED_BIT | 42) }.heap_index(),
            Some(42)
        );
        assert_eq!(unsafe { Value::from_raw_bits(HEAP_TAG | 42) }.heap_index(), Some(42));
    }
}
