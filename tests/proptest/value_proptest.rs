//! Value 算术属性测试 (Property-Based Tests)
//!
//! 对 NaN-tagged Value 的算术运算进行属性测试，覆盖以下属性：
//!
//! - **加法交换律**：a + b == b + a
//! - **加法结合律**：(a + b) + c == a + (b + c)（浮点容差内）
//! - **加法单位元**：a + 0 == a
//! - **减法自反性**：a - a == 0
//! - **乘法交换律**：a * b == b * a
//! - **乘法单位元**：a * 1 == a
//! - **乘法零元**：a * 0 == 0
//! - **除法逆元**：a / a == 1（a != 0）
//! - **取模定义**：a % b == a - (a / b) * b
//! - **幂零指数**：a ** 0 == 1
//! - **幂单位底**：1 ** a == 1
//! - **Smi 与 Float 一致性**：整数运算在两种编码下结果一致
//! - **布尔值不变性**：true/false 经任何运算不变（或返回错误）

use nuzo_core::Value;
use proptest::prelude::*;

fn smi_value() -> impl Strategy<Value = Value> {
    (-1_000_000i64..1_000_000).prop_map(Value::from_smi)
}

fn float_value() -> impl Strategy<Value = Value> {
    any::<f64>().prop_filter("filter non-finite", |v| v.is_finite()).prop_map(Value::from_number)
}

fn approx_eq(a: f64, b: f64) -> bool {
    if a == b {
        return true;
    }
    (a - b).abs() < 1e-6 * a.abs().max(b.abs()).max(1.0)
}

proptest! {
    #[test]
    fn prop_add_commutative_smi(a in -1_000_000i64..1_000_000, b in -1_000_000i64..1_000_000) {
        let va = Value::from_smi(a);
        let vb = Value::from_smi(b);
        let r1 = va.add(vb).unwrap();
        let r2 = vb.add(va).unwrap();
        prop_assert_eq!(r1.as_number(), r2.as_number());
    }

    #[test]
    fn prop_add_commutative_float(a in float_value(), b in float_value()) {
        let r1 = a.add(b).unwrap();
        let r2 = b.add(a).unwrap();
        prop_assert!(approx_eq(r1.as_number(), r2.as_number()));
    }

    #[test]
    fn prop_add_associative_smi(
        a in -10_000i64..10_000,
        b in -10_000i64..10_000,
        c in -10_000i64..10_000
    ) {
        let va = Value::from_smi(a);
        let vb = Value::from_smi(b);
        let vc = Value::from_smi(c);
        let r1 = va.add(vb).unwrap().add(vc).unwrap();
        let r2 = va.add(vb.add(vc).unwrap()).unwrap();
        prop_assert_eq!(r1.as_number(), r2.as_number());
    }

    #[test]
    fn prop_add_identity_smi(a in -1_000_000i64..1_000_000) {
        let va = Value::from_smi(a);
        let zero = Value::from_smi(0);
        let result = va.add(zero).unwrap();
        prop_assert_eq!(result.as_number(), a as f64);
    }

    #[test]
    fn prop_add_identity_float(a in float_value()) {
        let zero = Value::from_number(0.0);
        let result = a.add(zero).unwrap();
        prop_assert!(approx_eq(result.as_number(), a.as_number()));
    }

    #[test]
    fn prop_sub_self_zero_smi(a in -1_000_000i64..1_000_000) {
        let va = Value::from_smi(a);
        let result = va.sub(va).unwrap();
        prop_assert_eq!(result.as_number(), 0.0);
    }

    #[test]
    fn prop_sub_self_zero_float(a in float_value()) {
        let result = a.sub(a).unwrap();
        prop_assert!(approx_eq(result.as_number(), 0.0));
    }

    #[test]
    fn prop_mul_commutative_smi(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let va = Value::from_smi(a);
        let vb = Value::from_smi(b);
        let r1 = va.mul(vb).unwrap();
        let r2 = vb.mul(va).unwrap();
        prop_assert_eq!(r1.as_number(), r2.as_number());
    }

    #[test]
    fn prop_mul_identity_smi(a in -1_000_000i64..1_000_000) {
        let va = Value::from_smi(a);
        let one = Value::from_smi(1);
        let result = va.mul(one).unwrap();
        prop_assert_eq!(result.as_number(), a as f64);
    }

    #[test]
    fn prop_mul_zero_smi(a in -1_000_000i64..1_000_000) {
        let va = Value::from_smi(a);
        let zero = Value::from_smi(0);
        let result = va.mul(zero).unwrap();
        prop_assert_eq!(result.as_number(), 0.0);
    }

    #[test]
    fn prop_div_self_one_smi(a in 1i64..1_000_000) {
        let va = Value::from_smi(a);
        let result = va.div(va).unwrap();
        prop_assert_eq!(result.as_number(), 1.0);
    }

    #[test]
    fn prop_div_self_one_float(a in 0.001f64..1e15) {
        let va = Value::from_number(a);
        let result = va.div(va).unwrap();
        prop_assert!(approx_eq(result.as_number(), 1.0));
    }

    #[test]
    fn prop_div_distributive(
        a in -10_000f64..10_000.0,
        b in -10_000f64..10_000.0,
        c in 0.001f64..10_000.0
    ) {
        let va = Value::from_number(a);
        let vb = Value::from_number(b);
        let vc = Value::from_number(c);
        let left = va.add(vb).unwrap().div(vc).unwrap();
        let right = va.div(vc).unwrap().add(vb.div(vc).unwrap()).unwrap();
        prop_assert!(approx_eq(left.as_number(), right.as_number()));
    }

    #[test]
    fn prop_mod_definition_smi(a in 0i64..100_000, b in 1i64..100_000) {
        let va = Value::from_smi(a);
        let vb = Value::from_smi(b);
        let rem = va.rem(vb).unwrap();
        // rem always uses integer semantics, verify directly
        prop_assert_eq!(rem.as_number(), (a % b) as f64);
    }

    #[test]
    fn prop_pow_zero_exp(a in -1000f64..1000.0) {
        let va = Value::from_number(a);
        let zero = Value::from_number(0.0);
        let result = va.pow(zero).unwrap();
        prop_assert!(approx_eq(result.as_number(), 1.0));
    }

    #[test]
    fn prop_pow_one_base(a in -100f64..100.0) {
        let one = Value::from_number(1.0);
        let va = Value::from_number(a);
        let result = one.pow(va).unwrap();
        prop_assert!(approx_eq(result.as_number(), 1.0));
    }

    #[test]
    fn prop_pow_identity(a in -1000f64..1000.0) {
        let va = Value::from_number(a);
        let one = Value::from_number(1.0);
        let result = va.pow(one).unwrap();
        prop_assert!(approx_eq(result.as_number(), a));
    }

    #[test]
    fn prop_smi_float_consistency_add(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let smi_result = Value::from_smi(a).add(Value::from_smi(b)).unwrap();
        let float_result = Value::from_number(a as f64).add(Value::from_number(b as f64)).unwrap();
        prop_assert_eq!(smi_result.as_number(), float_result.as_number());
    }

    #[test]
    fn prop_smi_float_consistency_mul(a in -1_000i64..1_000, b in -1_000i64..1_000) {
        let smi_result = Value::from_smi(a).mul(Value::from_smi(b)).unwrap();
        let float_result = Value::from_number(a as f64).mul(Value::from_number(b as f64)).unwrap();
        prop_assert_eq!(smi_result.as_number(), float_result.as_number());
    }

    #[test]
    fn prop_bool_arithmetic_errors(a in smi_value()) {
        let tb = Value::from_bool(true);
        let result = tb.add(a);
        prop_assert!(result.is_err(), "bool + number should error");
    }

    #[test]
    fn prop_nil_arithmetic_errors(a in smi_value()) {
        let nil = Value::default();
        let result = nil.add(a);
        prop_assert!(result.is_err(), "nil + number should error");
    }

    #[test]
    fn prop_double_neg_smi(a in -1_000_000i64..1_000_000) {
        let va = Value::from_smi(a);
        let neg = va.neg().unwrap();
        let double_neg = neg.neg().unwrap();
        prop_assert_eq!(double_neg.as_number(), a as f64);
    }

    #[test]
    fn prop_double_neg_float(a in float_value()) {
        let neg = a.neg().unwrap();
        let double_neg = neg.neg().unwrap();
        prop_assert!(approx_eq(double_neg.as_number(), a.as_number()));
    }

    #[test]
    fn prop_mul_associative_smi(
        a in -100i64..100,
        b in -100i64..100,
        c in -100i64..100
    ) {
        let va = Value::from_smi(a);
        let vb = Value::from_smi(b);
        let vc = Value::from_smi(c);
        let r1 = va.mul(vb).unwrap().mul(vc).unwrap();
        let r2 = va.mul(vb.mul(vc).unwrap()).unwrap();
        prop_assert_eq!(r1.as_number(), r2.as_number());
    }

    #[test]
    fn prop_distributive_law_smi(
        a in -100i64..100,
        b in -100i64..100,
        c in -100i64..100
    ) {
        let va = Value::from_smi(a);
        let vb = Value::from_smi(b);
        let vc = Value::from_smi(c);
        let left = va.mul(vb.add(vc).unwrap()).unwrap();
        let right = va.mul(vb).unwrap().add(va.mul(vc).unwrap()).unwrap();
        prop_assert_eq!(left.as_number(), right.as_number());
    }

    #[test]
    fn prop_div_mul_inverse(a in 0i64..100_000, b in 1i64..1000) {
        let va = Value::from_smi(a);
        let vb = Value::from_smi(b);
        let quot = va.div(vb).unwrap();
        let reconstructed = quot.mul(vb).unwrap();
        if a % b == 0 {
            // Integer division: a/b * b == a
            prop_assert_eq!(reconstructed.as_number(), a as f64);
        } else {
            // Float division: a/b * b ≈ a (floating point)
            prop_assert!(approx_eq(reconstructed.as_number(), a as f64));
        }
    }
}

#[test]
fn prop_zero_add_zero() {
    let zero = Value::from_smi(0);
    let result = zero.add(zero).unwrap();
    assert_eq!(result.as_number(), 0.0);
}
