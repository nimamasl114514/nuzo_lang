//! Opcode Direct Dispatch Table -- `define_dispatch_auto!` 自动生成
//!
//! 通过 `nuzo_proc::define_dispatch_auto!` 从 Opcode 枚举自动推导 handler 映射。
//! 命名约定：`Opcode::CamelCaseName` → `_op_snake_case_name`。
//! 仅需为不符合约定的 handler 指定例外映射。
//!
//! # Architecture
//!
//! ```text
//! get_handler(Opcode::LoadK)  -->  Some(_op_loadk)
//!                                       |
//!                                       v
//!                                 (内联 handler 逻辑 / vm.op_xxx())
//! ```
//!
//! # ZeroUnbox Pipeline
//!
//! Arithmetic, comparison, and unary opcodes use the ZeroUnbox fast path:
//! - Read registers as `(u64, RegTag)` via `vm.register_tagged()`
//! - Branch on `RegTag` for type-specific fast paths (f64+f64, Smi+Smi)
//! - Fall through to `#[cold]` slow paths for edge cases
//!
//! # Usage
//!
//! 以下代码展示 `dispatch_opcode_fast` 在 VM 主循环中的调用位置；
//! 该函数为 crate 内部函数，无法在 doctest 中编译运行。
//!
//! ```rust,ignore
//! use crate::vm::dispatch_table::dispatch_opcode_fast;
//!
//! // In the main execution loop:
//! dispatch_opcode_fast(vm, opcode)?;
//! ```

use crate::trf::{RegTag, TypedRegFile};
use crate::zero_unbox;
use nuzo_bytecode::Opcode;
use nuzo_core::Value;
use nuzo_core::tag;
use nuzo_values::ValueExt;
use nuzo_values::{FALSE, NIL, NuzoError, TRUE};

// ============================================================================
// Handler Function Type
// ============================================================================

/// Unified handler signature for all opcode implementations.
///
/// Every opcode -- whether a simple load, an arithmetic operation, or a complex
/// control-flow operation -- conforms to this single function-pointer type.
type OpHandler = fn(&mut super::VM) -> Result<(), NuzoError>;

// ============================================================================
// Wrapper Functions: thin adapters from free-function to VM method / logic
//
// Naming convention: _op_xxx -- private, maps 1:1 to Opcode variants.
// Grouped by semantic category for readability.
//
// STITCH: 新增 Opcode 时必须同步此处 — 为每个变体添加对应的 _op_xxx
// 函数（共享 dispatch_kind 的指令可复用已有函数）。
// ============================================================================

// ---- Constants & Variables ----

/// LoadK: read constant-pool value into register.
///
/// ZeroUnbox: after loading the constant Value, we write it through
/// `set_register_tagged` with an inferred tag so that downstream
/// ZeroUnbox handlers can benefit from the tag information.
fn _op_loadk(vm: &mut super::VM) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    let const_idx = vm.read_u16()? as usize;
    let chunk = vm.current_chunk()?;
    if zero_unbox::unlikely(const_idx >= chunk.constants().len()) {
        return Err(NuzoError::internal(
            nuzo_values::InternalError::ConstantOutOfBounds {
                index: const_idx,
                pool_size: chunk.constants().len(),
            },
            None,
        ));
    }
    let val = chunk.constants()[const_idx];
    let raw = val.into_raw_bits();
    let tag = TypedRegFile::infer_tag(raw);
    vm.set_register_tagged(dest, raw, tag)
}

/// LoadNil: write nil into register.
fn _op_loadnil(vm: &mut super::VM) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    vm.set_register_tagged(dest, NIL.into_raw_bits(), RegTag::Nil)
}

/// LoadTrue: write true into register.
fn _op_loadtrue(vm: &mut super::VM) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    vm.set_register_tagged(dest, TRUE.into_raw_bits(), RegTag::Bool)
}

/// LoadFalse: write false into register.
fn _op_loadfalse(vm: &mut super::VM) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    vm.set_register_tagged(dest, FALSE.into_raw_bits(), RegTag::Bool)
}

/// Mov: copy value between registers.
///
/// ZeroUnbox: reads with `register_tagged` and writes with `set_register_tagged`
/// to preserve tag information across the copy.
fn _op_mov(vm: &mut super::VM) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    let src = vm.read_u16()?;
    let (raw, tag) = vm.register_tagged(src);
    vm.set_register_tagged(dest, raw, tag)
}

// ---- Arithmetic Operations (ZeroUnbox Fast Path) ----

/// ZeroUnbox arithmetic handler macro.
///
/// Generates a handler that:
/// 1. Reads operands as `(u64, RegTag)` pairs
/// 2. Fast path: f64+f64 → single `addsd`/`subsd`/... instruction
/// 3. Fast path: Smi+Smi → pure bitwise arithmetic with overflow detection
/// 4. Cold path: delegates to the appropriate `generic_*_slow` function
///
/// # Parameters
/// - `$name`: handler function name (e.g., `_op_add`)
/// - `$smi_fn`: Smi fast-path function (e.g., `zero_unbox::smi_add`)
/// - `$f64_op`: f64 computation expression using `$fa` and `$fb` (e.g., `$fa + $fb`)
/// - `$slow_fn`: cold-path function (e.g., `zero_unbox::generic_add_slow`)
macro_rules! arith_handler {
    ($name:ident, $smi_fn:expr, $f64_op:expr, $slow_fn:expr) => {
        fn $name(vm: &mut super::VM) -> Result<(), NuzoError> {
            let dest = vm.read_u16()?;
            let left = vm.read_u16()?;
            let right = vm.read_u16()?;

            let (a, tag_a) = vm.register_tagged(left);
            let (b, tag_b) = vm.register_tagged(right);

            let (result, tag_r) = if zero_unbox::likely(tag_a.is_f64_like() && tag_b.is_f64_like())
            {
                // f64+f64 fast path: single FP instruction
                let fa = zero_unbox::to_f64(a);
                let fb = zero_unbox::to_f64(b);
                let fr = $f64_op(fa, fb);
                (zero_unbox::from_f64(fr), RegTag::Float)
            } else if tag_a == RegTag::Smi && tag_b == RegTag::Smi {
                // Smi+Smi fast path: pure bitwise arithmetic
                match $smi_fn(a, b) {
                    Some(r) => (r, RegTag::Smi),
                    None => {
                        // Smi overflow → float fallback
                        let fa = zero_unbox::smi_to_i64(a) as f64;
                        let fb = zero_unbox::smi_to_i64(b) as f64;
                        let fr = $f64_op(fa, fb);
                        zero_unbox::smi_result_or_float(fr)
                    }
                }
            } else {
                // Cold path: string concat, type errors, mixed types
                $slow_fn(a, b)?
            };

            vm.set_register_tagged(dest, result, tag_r)
        }
    };
}

/// ZeroUnbox comparison handler macro.
///
/// Generates a complete comparison-opcode handler function that delegates
/// to a shared comparison helper (`_equality_comparison` or
/// `_binary_comparison`). Eliminates the signature boilerplate shared by
/// the 6 comparison opcodes (`_op_eq`, `_op_neq`, `_op_lt`, `_op_gt`,
/// `_op_le`, `_op_ge`), which all conform to the same
/// `fn(&mut VM) -> Result<(), NuzoError>` signature and immediately forward
/// to a helper.
///
/// # Parameters
/// - `$name`: handler function name (e.g., `_op_eq`)
/// - `$helper`: helper function to delegate to (`_equality_comparison` or
///   `_binary_comparison`)
/// - `$args`: trailing helper arguments — `EqualityMode::*` for equality,
///   or `|a, b| a < b` style closure for ordering. The macro injects `vm`
///   as the first argument to the helper, so callers only supply the tail.
///
/// # NaN & Type Semantics
///
/// NaN handling (IEEE 754) and Smi-vs-f64 dispatch live inside the shared
/// helpers (`_equality_comparison`, `_binary_comparison`), so this macro is
/// a thin signature-elimination wrapper. Behavior is identical to the prior
/// handwritten handlers — the macro exists purely to remove the 6×
/// `fn _op_xxx(vm: &mut super::VM) -> Result<(), NuzoError> { ... }`
/// duplication, mirroring the role of `arith_handler!` for arithmetic ops.
///
/// # Macro Hygiene Note
///
/// `vm` is injected by the macro itself (not by the caller's expression)
/// to bypass Rust's macro hygiene rules: identifiers defined inside a
/// macro body are not visible to token trees passed in as arguments.
///
/// # Example
///
/// ```rust,ignore
/// cmp_handler!(_op_eq, _equality_comparison, EqualityMode::Equal);
/// cmp_handler!(_op_lt, _binary_comparison, |a, b| a < b);
/// ```
macro_rules! cmp_handler {
    ($name:ident, $helper:ident, $($args:tt)*) => {
        fn $name(vm: &mut super::VM) -> Result<(), NuzoError> {
            $helper(vm, $($args)*)
        }
    };
}

arith_handler!(
    _op_add,
    zero_unbox::smi_add,
    |fa: f64, fb: f64| fa + fb,
    zero_unbox::generic_add_slow
);
arith_handler!(
    _op_sub,
    zero_unbox::smi_sub,
    |fa: f64, fb: f64| fa - fb,
    zero_unbox::generic_sub_slow
);
arith_handler!(
    _op_mul,
    zero_unbox::smi_mul,
    |fa: f64, fb: f64| fa * fb,
    zero_unbox::generic_mul_slow
);
arith_handler!(
    _op_pow,
    zero_unbox::smi_pow,
    |fa: f64, fb: f64| fa.powf(fb),
    zero_unbox::generic_pow_slow
);

/// 除法类运算（div/rem/mod）的通用实现，带除零保护（ZeroUnbox 快路径）。
///
/// 三者结构完全一致，仅快路径运算符与慢路径函数不同，故提取为泛型辅助。
/// `#[inline(always)]` 确保闭包与慢路径函数指针在调用点内联，零抽象开销。
#[inline(always)]
fn _binary_div_like_op(
    vm: &mut super::VM,
    fast_op: impl Fn(f64, f64) -> f64,
    slow_fn: fn(u64, u64) -> Result<(u64, RegTag), NuzoError>,
) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    let left = vm.read_u16()?;
    let right = vm.read_u16()?;

    let (a, tag_a) = vm.register_tagged(left);
    let (b, tag_b) = vm.register_tagged(right);

    let (result, tag_r) = if zero_unbox::likely(tag_a.is_f64_like() && tag_b.is_f64_like()) {
        let fb = zero_unbox::to_f64(b);
        if zero_unbox::unlikely(fb == 0.0) {
            return Err(vm.error_with_source_location(NuzoError::division_by_zero()));
        }
        let fa = zero_unbox::to_f64(a);
        let fr = fast_op(fa, fb);
        (zero_unbox::from_f64(fr), RegTag::Float)
    } else if tag_a == RegTag::Smi && tag_b == RegTag::Smi {
        // Smi 除法类运算总是降级为 float（极少产生整数）
        let fb = zero_unbox::smi_to_i64(b) as f64;
        if zero_unbox::unlikely(fb == 0.0) {
            return Err(vm.error_with_source_location(NuzoError::division_by_zero()));
        }
        let fa = zero_unbox::smi_to_i64(a) as f64;
        let fr = fast_op(fa, fb);
        zero_unbox::smi_result_or_float(fr)
    } else {
        slow_fn(a, b)?
    };

    vm.set_register_tagged(dest, result, tag_r)
}

/// Division handler with divide-by-zero protection (ZeroUnbox fast path).
///
/// Unlike add/sub/mul, division must check for zero divisor before
/// executing the FP instruction. The check is placed on the f64 fast path
/// because Smi division always degrades to float.
fn _op_div(vm: &mut super::VM) -> Result<(), NuzoError> {
    _binary_div_like_op(vm, |a, b| a / b, zero_unbox::generic_div_slow)
}

/// Remainder handler with divide-by-zero protection (ZeroUnbox fast path).
fn _op_rem(vm: &mut super::VM) -> Result<(), NuzoError> {
    _binary_div_like_op(vm, |a, b| a % b, zero_unbox::generic_rem_slow)
}

/// Modulo handler with divide-by-zero protection (ZeroUnbox fast path).
fn _op_mod(vm: &mut super::VM) -> Result<(), NuzoError> {
    _binary_div_like_op(vm, |a, b| a % b, zero_unbox::generic_mod_slow)
}

/// StringBuild: concatenate strings from contiguous register range.
///
/// Concatenates `count` string values from registers `[start..start+count]`
/// into a single string and stores it in `dest`.
///
/// # Zero-Allocation Path
///
/// Unlike repeated `Add` operations, StringBuild:
/// 1. Pre-computes total length by scanning all source strings
/// 2. Allocates a single buffer with exact capacity
/// 3. Copies string data in one pass without intermediate allocations
///
/// # Operands
///
/// - `dest`: u16 - destination register
/// - `start`: u16 - first register of the range
/// - `count`: u16 - number of registers to concatenate
fn _op_stringbuild(vm: &mut super::VM) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    let start = vm.read_u16()?;
    let count = vm.read_u16()?;

    // Edge case: empty concatenation → empty string
    if count == 0 {
        return vm.set_register(dest, Value::from_string(""));
    }

    // Phase 1: Collect string contents and compute total length
    let mut strings = Vec::with_capacity(count as usize);
    let mut total_len = 0usize;

    for i in 0..count {
        let reg = start + i;
        let value = vm.register(reg)?;

        // StringBuild 支持混合类型：非字符串值通过 concat_repr() 转为字符串表示。
        // 这与 Nuzo 的 `+` 运算符语义一致：当操作数包含非数字时回退到字符串拼接。
        let s = value.concat_repr();
        total_len += s.len();
        strings.push(s);
    }

    // Phase 2: Single allocation + batch copy
    let mut result = String::with_capacity(total_len);
    for s in &strings {
        result.push_str(s);
    }

    // Phase 3: Intern result and write to destination register
    let result_value = Value::from_string(&result);
    vm.set_register(dest, result_value)
}

/// Neg: unary negation (ZeroUnbox fast path).
fn _op_neg(vm: &mut super::VM) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    let src = vm.read_u16()?;

    let (a, tag_a) = vm.register_tagged(src);

    let (result, tag_r) = if tag_a.is_f64_like() {
        // Float/Nan fast path: negate f64 bits
        let fa = zero_unbox::to_f64(a);
        let fr = -fa;
        (zero_unbox::from_f64(fr), RegTag::Float)
    } else if tag_a == RegTag::Smi {
        // Smi fast path: negate and check overflow
        let iv = zero_unbox::smi_to_i64(a);
        match Value::try_from_smi(-iv) {
            Some(v) => (v.into_raw_bits(), RegTag::Smi),
            None => {
                // Overflow → float
                let fr = -(iv as f64);
                (zero_unbox::from_f64(fr), RegTag::Float)
            }
        }
    } else {
        // Cold path: type error for non-numbers
        zero_unbox::generic_neg_slow(a)?
    };

    vm.set_register_tagged(dest, result, tag_r)
}

// ---- Comparison Operations (ZeroUnbox Fast Path) ----

/// Equality comparison mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EqualityMode {
    Equal,
    NotEqual,
}

// Equality / inequality handlers — generated by `cmp_handler!`.
// Delegates to `_equality_comparison` with the appropriate mode; NaN and
// mixed-type semantics live in the shared helper.
cmp_handler!(_op_eq, _equality_comparison, EqualityMode::Equal);
cmp_handler!(_op_neq, _equality_comparison, EqualityMode::NotEqual);

/// Shared equality-comparison logic with ZeroUnbox fast paths.
///
/// Fast paths:
/// - Same tag + number types: compare raw bits or f64 values
/// - Smi+Smi: `a == b` (same encoding = same value)
/// - f64+f64: `f64::from_bits(a) == f64::from_bits(b)`
///
/// Cold path: collection contains, string comparison, etc.
#[inline(always)]
fn _equality_comparison(vm: &mut super::VM, mode: EqualityMode) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    let left = vm.read_u16()?;
    let right = vm.read_u16()?;

    let (a, tag_a) = vm.register_tagged(left);
    let (b, tag_b) = vm.register_tagged(right);

    let result = if tag_a == tag_b {
        // Same tag: fast comparison
        match tag_a {
            RegTag::Smi => a == b,
            RegTag::Float | RegTag::Nan => {
                let fa = zero_unbox::to_f64(a);
                let fb = zero_unbox::to_f64(b);
                fa == fb
            }
            RegTag::Bool => a == b,
            RegTag::Nil => true,
            _ => {
                // Heap types: use Value comparison
                // SAFETY: a and b are raw bits read from registers via register_tagged(),
                // which returns NaN-tagged bit patterns produced by Value::into_raw_bits().
                // Reconstructing via from_raw_bits is sound because the original bits
                // came from a valid Value encoding.
                let va = unsafe { Value::from_raw_bits(a) };
                let vb = unsafe { Value::from_raw_bits(b) };
                va.value_equals(&vb)
            }
        }
    } else if tag_a.is_number() && tag_b.is_number() {
        // Mixed number types (Smi vs Float): compare as f64
        let fa = if tag_a == RegTag::Smi {
            zero_unbox::smi_to_i64(a) as f64
        } else {
            zero_unbox::to_f64(a)
        };
        let fb = if tag_b == RegTag::Smi {
            zero_unbox::smi_to_i64(b) as f64
        } else {
            zero_unbox::to_f64(b)
        };
        fa == fb
    } else {
        // Cold path: collection contains, mixed types
        let (raw, _tag) = zero_unbox::generic_eq_slow(a, b);
        // SAFETY: raw is the bit pattern of a boolean Value (TRUE or FALSE)
        // produced by generic_eq_slow, which always returns a valid NaN-tagged bool.
        let eq_result = unsafe { Value::from_raw_bits(raw) }.as_bool();
        let final_result = match mode {
            EqualityMode::Equal => eq_result,
            EqualityMode::NotEqual => !eq_result,
        };
        return vm.set_register_tagged(
            dest,
            Value::from_bool(final_result).into_raw_bits(),
            RegTag::Bool,
        );
    };

    let final_result = match mode {
        EqualityMode::Equal => result,
        EqualityMode::NotEqual => !result,
    };
    vm.set_register_tagged(dest, Value::from_bool(final_result).into_raw_bits(), RegTag::Bool)
}

// Ordering comparison handlers — generated by `cmp_handler!`.
// Each delegates to `_binary_comparison` with the corresponding f64
// comparison closure; NaN (IEEE 754) and Smi-vs-f64 dispatch live in the
// shared helper.
cmp_handler!(_op_lt, _binary_comparison, |a, b| a < b);
cmp_handler!(_op_gt, _binary_comparison, |a, b| a > b);
cmp_handler!(_op_le, _binary_comparison, |a, b| a <= b);
cmp_handler!(_op_ge, _binary_comparison, |a, b| a >= b);

/// Shared binary-comparison logic with ZeroUnbox fast paths.
///
/// Fast paths:
/// - f64+f64: direct f64 comparison
/// - Smi+Smi: signed comparison via `smi_to_i64`
///
/// Cold path: type error for non-numeric operands.
#[inline(always)]
fn _binary_comparison<F>(vm: &mut super::VM, op: F) -> Result<(), NuzoError>
where
    F: Fn(f64, f64) -> bool,
{
    let dest = vm.read_u16()?;
    let left = vm.read_u16()?;
    let right = vm.read_u16()?;

    let (a, tag_a) = vm.register_tagged(left);
    let (b, tag_b) = vm.register_tagged(right);

    let result = if zero_unbox::likely(tag_a.is_f64_like() && tag_b.is_f64_like()) {
        // f64+f64 fast path
        let fa = zero_unbox::to_f64(a);
        let fb = zero_unbox::to_f64(b);
        op(fa, fb)
    } else if tag_a == RegTag::Smi && tag_b == RegTag::Smi {
        // Smi+Smi fast path: signed comparison
        let ia = zero_unbox::smi_to_i64(a);
        let ib = zero_unbox::smi_to_i64(b);
        op(ia as f64, ib as f64)
    } else if tag_a.is_number() && tag_b.is_number() {
        // Mixed number types (Smi vs Float)
        let fa = if tag_a == RegTag::Smi {
            zero_unbox::smi_to_i64(a) as f64
        } else {
            zero_unbox::to_f64(a)
        };
        let fb = if tag_b == RegTag::Smi {
            zero_unbox::smi_to_i64(b) as f64
        } else {
            zero_unbox::to_f64(b)
        };
        op(fa, fb)
    } else {
        // Cold path: type error
        return zero_unbox::generic_ord_slow(a, b, op)
            .and_then(|(raw, tag)| vm.set_register_tagged(dest, raw, tag));
    };

    vm.set_register_tagged(dest, Value::from_bool(result).into_raw_bits(), RegTag::Bool)
}

/// Logical NOT (ZeroUnbox fast path).
///
/// Uses `is_truthy_raw` for zero-overhead truthiness check on raw u64 bits.
fn _op_not(vm: &mut super::VM) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    let src = vm.read_u16()?;

    let (a, tag_a) = vm.register_tagged(src);

    let (result, tag_r) = if zero_unbox::likely(
        tag_a == RegTag::Bool
            || tag_a == RegTag::Nil
            || tag_a == RegTag::Smi
            || tag_a == RegTag::Float,
    ) {
        // Fast path: truthiness check on raw bits
        let truthy = zero_unbox::is_truthy_raw(a);
        (Value::from_bool(!truthy).into_raw_bits(), RegTag::Bool)
    } else {
        // Cold path: handles heap objects, strings, etc.
        zero_unbox::generic_not_slow(a)
    };

    vm.set_register_tagged(dest, result, tag_r)
}

// ---- Control Flow (custom op_*) ----

fn _op_jmp(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_jmp()
}
fn _op_test(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_test()
}

// ---- Object Operations (custom op_*) ----

fn _op_getprop(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_get_prop()
}
fn _op_setprop(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_set_prop()
}
fn _op_getindex(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_get_index()
}
fn _op_setindex(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_set_index()
}
fn _op_setindexmut(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_set_index_mut()
}

// ---- Function Operations ----

fn _op_call(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_call()
}
fn _op_return(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_return()
}

/// Closure: load prototype from constant pool into register.
fn _op_closure(vm: &mut super::VM) -> Result<(), NuzoError> {
    let dest = vm.read_u16()?;
    let proto_idx = vm.read_u16()? as usize;
    let chunk = vm.current_chunk()?;
    if zero_unbox::unlikely(proto_idx >= chunk.constants().len()) {
        return Err(NuzoError::internal(
            nuzo_values::InternalError::ConstantOutOfBounds {
                index: proto_idx,
                pool_size: chunk.constants().len(),
            },
            None,
        ));
    }
    let proto_val = chunk.constants()[proto_idx];
    let raw = proto_val.into_raw_bits();
    let tag = TypedRegFile::infer_tag(raw);
    vm.set_register_tagged(dest, raw, tag)
}

// ---- Built-in Operations ----

/// Print: output register value to stdout.
fn _op_print(vm: &mut super::VM) -> Result<(), NuzoError> {
    let reg = vm.read_u16()?;
    let value = vm.register(reg)?;
    let output_str = value.concat_repr();
    // Access the output_capture field on VM -- same logic as dispatch_handler!(print_value)
    // We need to reach into the VM's output_capture field. Since we're in a submodule,
    // use the same approach as the macro does.
    if let Some(ref capture) = vm.output_capture {
        std::sync::Mutex::lock(capture).unwrap_or_else(|e| e.into_inner()).push(output_str);
    } else {
        println!("{}", output_str);
    }
    Ok(())
}

// ---- Termination / Heap Objects / Captures / Globals / Range / Length (custom op_*) ----

fn _op_halt(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_halt()
}
fn _op_arraynew(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_array_new()
}
fn _op_capture(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_capture()
}
fn _op_getcaptured(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_get_captured()
}
fn _op_setcaptured(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_set_captured()
}
fn _op_getglobal(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_get_global()
}
fn _op_setglobal(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_set_global()
}
fn _op_getglobalcached(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_get_global_cached()
}
fn _op_rangenew(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_range_new()
}
fn _op_len(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_len()
}

/// SliceChainNew: 创建空切片链
fn _op_slicechainnew(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_slicechain_new()
}

/// SliceChainAppend: 追加到切片链
fn _op_slicechainappend(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_slicechain_append()
}

/// SliceChainFinish: 完成切片链
fn _op_slicechainfinish(vm: &mut super::VM) -> Result<(), NuzoError> {
    vm.op_slicechain_finish()
}

// ---- 异常处理操作码 (M1 Phase 4 实现) ----

/// TryStart - 标记 try 块开始，压入异常帧到异常栈
///
/// # 操作数
///
/// - `catch_offset`: i16 - 相对当前 IP 的 catch 块偏移量（有符号）
/// - `exc_reg`: u16 - 存放异常值的寄存器编号（catch 绑定变量）
///
/// # 行为
///
/// 1. 读取 catch_offset 和 exc_reg
/// 2. 计算 catch 的绝对地址: `catch_ip = current_ip + catch_offset`
/// 3. 压入 ExceptionFrame 到 vm.exception_stack
///
/// # 字节码布局
///
/// ```text
/// [TryStart] [catch_offset: i16] [exc_reg: u16]
/// ```
fn _op_trystart(vm: &mut super::VM) -> Result<(), NuzoError> {
    let catch_offset = vm.read_i16()? as i64;
    let exc_reg = vm.read_u16()?;

    // 计算 catch 的绝对 IP（read_i16/read_u16 已推进 IP，
    // 所以 current ip 就是下一条指令的地址）
    let current = vm.ip as i64;
    let catch_ip = (current + catch_offset) as usize;

    // 压入异常帧到异常栈
    vm.exception_stack.push(super::ExceptionFrame {
        catch_ip,
        exc_reg,
        base_stack_size: vm.stack_size(),
    });

    Ok(())
}

/// TryEnd - 标记 try 块结束（正常路径），弹出异常栈顶
///
/// # 行为
///
/// 弹出 exception_stack 栈顶。try 块正常完成，不需要跳转到 catch。
///
/// # 错误条件
///
/// 如果异常栈为空（没有匹配的 TryStart），返回断言失败错误。
fn _op_tryend(vm: &mut super::VM) -> Result<(), NuzoError> {
    if zero_unbox::unlikely(vm.exception_stack.pop().is_none()) {
        return Err(NuzoError::assert_failed("TryEnd: 异常栈为空 — 没有匹配的 TryStart"));
    }
    Ok(())
}

/// Out - 抛出异常，跳转到最近 catch 入口
///
/// # 操作数
///
/// - `value_reg`: u16 - 包含异常值的寄存器编号
///
/// # 异常传播流程
///
/// ```text
/// 1. Out 指令执行
///    ↓
/// 2. 从 value_reg 取出异常值（Value 类型）
///    ↓
/// 3. 检查 exception_stack 是否为空
///    ├── 空 → 返回 UncaughtException 错误（程序终止）
///    └── 非空 → 继续
///    ↓
/// 4. 取出栈顶 ExceptionFrame（不弹出！栈帧保留用于嵌套 try）
///    ↓
/// 5. 将异常值写入 frame.exc_reg（catch 绑定变量）
///    ↓
/// 6. 设置 vm.pending_exception = Some(exception_value)
///    ↓
/// 7. 修改 IP = frame.catch_ip（跳转到 catch 块入口）
///    ↓
/// 8. 正常返回 Ok(()) —— VM 主循环继续从 catch_ip 执行
/// ```
fn _op_out(vm: &mut super::VM) -> Result<(), NuzoError> {
    let value_reg = vm.read_u16()?;

    // 从寄存器读取异常值
    let exception_value = vm.register(value_reg)?;

    // 查找最近的 try 帧（栈顶）
    if let Some(frame) = vm.exception_stack.last() {
        // 提取所需字段（Copy 类型，立即释放不可变借用）
        let catch_ip = frame.catch_ip;
        let exc_reg = frame.exc_reg;

        // 将异常值存入 catch 绑定寄存器
        vm.set_register(exc_reg, exception_value)?;

        // 设置待处理的异常值（M2 keep 块使用）
        vm.set_pending_exception(Some(exception_value));

        // 跳转到 catch 入口（设置 IP）
        vm.set_ip(catch_ip);

        Ok(())
    } else {
        // 未捕获的异常：没有匹配的 try 帧
        Err(vm.error_with_source_location(NuzoError::assert_failed(format!(
            "未捕获的异常: {}",
            exception_value.concat_repr()
        ))))
    }
}

// ============================================================================
// LSRA Spill Handlers
// ============================================================================
//
// SpillLoad/SpillStore 操作码已定义于 Opcode 枚举（code=54/55）。
// 编译器后端在寄存器分配压力过大时发射这些指令，将寄存器值
// 临时溢出到栈帧的 spill_stack 上，并在需要时重新加载。

/// SpillLoad handler: 从 spill_stack[slot] 加载到 R[dst]。
///
/// 操作数: dst (u16), slot (u16)
/// 语义: R[dst] = spill_stack[slot]（若 slot 越界则返回 nil）
///
/// SCHF v6 Phase 3：读取路径切换到 `frame_data` 切片（spec 4.4 spill_get）。
/// 旧路径 `frames.back().spill_stack.get(slot)` 已废弃，spill 槽物理位置改为
/// `frame_data.data[base + locals_count + slot]`。
fn _op_spillload(vm: &mut super::VM) -> Result<(), NuzoError> {
    let dst = vm.read_u16()?;
    let slot = vm.read_u16()?;
    let val = vm.spill_get(slot);
    vm.set_register(dst, val)?;
    Ok(())
}

/// SpillStore handler: 从 R[src] 存储到 spill_stack[slot]。
///
/// 操作数: src (u16), slot (u16)
/// 语义: spill_stack[slot] = R[src]（若 slot 越界则自动扩容）
///
/// SCHF v6 Phase 3：写入路径切换到 `frame_data` 切片（spec 4.4 spill_set）。
/// 移除自动扩容：spill 槽数量由 CIP 编译期计算，运行期不会越界。
fn _op_spillstore(vm: &mut super::VM) -> Result<(), NuzoError> {
    let src = vm.read_u16()?;
    let slot = vm.read_u16()?;
    let val = vm.register(src)?;
    vm.spill_set(slot, val);
    Ok(())
}

/// OP_INIT_MODULE handler：lazy import 模块的运行期初始化。
///
/// # 操作数（共 5 字节：opcode + 2 × u16）
/// - `module_idx`: u16 - 常量池中模块路径字符串的索引
/// - `init_flag_slot`: u16 - 历史遗留操作数，仍读取以推进 IP（保持字节码格式
///   向后兼容），但实际不再使用。init flag 现按模块路径派生的唯一名字注册。
///
/// # 执行语义（spec §3.4 Lazy Import 运行时机制）
/// 1. 读取操作数（已自动推进 IP 至下一条指令）
/// 2. 从常量池取出模块路径字符串，派生 init flag 名 `__init_flag__<path>`
/// 3. 检查该名字的全局变量 — 若为 `true` 表示已初始化，直接返回（no-op）
/// 4. 先设置 init flag 为 `true`（幂等语义：避免模块错误后重复执行副作用）
/// 5. 从 `cx.module_cache` 取已编译的模块 Chunk
///    - 未找到 → 返回 `InternalError::ModuleNotLoaded { path }`
/// 6. 通过帧切换（`push_frame_with_base`）切到模块 Chunk
///    - run_inner 主循环自然接管，执行模块顶层代码
///    - 模块的 `OP_RETURN` → `pop_frame` 自动恢复 caller 的 chunk 与 IP
///
/// # 设计决策：先置 init flag 再执行
/// spec §3.4 中先 `run_chunk` 后置 flag 的语义假设 `run_chunk` 是同步返回。
/// 但 VM 不允许 `run_inner` 重入，必须用帧切换模式，导致"模块执行完成"这一事件
/// 无法在 `_op_initmodule` 内部观测到（控制权交还主循环后才执行）。
/// 因此选择"先置 flag 再切帧"策略：
/// - 模块执行成功 → flag 已为 true，下次 OP_INIT_MODULE 命中分支 3 跳过
/// - 模块执行失败 → 错误沿帧栈传播，flag 已为 true，避免重复执行副作用
/// - 幂等性：模块顶层的 `print` 等副作用不会被重复触发
///
/// # init flag 存储方式（T2.2b 回归修复）
/// 旧实现用 `global_scope.set(init_flag_slot, TRUE)` 按**数字索引**写入，
/// 会覆盖该 slot 上的现有全局变量（如 builtin `print` 在 slot 0），导致
/// `print(...)` 取到 TRUE(bool) 触发 TypeMismatch。
/// 现改用 `define_global("__init_flag__<path>", TRUE)` 按**名字**注册，
/// 不覆盖任何现有全局变量。
fn _op_initmodule(vm: &mut super::VM) -> Result<(), NuzoError> {
    // 1. 读取操作数（read_u16 已推进 IP，此时 self.ip 指向下一条指令）
    let module_idx = vm.read_u16()? as usize;
    // init_flag_slot 仍读取以推进 IP（保持字节码格式向后兼容），但实际不再使用：
    // 旧实现按数字 slot 写入 global_scope 会覆盖 slot 0 上的现有全局变量（如
    // builtin `print`），导致后续 `print(...)` 取到 TRUE(bool) 触发 TypeMismatch。
    // 现改用模块路径派生的唯一名字注册 init flag（见下方步骤 3/4）。
    let _init_flag_slot = vm.read_u16()? as usize;

    // 2. 取模块路径字符串（从常量池）— 提前到检查之前，用于派生 init flag 名
    let module_path = vm.get_module_path_from_constant(module_idx)?;
    let flag_name = format!("__init_flag__{}", module_path);

    // 3. 检查 init flag — 已为 true 则跳过（幂等性）
    //    Value 是 u64 包装的 tagged value，无 enum 变体；通过 `== TRUE` 判断
    //    （TRUE/FALSE 已在 dispatch_table.rs 顶部 use nuzo_values::{TRUE, FALSE, ...}）
    if let Some(idx) = vm.resolve_global(&flag_name)
        && let Some(val) = vm.get_global(idx)
        && val == TRUE
    {
        return Ok(());
    }

    // 4. 先置 init flag = true（幂等语义，详见函数文档）
    //    用 define_global 按名字注册，不会覆盖任何现有全局变量。
    vm.define_global(&flag_name, Value::from_bool(true));

    // 5. 从 module_cache 取已编译的模块 Chunk
    //    未找到 → 返回 ModuleNotLoaded 诊断
    let module_chunk = vm.get_module_chunk(&module_path)?;

    // 6. 帧切换执行模块顶层（不重入 run_inner）
    vm.execute_module_toplevel(module_chunk)?;

    Ok(())
}

// ============================================================================
// Dispatch Table -- 自动生成（由 nuzo_proc::define_dispatch_auto! 生成）
// ============================================================================
//
// STITCH: 新增 Opcode 时只需在 nuzo_bytecode/src/opcode.rs 的
// `Instruction` 枚举上添加变体（带 `#[opcode_meta(...)]`）。
// `#[derive(OpcodeSync)]` 会自动生成 `with_every_dispatch_opcode!` 宏，
// 本 dispatch 表通过 callback 模式自动同步，无需手动维护 opcode 列表。
//
// 例外：如果某个 handler 不符合 `_op_{snake_case}` 命名约定，
// 需要修改 `build_dispatch_table` callback 宏以支持 `Op => handler` 语法。

/// Callback 宏：接收 opcode 列表，展开为 `define_dispatch_auto!` 调用。
///
/// 通过 `with_every_dispatch_opcode!(build_dispatch_table)` 调用，
/// 实现 opcode 列表的自动同步。
macro_rules! build_dispatch_table {
    ($($op:ident),* $(,)?) => {
        nuzo_proc::define_dispatch_auto! {
            $($op),*
        }
    };
}

nuzo_bytecode::with_every_dispatch_opcode!(build_dispatch_table);

// ============================================================================
// Superinstruction Fused Handlers (融合指令处理器)
// ============================================================================
//
// 设计目标：将高频 Opcode 相邻对合并为单次函数调用，消除重复的 dispatch 开销
//
// 性能优势：
// 1. 消除第二次 match/jump table 分发（节省 ~2-5 CPU 周期）
// 2. 避免中间结果的寄存器写回+重读（节省 ~1-3 CPU 周期 + 缓存压力）
// 3. 减少函数调用栈深度（提升 I-Cache 命中率）
//
// 使用场景：
// - execute_hot_trace_batch() 中的热路径融合执行
// - 未来可扩展为编译期自动生成的 superinstruction 表
//
// 命名约定：_op_{opcode1}_{opcode2} (snake_case, 按执行顺序)
// 签名约束：fn _op_xxx(vm: &mut super::VM) -> Result<(), NuzoError>
// 内联标注：#[inline(always)] (强制内联到调用点，消除函数调用开销)

/// Fused Handler: LoadK + BinaryOp → 单次调用完成常量加载与二元运算
///
/// 泛型参数化设计：`_op_loadk_add` / `_op_loadk_mul` 结构完全一致，
/// 仅 `op_fn` / `smi_fn` / `slow_fn` 不同，故提取为泛型辅助（与
/// `_op_mov_binaryop` 对称）。`#[inline(always)]` 确保闭包与函数指针
/// 在调用点内联，零抽象开销。
///
/// # 字节码模式
/// ```text
/// LoadK    dest, const_idx    ; 加载常量到 dest
/// BinaryOp dest, src_a, src_b ; dest = dest op src_b  (dest 被复用)
/// ```
///
/// # 融合优化点
/// 1. **跳过 LoadK 的寄存器写回**：常量值直接参与运算，无需写入 dest 再读出
/// 2. **单次 dispatch**：避免第二次 opcode 分发开销
/// 3. **ZeroUnbox 快速路径**：直接走 f64+f64 或 Smi+Smi 热路径
///
/// # 操作数读取顺序（5 个 u16）
/// - [0] dest: LoadK 目标寄存器（也是 BinaryOp 的目标/左操作数）
/// - [1] const_idx: LoadK 常量池索引（融合后跳过实际加载，仅移动 IP）
/// - [2] binop_dest: BinaryOp 目标寄存器（应等于 dest，否则语义错误）
/// - [3] src_a: BinaryOp 左操作数寄存器
/// - [4] src_b: BinaryOp 右操作数寄存器
///
/// # Type Parameters
/// - `OpFn`: f64 运算函数 `(f64, f64) -> f64`
/// - `SmiFn`: Smi 运算函数 `(u64, u64) -> Option<u64>`
/// - `SlowFn`: 冷路径函数 `(u64, u64) -> Result<(u64, RegTag), NuzoError>`
///
/// # Performance
/// - 时间复杂度: O(1)（与独立两条指令相同，但常数因子更小）
/// - 内存访问: 2 次 register read + 1 次 register write（vs. 独立 3 + 2）
/// - 分支预测: 1 次 tag 匹配（vs. 独立 2 次）
#[inline(always)]
pub(super) fn _op_loadk_arith<
    OpFn: Fn(f64, f64) -> f64,
    SmiFn: Fn(u64, u64) -> Option<u64>,
    SlowFn: Fn(u64, u64) -> Result<(u64, RegTag), NuzoError>,
>(
    vm: &mut super::VM,
    op_fn: OpFn,
    smi_fn: SmiFn,
    slow_fn: SlowFn,
) -> Result<(), NuzoError> {
    // 读取 LoadK 操作数（dest 和 const_idx）
    let dest = vm.read_u16()?;
    let _const_idx = vm.read_u16()?; // 跳过常量池查找（融合优化核心）

    // 读取 BinaryOp 操作数
    let _binop_dest = vm.read_u16()?; // 应等于 dest（防御性编程可添加断言）
    let src_a = vm.read_u16()?;
    let src_b = vm.read_u16()?;

    // 从常量池加载值（LoadK 的核心逻辑）
    let chunk = vm.current_chunk()?;
    let const_idx = _const_idx as usize;
    if zero_unbox::unlikely(const_idx >= chunk.constants().len()) {
        return Err(NuzoError::internal(
            nuzo_values::InternalError::ConstantOutOfBounds {
                index: const_idx,
                pool_size: chunk.constants().len(),
            },
            None,
        ));
    }
    let const_val = chunk.constants()[const_idx];

    // ZeroUnbox Pipeline: 获取操作数的 (raw, tag) 对
    let (a_raw, tag_a) = if tag::is_number(const_val.into_raw_bits()) {
        // 常量是数字：直接使用其 raw bits 和推断的 tag
        (const_val.into_raw_bits(), TypedRegFile::infer_tag(const_val.into_raw_bits()))
    } else {
        // 常量不是数字：走慢路径（字符串拼接等）
        let (result, tag) = slow_fn(const_val.into_raw_bits(), vm.register_tagged(src_a).0)?;
        return vm.set_register_tagged(dest, result, tag);
    };

    let (b_raw, tag_b) = vm.register_tagged(src_b);

    // 类型分支：选择最优计算路径（复用 arith_handler! 宏的逻辑）
    let (result, tag_r) = if zero_unbox::likely(tag_a.is_f64_like() && tag_b.is_f64_like()) {
        // f64 + f64 快速路径：单条 FP 指令
        let fa = zero_unbox::to_f64(a_raw);
        let fb = zero_unbox::to_f64(b_raw);
        let fr = op_fn(fa, fb);
        (zero_unbox::from_f64(fr), RegTag::Float)
    } else if tag_a == RegTag::Smi && tag_b == RegTag::Smi {
        // Smi + Smi 快速路径：纯位运算（无 FP 开销）
        match smi_fn(a_raw, b_raw) {
            Some(r) => (r, RegTag::Smi),
            None => {
                // Smi 溢出 → 自动提升为 Float
                let fa = zero_unbox::smi_to_i64(a_raw) as f64;
                let fb = zero_unbox::smi_to_i64(b_raw) as f64;
                zero_unbox::smi_result_or_float(op_fn(fa, fb))
            }
        }
    } else {
        // 冷路径：混合类型、字符串拼接、类型错误
        slow_fn(a_raw, b_raw)?
    };

    // 写入结果（仅一次寄存器写回）
    vm.set_register_tagged(dest, result, tag_r)
}

/// Fused Handler: GetLocal + Add → 单次调用完成局部变量加载与加法
///
/// # 字节码模式
/// ```text
/// GetLocal dest, slot_idx  ; 从栈帧槽位加载局部变量到 dest
/// Add      dest, src_a, src_b ; dest = dest + src_b  (dest 被复用)
/// ```
///
/// # 融合优化点
/// 1. **跳过 GetLocal 的寄存器写回**：局部变量值直接用于加法运算
/// 2. **减少一次 tag 推断**：GetLocal 已知源 tag，可直接传递给 Add
/// 3. **典型场景**：闭包捕获变量的算术运算（如 `captured_x + 1`）
///
/// # 操作数读取顺序（5 个 u16）
/// - [0] dest: GetLocal 目标寄存器
/// - [1] slot_idx: 栈帧槽位索引
/// - [2] add_dest: Add 目标寄存器
/// - [3] src_a: Add 左操作数
/// - [4] src_b: Add 右操作数
#[inline(always)]
pub(super) fn _op_getlocal_add(vm: &mut super::VM) -> Result<(), NuzoError> {
    // 读取 GetLocal 操作数
    let dest = vm.read_u16()?;
    let slot_idx = vm.read_u16()?;

    // 读取 Add 操作数
    let _add_dest = vm.read_u16()?;
    let _src_a = vm.read_u16()?; // Add 左操作数（融合后等于 dest，跳过）
    let src_b = vm.read_u16()?;

    // 从当前栈帧加载局部变量（GetLocal 核心逻辑）
    //
    // 注意：这里简化了 GetLocal 的完整逻辑（应包含 frame.base + slot_idx 计算），
    // 实际实现可能需要根据 VM 的寄存器文件布局调整。
    // 当前版本假设 slot_idx 是全局寄存器索引（与现有 GetLocal handler 一致）。
    let (local_raw, local_tag) = vm.register_tagged(slot_idx);

    // ZeroUnbox Pipeline: 使用局部变量作为左操作数
    let (b_raw, tag_b) = vm.register_tagged(src_b);

    // 加法类型分支（复用 arith_handler! 逻辑）
    let (result, tag_r) = if zero_unbox::likely(local_tag.is_f64_like() && tag_b.is_f64_like()) {
        let fa = zero_unbox::to_f64(local_raw);
        let fb = zero_unbox::to_f64(b_raw);
        let fr = fa + fb;
        (zero_unbox::from_f64(fr), RegTag::Float)
    } else if local_tag == RegTag::Smi && tag_b == RegTag::Smi {
        match zero_unbox::smi_add(local_raw, b_raw) {
            Some(r) => (r, RegTag::Smi),
            None => {
                let fa = zero_unbox::smi_to_i64(local_raw) as f64;
                let fb = zero_unbox::smi_to_i64(b_raw) as f64;
                zero_unbox::smi_result_or_float(fa + fb)
            }
        }
    } else {
        zero_unbox::generic_add_slow(local_raw, b_raw)?
    };

    vm.set_register_tagged(dest, result, tag_r)
}

/// Fused Handler: Mov + BinaryOp → 消除中间 Mov 寄存器拷贝
///
/// # 字节码模式
/// ```text
/// Mov  dest, src         ; dest = src (寄存器拷贝)
/// BinaryOp dest, dest, other ; dest = dest op other  (dest 被复用)
/// ```
///
/// # 融合优化点
/// 1. **完全消除 Mov**：直接从源寄存器读取值参与运算，跳过中间拷贝
/// 2. **减少寄存器压力**：dest 不被临时占用（编译器可更好地优化寄存器分配）
/// 3. **典型场景**：编译器生成的临时变量拷贝（如 `let t = x; t + y`）
///
/// # 通用性设计
/// 此 handler 支持所有基于 arith_handler! 宏的二元运算（Add/Sub/Mul/Div/Pow）。
/// 通过 `op_fn` 参数指定具体的运算逻辑，实现代码复用。
///
/// # 操作数读取顺序（5 个 u16）
/// - [0] mov_dest: Mov 目标寄存器
/// - [1] mov_src: Mov 源寄存器
/// - [2] binop_dest: BinaryOp 目标寄存器（应等于 mov_dest）
/// - [3] binop_left: BinaryOp 左操作数（应等于 mov_dest）
/// - [4] binop_right: BinaryOp 右操作数
///
/// # Type Parameters
/// - `OP_FN`: 运算函数指针类型 `(f64, f64) -> f64`
/// - `SMI_FN`: Smi 运算函数指针类型 `(u64, u64) -> Option<u64>`
/// - `SLOW_FN`: 冷路径函数指针类型 `(u64, u64) -> Result<(u64, RegTag), NuzoError>`
#[inline(always)]
pub(super) fn _op_mov_binaryop<
    OpFn: Fn(f64, f64) -> f64,
    SmiFn: Fn(u64, u64) -> Option<u64>,
    SlowFn: Fn(u64, u64) -> Result<(u64, RegTag), NuzoError>,
>(
    vm: &mut super::VM,
    op_fn: OpFn,
    smi_fn: SmiFn,
    slow_fn: SlowFn,
    needs_zero_check: bool,
) -> Result<(), NuzoError> {
    // 读取 Mov 操作数
    let _mov_dest = vm.read_u16()?;
    let mov_src = vm.read_u16()?;

    // 读取 BinaryOp 操作数
    let _binop_dest = vm.read_u16()?;
    let _binop_left = vm.read_u16()?; // 应等于 mov_dest（融合后跳过）
    let binop_right = vm.read_u16()?;

    // 直接从 mov_src 读取值（跳过 Mov 的寄存器写回）
    let (a_raw, tag_a) = vm.register_tagged(mov_src);
    let (b_raw, tag_b) = vm.register_tagged(binop_right);

    // 通用二元运算类型分支（参数化设计，支持 Add/Sub/Mul 等）
    //
    // 除法类运算（Div/Pow 中以除法形式参与的语义）必须对除数做零检查，
    // 与普通路径 `_binary_div_like_op` 保持一致；Add/Sub/Mul 传入
    // `needs_zero_check = false`，零开销。
    let (result, tag_r) = if zero_unbox::likely(tag_a.is_f64_like() && tag_b.is_f64_like()) {
        let fb = zero_unbox::to_f64(b_raw);
        if zero_unbox::unlikely(needs_zero_check && fb == 0.0) {
            return Err(vm.error_with_source_location(NuzoError::division_by_zero()));
        }
        let fa = zero_unbox::to_f64(a_raw);
        let fr = op_fn(fa, fb);
        (zero_unbox::from_f64(fr), RegTag::Float)
    } else if tag_a == RegTag::Smi && tag_b == RegTag::Smi {
        let fb = zero_unbox::smi_to_i64(b_raw) as f64;
        if zero_unbox::unlikely(needs_zero_check && fb == 0.0) {
            return Err(vm.error_with_source_location(NuzoError::division_by_zero()));
        }
        match smi_fn(a_raw, b_raw) {
            Some(r) => (r, RegTag::Smi),
            None => {
                let fa = zero_unbox::smi_to_i64(a_raw) as f64;
                zero_unbox::smi_result_or_float(op_fn(fa, fb))
            }
        }
    } else {
        slow_fn(a_raw, b_raw)?
    };

    // 写入结果到原始 Mov 目标寄存器
    vm.set_register_tagged(_mov_dest, result, tag_r)
}

/// Compile-time sanity: macro-generated count must match `nuzo_bytecode::INSTRUCTION_COUNT`.
///
/// If this fails, a new Opcode variant was added to `define_opcodes!` but not
/// to the `define_dispatch!` invocation above (or vice versa).
const _: () = assert!(
    INSTRUCTION_COUNT == nuzo_bytecode::INSTRUCTION_COUNT,
    "dispatch_table INSTRUCTION_COUNT does not match nuzo_bytecode::INSTRUCTION_COUNT \
     -- sync the define_dispatch! invocation with the Opcode enum"
);

// ============================================================================
// Public Dispatch Entry Point
// ============================================================================

/// Execute a single opcode via match-based direct dispatch.
///
/// Delegates to [`get_handler`] for the lookup, then invokes the handler.
/// The compiler optimizes the `match` into a jump table, yielding
/// performance equivalent to the previous array-indexed approach.
///
/// # Error Handling
///
/// Returns `Err(NuzoError)` if:
/// - The opcode has no mapped handler (should never happen for valid bytecode)
/// - The handler itself returns an error (type mismatch, out-of-bounds, etc.)
///
/// Cold path: no handler mapped for the given opcode (should never happen).
#[cold]
#[inline(never)]
fn no_handler_error(opcode: Opcode) -> NuzoError {
    NuzoError::internal(
        nuzo_values::InternalError::CompilerBug {
            message: format!("no handler mapped for opcode {:?}", opcode),
        },
        None,
    )
}

#[inline(always)]
pub fn dispatch_opcode_fast(vm: &mut super::VM, opcode: Opcode) -> Result<(), NuzoError> {
    let handler = get_handler(opcode).ok_or_else(|| no_handler_error(opcode))?;
    handler(vm)
}

// ============================================================================
// Tests
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::VM;

    #[test]
    fn test_dispatch_opcode_fast_halt() {
        // Halt opcode should be safe to dispatch on a fresh VM
        let mut vm = VM::new();
        let result = dispatch_opcode_fast(&mut vm, Opcode::Halt);
        // Halt may return Ok or a specific error; just ensure no panic
        let _ = result;
    }

    #[test]
    fn test_dispatch_opcode_fast_no_crash_on_nop() {
        let mut vm = VM::new();
        let result = dispatch_opcode_fast(&mut vm, Opcode::Halt);
        let _ = result;
    }
}
