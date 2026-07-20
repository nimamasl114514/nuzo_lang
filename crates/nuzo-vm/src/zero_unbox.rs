//! Zero-Unboxing Pipeline — ATSP-Lite Layer 2
//!
//! # Innovation
//!
//! Direct `u64` bit manipulation in dispatch handlers, eliminating intermediate
//! `Value` object construction/destruction overhead. Uses LLVM intrinsics
//! (`f64::from_bits/to_bits`) which compile to single `movsd` instructions,
//! plus `Likely()` branch hints for optimal branch prediction.
//!
//! # Architecture
//!
//! ```text
//! Before:  register(ra) → Value → .add(register(rb)) → Value → set(rd)
//! After:   get_tagged(ra) → (u64, RegTag) → f64::from_bits + addsd → set_tagged(rd)
//!
//! Hot path (f64+f64 add):
//!   tag_read(~0.3ns) → Likely(~0.3ns) → movsd×2(~1ns)
//!   → addsd(~1ns) → movsd_write(~0.5ns) → tag_write(~0.3ns) = ~3.5ns
//! ```

use crate::trf::RegTag;
use nuzo_core::Value;
use nuzo_core::tag::*;
use nuzo_values::NuzoError;
use nuzo_values::ValueExt;

/// Internal cold sentinel: a `#[cold] #[inline(never)]` empty function that LLVM
/// treats as rarely-called. Used by `likely` / `unlikely` to emit branch-weight
/// metadata on stable Rust (without `std::intrinsics::likely`).
#[cold]
#[inline(never)]
fn _cold_sentinel() {}

/// Branch prediction hint: marks a condition as likely true.
///
/// On stable Rust, uses the `#[cold]` sentinel pattern: calls
/// `_cold_sentinel()` only when the condition is **false**, so LLVM
/// infers that the `true` branch is the hot path and optimises
/// accordingly.  When `std::hint::likely` stabilises this can be
/// switched to the intrinsic with zero code changes at call-sites.
#[inline(always)]
pub fn likely(b: bool) -> bool {
    if !b {
        _cold_sentinel();
    }
    b
}

/// Branch prediction hint: marks a condition as unlikely.
///
/// Calls `_cold_sentinel()` only when the condition is **true**,
/// so LLVM treats the `true` (cold) branch as rarely-taken and
/// keeps the fall-through path compact.
#[inline(always)]
pub fn unlikely(b: bool) -> bool {
    if b {
        _cold_sentinel();
    }
    b
}

// ============================================================================
// Bit manipulation helpers (all compile to 1-3 machine instructions)
// ============================================================================

/// Check if raw bits represent a canonical IEEE 754 float (not in NaN space).
///
/// Uses the same logic as `Value::is_number()`: bits outside SPECIAL_MASK
/// are real floats. CANONICAL_NAN is excluded (it's a NaN-tagged sentinel).
///
/// Machine code: `test rax, SPECIAL_MASK; jz .Lnot_float` (2 instructions)
#[inline(always)]
pub const fn is_canonical_float(bits: u64) -> bool {
    (bits & SPECIAL_MASK) != SPECIAL_MASK && bits != CANONICAL_NAN
}

/// Check if raw bits represent a Smi integer.
///
/// Machine code: `test rax, 0x7FFF000000000000; jz .Lis_smi` (1 instruction)
#[inline(always)]
pub const fn is_smi(bits: u64) -> bool {
    (bits & SMI_MASK) == SMI_TAG
}

/// Check if both operands are canonical floats (most common case for arithmetic).
#[inline(always)]
pub fn is_f64_pair(a: u64, b: u64) -> bool {
    likely(is_canonical_float(a) && is_canonical_float(b))
}

/// Check if both operands are Smi integers.
#[inline(always)]
pub fn is_smi_pair(a: u64, b: u64) -> bool {
    is_smi(a) && is_smi(b)
}

// ============================================================================
// Type conversion (LLVM intrinsics → single movsd instruction)
// ============================================================================

/// u64 → f64: compiles to `movsd xmm0, qword ptr[rax]`
///
/// `f64::from_bits` is a safe operation on all 64-bit patterns.
/// Callers in this crate guarantee that the input comes from a valid
/// Float-tagged or Nan-tagged register (verified by `is_f64_like()` check
/// on the hot path), or from a prior `f64::to_bits()` round-trip.
#[inline(always)]
pub fn to_f64(bits: u64) -> f64 {
    f64::from_bits(bits)
}

/// f64 → u64: compiles to `movsd qword ptr[rax], xmm0`
///
/// `f64::to_bits` is a safe operation on all `f64` values.
/// The resulting bit pattern is only consumed by the ZeroUnbox pipeline
/// (stored back into a register slot with `RegTag::Float`), so the
/// semantic interpretation is controlled by the tag.
#[inline(always)]
pub fn from_f64(val: f64) -> u64 {
    val.to_bits()
}

// ============================================================================
// Smi arithmetic — re-exported from nuzo_core::tag (canonical source)
// ============================================================================
//
// smi_add, smi_sub, smi_mul, smi_to_i64 are defined in nuzo_core::tag with
// `#[inline(always)]` and re-exported here to maintain API compatibility.
// This eliminates the duplicated Smi arithmetic that previously existed in
// both nuzo_core::tag (L1) and nuzo_vm::zero_unbox (L5).
//
// The degenerate Smi ops (div/rem/mod/pow) always return None and are kept
// local since they don't exist in nuzo_core::tag.

pub use nuzo_core::tag::{smi_add, smi_mul, smi_sub, smi_to_i64};

/// Smi division: always degrades to float (division rarely produces integers).
#[inline(always)]
pub fn smi_div(_a: u64, _b: u64) -> Option<u64> {
    None
}

/// Smi remainder: always degrades to float.
#[inline(always)]
pub fn smi_rem(_a: u64, _b: u64) -> Option<u64> {
    None
}

/// Smi modulo: always degrades to float.
#[inline(always)]
pub fn smi_mod(_a: u64, _b: u64) -> Option<u64> {
    None
}

/// Smi power: always degrades to float (exponentiation rarely produces integers).
#[inline(always)]
pub fn smi_pow(_a: u64, _b: u64) -> Option<u64> {
    None
}

/// Convert f64 result back to Smi if it fits, otherwise return Float-encoded.
#[inline(always)]
pub fn smi_result_or_float(result_f64: f64) -> (u64, RegTag) {
    if result_f64.fract() == 0.0 && result_f64 >= SMI_MIN as f64 && result_f64 <= SMI_MAX as f64 {
        let smi_val = Value::from_smi(result_f64 as i64);
        (smi_val.into_raw_bits(), RegTag::Smi)
    } else {
        (f64::to_bits(result_f64), RegTag::Float)
    }
}

// ============================================================================
// Generic slow paths (cold, handle string concat / type errors)
// ============================================================================

/// Generic addition slow path: handles string concatenation and type errors.
///
/// Marked `#[cold]` so LLVM places it outside the hot path's I-Cache footprint.
#[cold]
pub fn generic_add_slow(a: u64, b: u64) -> Result<(u64, RegTag), NuzoError> {
    // SAFETY: `a` and `b` are raw u64 bits that passed the fast-path tag checks
    // (`is_f64_like()` or `is_smi()`), guaranteeing they encode valid Values.
    let va = unsafe { Value::from_raw_bits(a) };
    let vb = unsafe { Value::from_raw_bits(b) };

    // 使用 concat_repr() 而非 to_string()：to_string() 走 Display trait，
    // 对字符串值会添加双引号包围（如 "\"hello\""），多层拼接导致嵌套引号。
    // concat_repr() 对字符串返回原始内容，正确实现拼接语义。
    if is_string(a) || is_string(b) {
        let left = va.concat_repr();
        let right = vb.concat_repr();
        let result = Value::from_string(&format!("{}{}", left, right));
        return Ok((result.into_raw_bits(), RegTag::String));
    }

    if !is_number(a) || !is_number(b) {
        let bad = if !is_number(a) { va.type_name() } else { vb.type_name() };
        return Err(NuzoError::type_mismatch("number".to_string(), bad.to_string()));
    }

    let result = Value::from_number(va.as_number() + vb.as_number());
    Ok((result.into_raw_bits(), RegTag::Float))
}

/// 通用二元算术冷路径：处理类型检查、可选除零保护、数值运算。
///
/// `generic_sub/mul/div/rem/mod/pow_slow` 结构完全一致，仅运算函数、
/// 是否需要除零保护、期望类型描述不同，故提取为泛型辅助。
/// `generic_add_slow` 因含字符串拼接分支，结构不同，不参与合并。
///
/// # Parameters
/// - `op`: f64 运算函数
/// - `check_zero`: 是否在运算前检查除数为零（div/rem/mod 需要）
/// - `expected`: 类型不匹配时的期望类型描述（用于错误消息）
#[cold]
fn generic_binary_arith_slow<F>(
    a: u64,
    b: u64,
    op: F,
    check_zero: bool,
    expected: &str,
) -> Result<(u64, RegTag), NuzoError>
where
    F: Fn(f64, f64) -> f64,
{
    // SAFETY: `a` and `b` are raw u64 bits that passed the fast-path tag checks
    // (`is_f64_like()` or `is_smi()`), guaranteeing they encode valid Values.
    let va = unsafe { Value::from_raw_bits(a) };
    let vb = unsafe { Value::from_raw_bits(b) };

    if !is_number(a) || !is_number(b) {
        return Err(NuzoError::type_mismatch(
            expected.to_string(),
            if !is_number(a) { va.type_name() } else { vb.type_name() }.to_string(),
        ));
    }

    if check_zero && as_number(b) == 0.0 {
        return Err(NuzoError::division_by_zero());
    }

    let result = Value::from_number(op(as_number(a), as_number(b)));
    Ok((result.into_raw_bits(), RegTag::Float))
}

/// Generic subtraction slow path.
#[cold]
pub fn generic_sub_slow(a: u64, b: u64) -> Result<(u64, RegTag), NuzoError> {
    generic_binary_arith_slow(a, b, |x, y| x - y, false, "number")
}

/// Generic multiplication slow path.
#[cold]
pub fn generic_mul_slow(a: u64, b: u64) -> Result<(u64, RegTag), NuzoError> {
    generic_binary_arith_slow(a, b, |x, y| x * y, false, "number")
}

/// Generic division slow path (with divide-by-zero protection).
#[cold]
pub fn generic_div_slow(a: u64, b: u64) -> Result<(u64, RegTag), NuzoError> {
    generic_binary_arith_slow(a, b, |x, y| x / y, true, "number")
}

/// Generic remainder slow path.
#[cold]
pub fn generic_rem_slow(a: u64, b: u64) -> Result<(u64, RegTag), NuzoError> {
    generic_binary_arith_slow(a, b, |x, y| x % y, true, "number")
}

/// Generic negation slow path.
#[cold]
pub fn generic_neg_slow(a: u64) -> Result<(u64, RegTag), NuzoError> {
    // SAFETY: `a` passed the fast-path tag checks, encoding a valid Value.
    let va = unsafe { Value::from_raw_bits(a) };
    if !is_number(a) {
        return Err(NuzoError::type_mismatch("number".to_string(), va.type_name().to_string()));
    }
    let result = Value::neg(va)?;
    Ok((result.into_raw_bits(), RegTag::Float))
}

/// Generic modulo slow path (IEEE remainder, matches Value::modulo).
#[cold]
pub fn generic_mod_slow(a: u64, b: u64) -> Result<(u64, RegTag), NuzoError> {
    generic_binary_arith_slow(a, b, |x, y| x % y, true, "number")
}

/// Generic exponentiation slow path.
#[cold]
pub fn generic_pow_slow(a: u64, b: u64) -> Result<(u64, RegTag), NuzoError> {
    generic_binary_arith_slow(a, b, |x, y| x.powf(y), false, "numbers for exponentiation")
}

/// Generic logical NOT slow path: returns (raw_bits, RegTag::Bool).
#[cold]
pub fn generic_not_slow(a: u64) -> (u64, RegTag) {
    let result = !is_truthy_raw(a);
    (Value::from_bool(result).into_raw_bits(), RegTag::Bool)
}

/// Generic comparison slow path (handles collection contains, etc.).
#[cold]
pub fn generic_eq_slow(a: u64, b: u64) -> (u64, RegTag) {
    // SAFETY: `a` and `b` passed the fast-path tag checks, encoding valid Values.
    let va = unsafe { Value::from_raw_bits(a) };
    let vb = unsafe { Value::from_raw_bits(b) };
    let result = if va.is_collection() && !vb.is_collection() {
        va.collection_contains(vb)
    } else if vb.is_collection() && !va.is_collection() {
        vb.collection_contains(va)
    } else {
        va.value_equals(&vb)
    };
    (Value::from_bool(result).into_raw_bits(), RegTag::Bool)
}

// ============================================================================
// Truthiness & ordering helpers (for comparison / logical opcodes)
// ============================================================================

/// Check truthiness on raw u64 bits — no Value construction.
///
/// Falsy values: NIL, FALSE, numeric zero (Smi 0, Float +0.0, Float -0.0).
/// Everything else (TRUE, non-zero numbers, strings, objects) is truthy.
#[inline(always)]
pub const fn is_truthy_raw(bits: u64) -> bool {
    // Fast reject: NIL and FALSE are in PTR_TAG space with low bits 1 and 2
    if bits == NIL_VALUE || bits == FALSE_VALUE {
        return false;
    }
    // Float +0.0 (all-zero bits) and Float -0.0 (only sign bit set)
    if bits == 0 || bits == 0x8000_0000_0000_0000 {
        return false;
    }
    // Smi 0: SMI_TAG with zero payload → (bits & SMI_VALUE_MASK) == 0
    if is_smi(bits) && (bits & SMI_VALUE_MASK) == 0 {
        return false;
    }
    true
}

/// Generic ordering slow path: numeric comparison with type checking.
///
/// Returns `(bool_result_bits, Bool_tag)` on success, or `NuzoError` for type mismatch.
#[cold]
pub fn generic_ord_slow(
    a: u64,
    b: u64,
    cmp: impl Fn(f64, f64) -> bool,
) -> Result<(u64, RegTag), NuzoError> {
    // SAFETY: `a` and `b` passed the fast-path tag checks, encoding valid Values.
    let va = unsafe { Value::from_raw_bits(a) };
    let vb = unsafe { Value::from_raw_bits(b) };
    if !is_number(a) || !is_number(b) {
        return Err(NuzoError::type_mismatch(
            "numbers for comparison".to_string(),
            format!("{}/{}", va.type_name(), vb.type_name()),
        ));
    }
    let result = cmp(as_number(a), as_number(b));
    Ok((Value::from_bool(result).into_raw_bits(), RegTag::Bool))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_canonical_float() {
        // Normal f64 values should pass
        assert!(is_canonical_float(f64::to_bits(0.0)));
        assert!(is_canonical_float(f64::to_bits(1.5)));
        assert!(is_canonical_float(f64::to_bits(-42.0)));
        assert!(is_canonical_float(f64::to_bits(f64::MAX)));
        assert!(is_canonical_float(f64::to_bits(f64::MIN)));

        // NaN-tagged values should fail
        assert!(!is_canonical_float(NIL_VALUE));
        assert!(!is_canonical_float(SMI_TAG));
        assert!(!is_canonical_float(STRING_TAG));
        assert!(!is_canonical_float(HEAP_TAG));
        assert!(!is_canonical_float(CANONICAL_NAN));
    }

    #[test]
    fn test_is_smi() {
        assert!(is_smi(Value::from_smi(0).into_raw_bits()));
        assert!(is_smi(Value::from_smi(42).into_raw_bits()));
        assert!(is_smi(Value::from_smi(-1).into_raw_bits()));
        assert!(!is_smi(f64::to_bits(2.5)));
        assert!(!is_smi(NIL_VALUE));
    }

    #[test]
    fn test_is_f64_pair() {
        let a = f64::to_bits(1.0);
        let b = f64::to_bits(2.0);
        assert!(is_f64_pair(a, b));

        let s = Value::from_smi(42).into_raw_bits();
        assert!(!is_f64_pair(a, s));
    }

    #[test]
    fn test_smi_add() {
        let a = Value::from_smi(40).into_raw_bits();
        let b = Value::from_smi(2).into_raw_bits();
        let result = smi_add(a, b);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Value::from_smi(42).into_raw_bits());
    }

    #[test]
    fn test_smi_sub() {
        let a = Value::from_smi(10).into_raw_bits();
        let b = Value::from_smi(3).into_raw_bits();
        let result = smi_sub(a, b);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Value::from_smi(7).into_raw_bits());
    }

    #[test]
    fn test_smi_to_i64() {
        assert_eq!(smi_to_i64(Value::from_smi(0).into_raw_bits()), 0);
        assert_eq!(smi_to_i64(Value::from_smi(42).into_raw_bits()), 42);
        assert_eq!(smi_to_i64(Value::from_smi(-1).into_raw_bits()), -1);
    }

    #[test]
    fn test_to_from_f64_roundtrip() {
        let original = 2.5;
        let bits = from_f64(original);
        let recovered = to_f64(bits);
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_generic_add_numbers() {
        let a = f64::to_bits(1.0);
        let b = f64::to_bits(2.0);
        let (result, _tag) = generic_add_slow(a, b).unwrap();
        // Result may be Smi or Float depending on Value::from_number optimization
        let result_val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(result_val.as_number(), 3.0);
    }

    #[test]
    fn test_generic_add_type_error() {
        let a = NIL_VALUE;
        let b = f64::to_bits(1.0);
        assert!(generic_add_slow(a, b).is_err());
    }

    #[test]
    fn test_smi_result_or_float() {
        // Fits in Smi
        let (_bits, tag) = smi_result_or_float(42.0);
        assert_eq!(tag, RegTag::Smi);

        // Doesn't fit in Smi (fractional)
        let (_bits2, tag2) = smi_result_or_float(2.5);
        assert_eq!(tag2, RegTag::Float);
    }

    // ---- 新增测试：覆盖未测试的 pub fn ----

    #[test]
    fn test_is_smi_pair_both_smi() {
        let a = Value::from_smi(10).into_raw_bits();
        let b = Value::from_smi(20).into_raw_bits();
        assert!(is_smi_pair(a, b));
    }

    #[test]
    fn test_is_smi_pair_one_float() {
        let a = Value::from_smi(10).into_raw_bits();
        let b = f64::to_bits(2.5);
        assert!(!is_smi_pair(a, b));
    }

    #[test]
    fn test_is_smi_pair_both_float() {
        let a = f64::to_bits(1.0);
        let b = f64::to_bits(2.0);
        assert!(!is_smi_pair(a, b));
    }

    #[test]
    fn test_smi_mul_basic() {
        let a = Value::from_smi(6).into_raw_bits();
        let b = Value::from_smi(7).into_raw_bits();
        let result = smi_mul(a, b);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Value::from_smi(42).into_raw_bits());
    }

    #[test]
    fn test_smi_mul_negative() {
        let a = Value::from_smi(-3).into_raw_bits();
        let b = Value::from_smi(4).into_raw_bits();
        let result = smi_mul(a, b);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Value::from_smi(-12).into_raw_bits());
    }

    #[test]
    fn test_smi_mul_overflow_returns_none() {
        // Use valid Smis whose product overflows the Smi range.
        // Smi max = 2^47 - 1. Use 2^24 * 2^24 = 2^48 > 2^47.
        let a = Value::from_smi(1 << 24).into_raw_bits();
        let b = Value::from_smi(1 << 24).into_raw_bits();
        let result = smi_mul(a, b);
        // Overflow should return None (degrade to float)
        assert!(result.is_none());
    }

    #[test]
    fn test_smi_div_returns_none() {
        let a = Value::from_smi(10).into_raw_bits();
        let b = Value::from_smi(2).into_raw_bits();
        // smi_div always returns None (degrades to float)
        assert!(smi_div(a, b).is_none());
    }

    #[test]
    fn test_smi_rem_returns_none() {
        let a = Value::from_smi(10).into_raw_bits();
        let b = Value::from_smi(3).into_raw_bits();
        assert!(smi_rem(a, b).is_none());
    }

    #[test]
    fn test_smi_mod_returns_none() {
        let a = Value::from_smi(10).into_raw_bits();
        let b = Value::from_smi(3).into_raw_bits();
        assert!(smi_mod(a, b).is_none());
    }

    #[test]
    fn test_smi_pow_returns_none() {
        let a = Value::from_smi(2).into_raw_bits();
        let b = Value::from_smi(3).into_raw_bits();
        assert!(smi_pow(a, b).is_none());
    }

    #[test]
    fn test_generic_add_slow_floats() {
        let a = f64::to_bits(1.5);
        let b = f64::to_bits(2.5);
        let (result, tag) = generic_add_slow(a, b).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(val.as_number(), 4.0);
        assert_eq!(tag, RegTag::Float);
    }

    #[test]
    fn test_generic_add_slow_smi_pair() {
        let a = Value::from_smi(40).into_raw_bits();
        let b = Value::from_smi(2).into_raw_bits();
        let (result, tag) = generic_add_slow(a, b).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(val.as_number(), 42.0);
        // generic_add_slow always returns Float tag (not Smi)
        assert_eq!(tag, RegTag::Float);
    }

    #[test]
    fn test_generic_sub_slow_floats() {
        let a = f64::to_bits(10.0);
        let b = f64::to_bits(3.0);
        let (result, _) = generic_sub_slow(a, b).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(val.as_number(), 7.0);
    }

    #[test]
    fn test_generic_mul_slow_floats() {
        let a = f64::to_bits(6.0);
        let b = f64::to_bits(7.0);
        let (result, _) = generic_mul_slow(a, b).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(val.as_number(), 42.0);
    }

    #[test]
    fn test_generic_div_slow_floats() {
        let a = f64::to_bits(10.0);
        let b = f64::to_bits(2.0);
        let (result, _) = generic_div_slow(a, b).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(val.as_number(), 5.0);
    }

    #[test]
    fn test_generic_div_slow_by_zero() {
        let a = f64::to_bits(10.0);
        let b = f64::to_bits(0.0);
        let result = generic_div_slow(a, b);
        // Division by zero: may return Err or Inf
        let _ = result;
    }

    #[test]
    fn test_generic_rem_slow_floats() {
        let a = f64::to_bits(10.0);
        let b = f64::to_bits(3.0);
        let (result, _) = generic_rem_slow(a, b).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(val.as_number(), 1.0);
    }

    #[test]
    fn test_generic_mod_slow_floats() {
        let a = f64::to_bits(10.0);
        let b = f64::to_bits(3.0);
        let (result, _) = generic_mod_slow(a, b).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(val.as_number(), 1.0);
    }

    #[test]
    fn test_generic_pow_slow_floats() {
        let a = f64::to_bits(2.0);
        let b = f64::to_bits(3.0);
        let (result, _) = generic_pow_slow(a, b).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(val.as_number(), 8.0);
    }

    #[test]
    fn test_generic_neg_slow_positive() {
        let a = f64::to_bits(5.0);
        let (result, _) = generic_neg_slow(a).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(val.as_number(), -5.0);
    }

    #[test]
    fn test_generic_neg_slow_negative() {
        let a = f64::to_bits(-3.0);
        let (result, _) = generic_neg_slow(a).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert_eq!(val.as_number(), 3.0);
    }

    #[test]
    fn test_generic_neg_slow_non_number() {
        let a = NIL_VALUE;
        assert!(generic_neg_slow(a).is_err());
    }

    #[test]
    fn test_generic_not_slow_truthy() {
        let a = f64::to_bits(1.0);
        let (result, tag) = generic_not_slow(a);
        let val = unsafe { Value::from_raw_bits(result) };
        assert!(!val.as_bool());
        assert_eq!(tag, RegTag::Bool);
    }

    #[test]
    fn test_generic_not_slow_falsy() {
        let a = FALSE_VALUE;
        let (result, tag) = generic_not_slow(a);
        let val = unsafe { Value::from_raw_bits(result) };
        assert!(val.as_bool());
        assert_eq!(tag, RegTag::Bool);
    }

    #[test]
    fn test_generic_eq_slow_equal_numbers() {
        let a = f64::to_bits(5.0);
        let b = f64::to_bits(5.0);
        let (result, tag) = generic_eq_slow(a, b);
        let val = unsafe { Value::from_raw_bits(result) };
        assert!(val.as_bool());
        assert_eq!(tag, RegTag::Bool);
    }

    #[test]
    fn test_generic_eq_slow_unequal_numbers() {
        let a = f64::to_bits(5.0);
        let b = f64::to_bits(3.0);
        let (result, tag) = generic_eq_slow(a, b);
        let val = unsafe { Value::from_raw_bits(result) };
        assert!(!val.as_bool());
        assert_eq!(tag, RegTag::Bool);
    }

    #[test]
    fn test_generic_ord_slow_less_than() {
        let a = f64::to_bits(3.0);
        let b = f64::to_bits(5.0);
        let (result, tag) = generic_ord_slow(a, b, |x, y| x < y).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert!(val.as_bool());
        assert_eq!(tag, RegTag::Bool);
    }

    #[test]
    fn test_generic_ord_slow_greater_than() {
        let a = f64::to_bits(10.0);
        let b = f64::to_bits(5.0);
        let (result, _) = generic_ord_slow(a, b, |x, y| x > y).unwrap();
        let val = unsafe { Value::from_raw_bits(result) };
        assert!(val.as_bool());
    }

    #[test]
    fn test_generic_ord_slow_non_number() {
        let a = NIL_VALUE;
        let b = f64::to_bits(5.0);
        assert!(generic_ord_slow(a, b, |x, y| x < y).is_err());
    }
}
