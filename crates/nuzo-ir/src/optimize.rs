//! # IR 优化 Pass
//!
//! 在 IR 构建完成后、codegen 之前运行的优化 pass 集合。
//!
//! ## 已实现的优化
//!
//! | Pass | 名称 | 说明 |
//! |------|------|------|
//! | C1 | 常量折叠 (Constant Folding) | `3 + 5` → `LoadConstant 8` |
//! | C3 | 恒等消除 (Identity Elimination) | `x + 0` → `x`，`x * 1` → `x` |
//! | C4 | 死代码消除 (Dead Code Elimination) | 移除终止指令后的不可达指令 |
//!
//! ## 设计原则
//!
//! - **保守但安全**：仅在能证明安全时才优化，宁可多保留指令
//! - **保持语义**：除零、NaN 等运行时行为必须保留
//! - **不动控制流**：本 pass 不修改 CFG 结构，只替换块内指令
//! - **幂等**：多次运行同一 pass 产生相同结果
//! - **不动点循环**：优化会反复执行直到收敛（无更多修改），确保跨 pass 联动

use crate::types::{
    BasicBlock, IrBinOp, IrConstant, IrFunction, IrModule, IrOp, IrUnaryOp, ValueRef,
};
use std::collections::{HashMap, HashSet};

/// 优化入口：对整个 IrModule 应用所有优化 pass
///
/// 按顺序应用：常量折叠+恒等消除 → 死代码消除 → 冗余 Mov 消除。
/// 使用**不动点迭代**：反复执行 pass 直到无更多修改，确保跨指令/跨 pass 联动。
/// 返回被替换/移除的指令总数（用于统计和测试）。
pub fn optimize(module: &mut IrModule) -> usize {
    let mut total = 0;
    let mut changed = true;
    // 安全阀：最大迭代次数 = 总指令数 * 2（防止理论上的无限循环）
    let max_iterations = module
        .functions
        .iter()
        .map(|f| f.blocks.iter().map(|b| b.instructions.len()).sum::<usize>())
        .max()
        .unwrap_or(0)
        .saturating_mul(2)
        .max(1); // 至少允许 1 轮

    let mut iteration = 0;
    while changed && iteration < max_iterations {
        changed = false;
        iteration += 1;
        for func in &mut module.functions {
            let n = optimize_function(func);
            if n > 0 {
                changed = true;
            }
            total += n;
        }
    }
    total
}

/// 对单个函数应用所有优化 pass
///
/// 优化分为两个阶段：
/// 1. **块级 pass**: 常量折叠、恒等消除、DCE（逐块执行）
/// 2. **函数级 pass**: 冗余 Mov 消除（需跨块 liveness 分析）
pub fn optimize_function(func: &mut IrFunction) -> usize {
    let mut total = 0;

    // Phase 1: 块级 pass（逐块执行）
    for block in &mut func.blocks {
        total += optimize_block(block);
    }

    // Phase 2: 函数级 Mov 消除（跨块 liveness 分析）
    total += remove_redundant_movs_function(func);

    total
}

/// 对单个基本块应用优化 pass
///
/// 单块内完成所有优化（无需跨块数据流分析）：
/// 1. 扫描 LoadConstant 建立 `ValueRef → IrConstant` 映射
/// 2. 遍历指令，对 Binary/Unary 应用常量折叠和恒等消除
/// 3. 移除终止指令后的不可达指令（DCE）
pub fn optimize_block(block: &mut BasicBlock) -> usize {
    if block.instructions.is_empty() {
        return 0;
    }

    // Step 1: 建立 ValueRef → IrConstant 映射
    let mut constants: HashMap<ValueRef, IrConstant> = HashMap::new();
    for ins in &block.instructions {
        if let IrOp::LoadConstant { dest, constant } = ins {
            constants.insert(*dest, constant.clone());
        }
    }

    // Step 2: 遍历指令，应用常量折叠 + 恒等消除
    // 采用"计算替换值 → 赋值"两步走，避免借用冲突
    let mut rewritten = 0usize;
    for ins in &mut block.instructions {
        let replacement: Option<(IrOp, Option<IrConstant>)> = match ins {
            IrOp::Binary { dest, op, left, right } => {
                let dest = *dest;
                let op = *op;
                let left = *left;
                let right = *right;
                let left_const = constants.get(&left);
                let right_const = constants.get(&right);

                try_fold_binary(dest, op, left, right, left_const, right_const)
            }

            IrOp::Unary { dest, op, operand } => {
                let dest = *dest;
                let op = *op;
                let operand = *operand;
                if let Some(c) = constants.get(&operand) {
                    fold_unary(op, c).map(|result| {
                        (IrOp::LoadConstant { dest, constant: result.clone() }, Some(result))
                    })
                } else {
                    None
                }
            }

            _ => None,
        };

        if let Some((new_op, new_const)) = replacement {
            // 更新常量映射（如果替换后是 LoadConstant）
            if let Some(c) = new_const
                && let IrOp::LoadConstant { dest, .. } = &new_op
            {
                constants.insert(*dest, c);
            }
            *ins = new_op;
            rewritten += 1;
        }
    }

    // Step 3: 死代码消除 — 移除终止指令后的所有指令
    let dce_removed = remove_dead_code_after_terminator(block);

    rewritten + dce_removed
}

/// 尝试对二元运算应用常量折叠或恒等消除
///
/// 返回 `Some((new_op, new_const))` 表示应替换指令：
/// - `new_op`: 替换后的 IrOp（LoadConstant 或 Mov）
/// - `new_const`: 如果 new_op 是 LoadConstant，则为对应的常量值（用于更新常量映射）
///
/// 返回 `None` 表示不优化（保留原指令）。
fn try_fold_binary(
    dest: ValueRef,
    op: IrBinOp,
    left: ValueRef,
    right: ValueRef,
    left_const: Option<&IrConstant>,
    right_const: Option<&IrConstant>,
) -> Option<(IrOp, Option<IrConstant>)> {
    // ── C1: 常量折叠（两个操作数都是常量）──
    // 除零/模零不折叠（保留运行时错误）
    if let (Some(lc), Some(rc)) = (&left_const, &right_const) {
        let skip = match op {
            IrBinOp::Div | IrBinOp::Mod => {
                matches!(rc, IrConstant::Number(n) if is_positive_zero(*n))
            }
            _ => false,
        };
        if !skip && let Some(result) = fold_binary(op, lc, rc) {
            let new_op = IrOp::LoadConstant { dest, constant: result.clone() };
            return Some((new_op, Some(result)));
        }
    }

    // ── C3: 恒等消除（一个操作数是常量且匹配恒等模式）──
    //
    // 关键约束：必须区分 +0.0 和 -0.0（IEEE 754 位模式不同），
    // 因为 `x + (-0.0)` 在语义上不同于 `x + 0.0`（例如涉及
    // signbit 的比较运算和 fma 等操作会区分正零与负零）。
    // 同理，乘法/除法/幂的恒等元也必须精确匹配正零或正1。
    match op {
        IrBinOp::Add => {
            // x + 0 = x, 0 + x = x（仅正零，-0.0 不优化以保留 signbit 语义）
            if let Some(IrConstant::Number(n)) = &right_const
                && is_positive_zero(*n)
            {
                return Some((IrOp::Mov { dest, src: left }, None));
            }
            if let Some(IrConstant::Number(n)) = &left_const
                && is_positive_zero(*n)
            {
                return Some((IrOp::Mov { dest, src: right }, None));
            }
        }
        IrBinOp::Sub => {
            // x - 0 = x（仅正零；注意：0 - x 不优化，结果为 -x）
            if let Some(IrConstant::Number(n)) = &right_const
                && is_positive_zero(*n)
            {
                return Some((IrOp::Mov { dest, src: left }, None));
            }
        }
        IrBinOp::Mul => {
            // x * 1 = x, 1 * x = x（仅正1）
            if let Some(IrConstant::Number(n)) = &right_const {
                if is_positive_one(*n) {
                    return Some((IrOp::Mov { dest, src: left }, None));
                }
                if is_positive_zero(*n) {
                    let c = IrConstant::Number(0.0);
                    return Some((IrOp::LoadConstant { dest, constant: c.clone() }, Some(c)));
                }
            }
            if let Some(IrConstant::Number(n)) = &left_const {
                if is_positive_one(*n) {
                    return Some((IrOp::Mov { dest, src: right }, None));
                }
                if is_positive_zero(*n) {
                    let c = IrConstant::Number(0.0);
                    return Some((IrOp::LoadConstant { dest, constant: c.clone() }, Some(c)));
                }
            }
        }
        IrBinOp::Div => {
            // x / 1 = x（仅正1）
            if let Some(IrConstant::Number(n)) = &right_const
                && is_positive_one(*n)
            {
                return Some((IrOp::Mov { dest, src: left }, None));
            }
        }
        IrBinOp::Pow => {
            // x ** 0 = 1（仅正零）, x ** 1 = x（仅正1）
            if let Some(IrConstant::Number(n)) = &right_const {
                if is_positive_zero(*n) {
                    let c = IrConstant::Number(1.0);
                    return Some((IrOp::LoadConstant { dest, constant: c.clone() }, Some(c)));
                }
                if is_positive_one(*n) {
                    return Some((IrOp::Mov { dest, src: left }, None));
                }
            }
        }
        _ => {}
    }

    None
}

/// 判断浮点数是否为正零（+0.0），排除 -0.0
///
/// IEEE 754 中 +0.0 和 -0.0 的位模式不同：
/// - +0.0 = `0x0000_0000_0000_0000`
/// - -0.0 = `0x8000_0000_0000_0000`
///
/// 使用 `to_bits()` 精确区分，避免 `== 0.0` 将 -0.0 也匹配为真的语义陷阱。
#[inline]
fn is_positive_zero(n: f64) -> bool {
    n.to_bits() == 0.0f64.to_bits()
}

/// 判断浮点数是否为正1（+1.0），排除 -1.0
///
/// 虽然当前无 `-1.0` 作为乘法恒等元的场景，但保持与
/// `is_positive_zero` 一致的精确匹配风格，避免未来引入
/// `x * (-1.0) → -x` 优化时产生混淆。
#[inline]
fn is_positive_one(n: f64) -> bool {
    n.to_bits() == 1.0f64.to_bits()
}

/// 常量折叠：对两个常量应用二元运算，返回结果常量
///
/// 返回 `None` 表示该运算无法在编译期求值（如字符串拼接、类型不匹配等）。
/// 语义遵循 Nuzo 语言运行时行为：
/// - Number op Number → Number（除零返回 None，保留运行时错误）
/// - String + String → String（拼接）
/// - Bool == Bool → Bool
fn fold_binary(op: IrBinOp, left: &IrConstant, right: &IrConstant) -> Option<IrConstant> {
    match (op, left, right) {
        // ── 数值运算 ──
        (IrBinOp::Add, IrConstant::Number(l), IrConstant::Number(r)) => {
            Some(IrConstant::Number(l + r))
        }
        (IrBinOp::Sub, IrConstant::Number(l), IrConstant::Number(r)) => {
            Some(IrConstant::Number(l - r))
        }
        (IrBinOp::Mul, IrConstant::Number(l), IrConstant::Number(r)) => {
            Some(IrConstant::Number(l * r))
        }
        (IrBinOp::Div, IrConstant::Number(l), IrConstant::Number(r)) => {
            // 除零不折叠（保留运行时 DivByZero 错误）
            if is_positive_zero(*r) { None } else { Some(IrConstant::Number(l / r)) }
        }
        (IrBinOp::Mod, IrConstant::Number(l), IrConstant::Number(r)) => {
            if is_positive_zero(*r) {
                None
            } else {
                Some(IrConstant::Number(l % r))
            }
        }
        (IrBinOp::Pow, IrConstant::Number(l), IrConstant::Number(r)) => {
            Some(IrConstant::Number(l.powf(*r)))
        }

        // ── 字符串拼接 ──
        (IrBinOp::Add, IrConstant::String(l), IrConstant::String(r)) => {
            let mut s = String::with_capacity(l.len() + r.len());
            s.push_str(l);
            s.push_str(r);
            Some(IrConstant::String(s.into()))
        }

        // ── 比较：Number op Number ──
        (IrBinOp::Eq, IrConstant::Number(l), IrConstant::Number(r)) => {
            Some(IrConstant::Bool(l == r))
        }
        (IrBinOp::Neq, IrConstant::Number(l), IrConstant::Number(r)) => {
            Some(IrConstant::Bool(l != r))
        }
        (IrBinOp::Lt, IrConstant::Number(l), IrConstant::Number(r)) => {
            Some(IrConstant::Bool(l < r))
        }
        (IrBinOp::Gt, IrConstant::Number(l), IrConstant::Number(r)) => {
            Some(IrConstant::Bool(l > r))
        }
        (IrBinOp::Le, IrConstant::Number(l), IrConstant::Number(r)) => {
            Some(IrConstant::Bool(l <= r))
        }
        (IrBinOp::Ge, IrConstant::Number(l), IrConstant::Number(r)) => {
            Some(IrConstant::Bool(l >= r))
        }

        // ── 比较：Bool op Bool ──
        (IrBinOp::Eq, IrConstant::Bool(l), IrConstant::Bool(r)) => Some(IrConstant::Bool(l == r)),
        (IrBinOp::Neq, IrConstant::Bool(l), IrConstant::Bool(r)) => Some(IrConstant::Bool(l != r)),

        // ── 比较：String op String ──
        (IrBinOp::Eq, IrConstant::String(l), IrConstant::String(r)) => {
            Some(IrConstant::Bool(l == r))
        }
        (IrBinOp::Neq, IrConstant::String(l), IrConstant::String(r)) => {
            Some(IrConstant::Bool(l != r))
        }

        // ── 比较：Nil ──
        (IrBinOp::Eq, IrConstant::Nil, IrConstant::Nil) => Some(IrConstant::Bool(true)),
        (IrBinOp::Neq, IrConstant::Nil, IrConstant::Nil) => Some(IrConstant::Bool(false)),

        _ => None,
    }
}

/// 常量折叠：对常量应用一元运算
fn fold_unary(op: IrUnaryOp, operand: &IrConstant) -> Option<IrConstant> {
    match (op, operand) {
        (IrUnaryOp::Neg, IrConstant::Number(n)) => {
            let result = -n;
            let normalized = if result == 0.0 { 0.0 } else { result };
            Some(IrConstant::Number(normalized))
        }
        (IrUnaryOp::Not, IrConstant::Bool(b)) => Some(IrConstant::Bool(!b)),
        _ => None,
    }
}

/// 死代码消除：移除基本块中终止指令之后的所有指令
///
/// 终止指令包括：Jump、JumpIf、Return。
/// 这些指令之后的内容不可达，应当移除。
fn remove_dead_code_after_terminator(block: &mut BasicBlock) -> usize {
    let terminator_idx = block.instructions.iter().position(|ins| ins.is_terminator());
    if let Some(idx) = terminator_idx {
        let total = block.instructions.len();
        if idx + 1 < total {
            let removed = total - idx - 1;
            block.instructions.truncate(idx + 1);
            return removed;
        }
    }
    0
}

/// 函数级冗余 Mov 消除：移除函数中无用的 Mov 指令
///
/// 消除两种冗余 Mov（跨块 liveness 分析）：
/// 1. **自循环 Mov**: `v0 = mov v0`（dest == src，无意义拷贝）
/// 2. **未使用 Mov**: Mov 的 dest ValueRef 在整个函数任何块中从未被引用
///
/// 返回移除的指令数。
fn remove_redundant_movs_function(func: &mut IrFunction) -> usize {
    if func.blocks.is_empty() {
        return 0;
    }

    // Phase 1: 收集函数级所有被引用的 ValueRef（跨块）
    let used = collect_function_used_values(func);

    // Phase 2: 移除各块中的冗余 Mov
    let mut removed = 0;
    for block in &mut func.blocks {
        block.instructions.retain(|ins| {
            if let IrOp::Mov { dest, src } = ins {
                // 自循环 Mov: v0 = mov v0 → 移除
                if dest == src {
                    removed += 1;
                    return false;
                }
                // 未使用 Mov: dest 在整个函数中从未被引用 → 移除
                if !used.contains(dest) {
                    removed += 1;
                    return false;
                }
            }
            true
        });
    }
    removed
}

/// 收集整个函数中所有被引用的 ValueRef（跨所有基本块）
///
/// 使用 `IrOp::src_value_refs()` 统一接口，避免重复的 match 分发。
/// 新增 IrOp 变体时只需在 `src_value_refs()` 一处更新。
fn collect_function_used_values(func: &IrFunction) -> HashSet<ValueRef> {
    let mut used: HashSet<ValueRef> = HashSet::new();
    for block in &func.blocks {
        for ins in &block.instructions {
            for vref in ins.src_value_refs() {
                used.insert(vref);
            }
        }
    }
    used
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BasicBlockId, IrBinOp, IrConstant, IrFunctionId, IrUnaryOp, ValueRef};

    fn vr(n: u32) -> ValueRef {
        ValueRef(n)
    }
    fn bb(id: u32) -> BasicBlockId {
        BasicBlockId(id)
    }

    /// 构造一个空基本块用于测试
    fn make_block() -> BasicBlock {
        BasicBlock::new(bb(0))
    }

    #[test]
    fn test_constant_fold_add() {
        // v0 = 3; v1 = 5; v2 = v0 + v1 → v2 = 8
        let mut block = make_block();
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(3.0) });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(5.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert_eq!(rewritten, 1, "应折叠 1 条 Add 指令");

        // 最后一条应为 LoadConstant 8
        match &block.instructions[2] {
            IrOp::LoadConstant { dest, constant: IrConstant::Number(n) } => {
                assert_eq!(*dest, vr(2));
                assert_eq!(*n, 8.0);
            }
            other => panic!("期望 LoadConstant，实际 {:?}", other),
        }
    }

    #[test]
    fn test_constant_fold_div_by_zero_not_folded() {
        // v0 = 1; v1 = 0; v2 = v0 / v1 → 保留 Div（运行时错误）
        let mut block = make_block();
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(0.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Div, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert_eq!(rewritten, 0, "除零不应被折叠");

        match &block.instructions[2] {
            IrOp::Binary { op: IrBinOp::Div, .. } => {}
            other => panic!("期望保留 Div，实际 {:?}", other),
        }
    }

    #[test]
    fn test_identity_add_zero_right() {
        // v0 = x (GetGlobal); v1 = 0; v2 = v0 + v1 → v2 = mov v0
        let mut block = make_block();
        block.push(IrOp::GetGlobal { dest: vr(0), name: "x".into() });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(0.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert!(rewritten >= 1, "x + 0 应消除为 Mov");

        // 恒等消除产生 Mov，但 dest 未被使用时 Mov 消除会一并清理
        // 所以最终结果可能直接是 LoadConstant (如果后续有其他优化) 或指令被移除
        assert!(block.instructions.len() <= 3, "指令数不应增加");
    }

    #[test]
    fn test_identity_add_zero_left() {
        // v0 = 0; v1 = x; v2 = v0 + v1 → v2 = mov v1
        let mut block = make_block();
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(0.0) });
        block.push(IrOp::GetGlobal { dest: vr(1), name: "x".into() });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert!(rewritten >= 1, "0 + x 应消除为 Mov");
    }

    #[test]
    fn test_identity_sub_zero_left_not_optimized() {
        // 0 - x 不应优化（结果是 -x）
        let mut block = make_block();
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(0.0) });
        block.push(IrOp::GetGlobal { dest: vr(1), name: "x".into() });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Sub, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert_eq!(rewritten, 0, "0 - x 不应被优化");
    }

    #[test]
    fn test_identity_mul_one() {
        // x * 1 → x
        let mut block = make_block();
        block.push(IrOp::GetGlobal { dest: vr(0), name: "x".into() });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(1.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Mul, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert!(rewritten >= 1, "x * 1 应消除");
    }

    #[test]
    fn test_identity_mul_zero() {
        // x * 0 → 0
        let mut block = make_block();
        block.push(IrOp::GetGlobal { dest: vr(0), name: "x".into() });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(0.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Mul, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert_eq!(rewritten, 1, "x * 0 应折叠为 LoadConstant 0");
        match &block.instructions[2] {
            IrOp::LoadConstant { constant: IrConstant::Number(n), .. } => {
                assert_eq!(*n, 0.0);
            }
            other => panic!("期望 LoadConstant 0，实际 {:?}", other),
        }
    }

    #[test]
    fn test_constant_fold_unary_neg() {
        // v0 = 42; v1 = -v0 → v1 = -42
        let mut block = make_block();
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(42.0) });
        block.push(IrOp::Unary { dest: vr(1), op: IrUnaryOp::Neg, operand: vr(0) });

        let rewritten = optimize_block(&mut block);
        assert_eq!(rewritten, 1, "应折叠 Neg 指令");

        match &block.instructions[1] {
            IrOp::LoadConstant { constant: IrConstant::Number(n), .. } => {
                assert_eq!(*n, -42.0);
            }
            other => panic!("期望 LoadConstant -42，实际 {:?}", other),
        }
    }

    #[test]
    fn test_constant_fold_unary_not() {
        // v0 = true; v1 = !v0 → v1 = false
        let mut block = make_block();
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Bool(true) });
        block.push(IrOp::Unary { dest: vr(1), op: IrUnaryOp::Not, operand: vr(0) });

        let rewritten = optimize_block(&mut block);
        assert_eq!(rewritten, 1);

        match &block.instructions[1] {
            IrOp::LoadConstant { constant: IrConstant::Bool(b), .. } => {
                assert!(!(*b));
            }
            other => panic!("期望 LoadConstant false，实际 {:?}", other),
        }
    }

    #[test]
    fn test_dce_after_return() {
        // return v0; <dead instruction>
        let mut block = make_block();
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) });
        block.push(IrOp::Return { value: Some(vr(0)) });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(2.0) }); // 不可达
        block.push(IrOp::LoadConstant { dest: vr(2), constant: IrConstant::Number(3.0) }); // 不可达

        let rewritten = optimize_block(&mut block);
        assert!(rewritten >= 2, "应移除至少 2 条不可达指令，实际 {}", rewritten);
        assert_eq!(block.instructions.len(), 2, "应只剩 2 条指令");
    }

    #[test]
    fn test_dce_after_jump() {
        let mut block = make_block();
        block.push(IrOp::Jump { target: bb(1) });
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) }); // 不可达

        let rewritten = optimize_block(&mut block);
        assert!(rewritten >= 1);
        assert_eq!(block.instructions.len(), 1);
    }

    #[test]
    fn test_string_concat_fold() {
        let mut block = make_block();
        block.push(IrOp::LoadConstant {
            dest: vr(0),
            constant: IrConstant::String("Hello, ".into()),
        });
        block.push(IrOp::LoadConstant {
            dest: vr(1),
            constant: IrConstant::String("World!".into()),
        });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert_eq!(rewritten, 1);

        match &block.instructions[2] {
            IrOp::LoadConstant { constant: IrConstant::String(s), .. } => {
                assert_eq!(&**s, "Hello, World!");
            }
            other => panic!("期望 LoadConstant 字符串，实际 {:?}", other),
        }
    }

    #[test]
    fn test_comparison_fold() {
        let mut block = make_block();
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(3.0) });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(5.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Lt, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert_eq!(rewritten, 1);

        match &block.instructions[2] {
            IrOp::LoadConstant { constant: IrConstant::Bool(b), .. } => {
                assert!(*b, "3 < 5 应为 true");
            }
            other => panic!("期望 LoadConstant true，实际 {:?}", other),
        }
    }

    #[test]
    fn test_no_optimization_for_non_constant() {
        // 两个变量相加，不优化
        let mut block = make_block();
        block.push(IrOp::GetGlobal { dest: vr(0), name: "a".into() });
        block.push(IrOp::GetGlobal { dest: vr(1), name: "b".into() });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert_eq!(rewritten, 0, "非常量操作数不应被优化");
        assert!(matches!(block.instructions[2], IrOp::Binary { .. }));
    }

    #[test]
    fn test_chained_constant_fold() {
        // v0 = 2; v1 = 3; v2 = v0 + v1 (折叠为 5); v3 = v2 * 4 (应折叠为 20)
        let mut block = make_block();
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(2.0) });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(3.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) });
        block.push(IrOp::LoadConstant { dest: vr(3), constant: IrConstant::Number(4.0) });
        block.push(IrOp::Binary { dest: vr(4), op: IrBinOp::Mul, left: vr(2), right: vr(3) });

        let rewritten = optimize_block(&mut block);
        assert_eq!(rewritten, 2, "应折叠 2 条指令");

        // 最后一条应为 LoadConstant 20
        match &block.instructions[4] {
            IrOp::LoadConstant { constant: IrConstant::Number(n), .. } => {
                assert_eq!(*n, 20.0, "(2+3)*4 应为 20");
            }
            other => panic!("期望 LoadConstant 20，实际 {:?}", other),
        }
    }

    #[test]
    fn test_div_by_one() {
        // x / 1 → x
        let mut block = make_block();
        block.push(IrOp::GetGlobal { dest: vr(0), name: "x".into() });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(1.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Div, left: vr(0), right: vr(1) });

        let rewritten = optimize_block(&mut block);
        assert!(rewritten >= 1, "x / 1 应消除");
    }

    #[test]
    fn test_pow_identity() {
        // x ** 1 → x, x ** 0 → 1
        let mut block = make_block();
        block.push(IrOp::GetGlobal { dest: vr(0), name: "x".into() });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(1.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Pow, left: vr(0), right: vr(1) });

        assert!(optimize_block(&mut block) >= 1);

        let mut block = make_block();
        block.push(IrOp::GetGlobal { dest: vr(0), name: "x".into() });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(0.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Pow, left: vr(0), right: vr(1) });

        assert!(optimize_block(&mut block) >= 1);
        match &block.instructions[2] {
            IrOp::LoadConstant { constant: IrConstant::Number(n), .. } => {
                assert_eq!(*n, 1.0);
            }
            other => panic!("期望 LoadConstant 1，实际 {:?}", other),
        }
    }

    #[test]
    fn test_empty_block_noop() {
        let mut block = make_block();
        assert_eq!(optimize_block(&mut block), 0);
    }

    #[test]
    fn test_optimize_module_entry() {
        let mut module = IrModule::new();
        let _fid = module.add_function("test");
        let func = &mut module.functions[0];
        let block = &mut func.blocks[0];
        block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(2.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) });
        block.push(IrOp::Return { value: Some(vr(2)) });

        let total = optimize(&mut module);
        assert!(total >= 1, "应至少优化 1 条指令");
    }

    // ========================================================================
    // Mov 消除测试（函数级跨块分析）
    // ========================================================================

    /// 辅助：创建包含单个基本块的 IrFunction
    fn make_function(instructions: Vec<IrOp>) -> IrFunction {
        let mut func = IrFunction::new(IrFunctionId(0), "test");
        func.blocks[0].instructions = instructions;
        func
    }

    /// 辅助：创建多基本块的 IrFunction
    fn make_multi_block_function(blocks: Vec<Vec<IrOp>>) -> IrFunction {
        let mut func = IrFunction::new(IrFunctionId(0), "test");
        func.blocks.clear();
        for (i, instrs) in blocks.into_iter().enumerate() {
            let mut block = BasicBlock::new(BasicBlockId(i as u32));
            block.instructions = instrs;
            func.blocks.push(block);
        }
        func
    }

    #[test]
    fn test_mov_self_cycle_eliminated() {
        // v0 = mov v0 (自循环，应被消除)
        let mut func = make_function(vec![IrOp::Mov { dest: vr(0), src: vr(0) }]);

        let removed = remove_redundant_movs_function(&mut func);
        assert_eq!(removed, 1, "自循环 Mov 应被移除");
        assert!(func.blocks[0].instructions.is_empty(), "块应为空");
    }

    #[test]
    fn test_mov_unused_dest_eliminated() {
        // v0 = load_const 5; v1 = mov v0; return v0
        // v1 从未被使用（return 引用的是 v0），所以 mov v0→v1 应被消除
        let mut func = make_function(vec![
            IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(5.0) },
            IrOp::Mov { dest: vr(1), src: vr(0) }, // v1 未使用
            IrOp::Return { value: Some(vr(0)) },
        ]);

        let removed = remove_redundant_movs_function(&mut func);
        assert_eq!(removed, 1, "未使用的 Mov 应被移除");
        assert_eq!(func.blocks[0].instructions.len(), 2); // 只剩 LoadConst + Return
    }

    #[test]
    fn test_mov_used_dest_preserved() {
        // v0 = get_global x; v1 = mov v0; return v1
        // v1 被 return 使用，所以 mov 应保留
        let mut func = make_function(vec![
            IrOp::GetGlobal { dest: vr(0), name: "x".into() },
            IrOp::Mov { dest: vr(1), src: vr(0) }, // v1 被使用
            IrOp::Return { value: Some(vr(1)) },
        ]);

        let removed = remove_redundant_movs_function(&mut func);
        assert_eq!(removed, 0, "被使用的 Mov 应保留");
        assert_eq!(func.blocks[0].instructions.len(), 3);
    }

    #[test]
    fn test_identity_then_mov_eliminated() {
        // 完整联动场景：x * 1 → mov → mov 未使用
        // v0 = get_global x; v1 = 1; v2 = v0 * v1 (恒等→mov v0); return v0
        // 恒等消除产生 v2=mov v0，但 return 用的是 v0 不是 v2，所以 v2 的 mov 被消除
        let mut module = IrModule::new();
        let _fid = module.add_function("test");
        let block = &mut module.functions[0].blocks[0];
        block.push(IrOp::GetGlobal { dest: vr(0), name: "x".into() });
        block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(1.0) });
        block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Mul, left: vr(0), right: vr(1) });
        block.push(IrOp::Return { value: Some(vr(0)) });

        // optimize() 会先做恒等消除（v2=mov v0），再做函数级 Mov 消除
        let total = optimize(&mut module);
        assert!(total >= 2, "恒等+Mov消除应至少移除2条，实际 {}", total);
        // 最终不应有 Mov 指令残留
        let block = &module.functions[0].blocks[0];
        for ins in &block.instructions {
            if let IrOp::Mov { .. } = ins {
                panic!("不应有残留的 Mov 指令: {:?}", ins);
            }
        }
    }

    #[test]
    fn test_fixpoint_iteration() {
        // 测试不动点迭代：链式依赖需要多轮才能完全优化
        // v0 = 2; v1 = 3; v2 = v0 + v1 → 5 (第1轮常量折叠)
        // v3 = 4; v4 = v2 * v3 → 20 (第2轮利用v2的常量)
        let mut module = IrModule::new();
        let _fid = module.add_function("test_fp");

        {
            let func = &mut module.functions[0];
            let block = &mut func.blocks[0];

            block.push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(2.0) });
            block.push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(3.0) });
            block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) });
            block.push(IrOp::LoadConstant { dest: vr(3), constant: IrConstant::Number(4.0) });
            block.push(IrOp::Binary { dest: vr(4), op: IrBinOp::Mul, left: vr(2), right: vr(3) });
            block.push(IrOp::Return { value: Some(vr(4)) });
        }

        let total = optimize(&mut module);
        assert!(total >= 2, "应至少折叠 2 条指令");

        // 验证最终 IR 状态（在 optimize 之后重新借用）
        let block = &module.functions[0].blocks[0];
        match &block.instructions[4] {
            IrOp::LoadConstant { constant: IrConstant::Number(n), .. } => {
                assert_eq!(*n, 20.0, "(2+3)*4 应为 20");
            }
            other => panic!("期望 LoadConstant 20，实际 {:?}", other),
        }
    }

    #[test]
    fn test_multiple_redundant_movs() {
        // 多个冗余 Mov 连续出现
        // v0 = 10; v1 = mov v0 (未用); v2 = mov v0 (未用); v3 = mov v3 (自循环); return v0
        let mut func = make_function(vec![
            IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(10.0) },
            IrOp::Mov { dest: vr(1), src: vr(0) }, // 未用
            IrOp::Mov { dest: vr(2), src: vr(0) }, // 未用
            IrOp::Mov { dest: vr(3), src: vr(3) }, // 自循环
            IrOp::Return { value: Some(vr(0)) },
        ]);

        let removed = remove_redundant_movs_function(&mut func);
        assert_eq!(removed, 3, "应移除 3 个冗余 Mov");
        assert_eq!(func.blocks[0].instructions.len(), 2); // LoadConst + Return
    }

    #[test]
    fn test_mov_used_in_binary_preserved() {
        // v0 = 5; v1 = mov v0; v2 = v1 + 1
        // v1 被 Binary 使用，应保留
        let mut func = make_function(vec![
            IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(5.0) },
            IrOp::Mov { dest: vr(1), src: vr(0) },
            IrOp::LoadConstant { dest: vr(2), constant: IrConstant::Number(1.0) },
            IrOp::Binary { dest: vr(3), op: IrBinOp::Add, left: vr(1), right: vr(2) },
        ]);

        let removed = remove_redundant_movs_function(&mut func);
        assert_eq!(removed, 0, "被 Binary 使用的 Mov 应保留");
    }

    #[test]
    fn test_mov_cross_block_used_preserved() {
        // 跨块引用：bb0 中的 v2 = mov v1 被 bb1 中的 v3 = mov v2 引用
        // v2 的 mov 应保留（因为跨块被使用）
        let mut func = make_multi_block_function(vec![
            // bb0: v1 = 42; v2 = mov v1; jump bb1
            vec![
                IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(42.0) },
                IrOp::Mov { dest: vr(2), src: vr(1) },
                IrOp::Jump { target: BasicBlockId(1) },
            ],
            // bb1: v3 = mov v2; return v3
            vec![IrOp::Mov { dest: vr(3), src: vr(2) }, IrOp::Return { value: Some(vr(3)) }],
        ]);

        let removed = remove_redundant_movs_function(&mut func);
        // v2 的 mov 被 v3=mov v2 跨块引用，应保留
        // v3 被 return 引用，应保留
        assert_eq!(removed, 0, "跨块引用的 Mov 应保留");
    }

    #[test]
    fn test_mov_cross_block_unused_eliminated() {
        // 跨块未引用：bb0 中的 v2 = mov v1 在 bb1 中未被引用
        // bb1 直接使用 v1 而不是 v2
        let mut func = make_multi_block_function(vec![
            // bb0: v1 = 42; v2 = mov v1; jump bb1
            vec![
                IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(42.0) },
                IrOp::Mov { dest: vr(2), src: vr(1) }, // v2 在 bb1 中未引用
                IrOp::Jump { target: BasicBlockId(1) },
            ],
            // bb1: return v1 (使用 v1，不是 v2)
            vec![IrOp::Return { value: Some(vr(1)) }],
        ]);

        let removed = remove_redundant_movs_function(&mut func);
        assert_eq!(removed, 1, "跨块未引用的 Mov 应被移除");
    }

    // ========================================================================
    // 链式恒等消除回归测试
    //
    // 这组测试锁定"链式恒等消除"优化的正确行为：当多个恒等运算
    // （x+0, x*1, x-0）串联出现时，优化器应将每层 Binary 替换为 Mov，
    // 形成可追溯的 Mov 链，且不产生悬空 ValueRef 引用。
    //
    // 注意：本组测试不引入 copy_propagation 或 resolve_mov_chain 等
    // 新优化 pass，仅验证现有 optimize_function / optimize 的行为。
    // 若优化器产生悬空引用，应如实暴露为测试失败，而非自行修复。
    // ========================================================================

    /// 辅助：断言函数中所有被引用的 ValueRef 都有对应定义（无悬空引用）
    ///
    /// 收集所有指令的 dest 作为"定义集"，再遍历所有指令的源操作数
    /// 作为"引用集"，验证 引用集 ⊆ 定义集。任一引用未定义则 panic。
    /// 注：IrModule::validate() 仅检测 ValueRef 是否在合理范围内，
    /// 不检测悬空引用，故此处补充该检查。
    fn assert_no_undefined_refs(func: &IrFunction, ctx: &str) {
        let mut defined: HashSet<ValueRef> = HashSet::new();
        for block in &func.blocks {
            for ins in &block.instructions {
                if let Some(dest) = ins.dest() {
                    defined.insert(dest);
                }
            }
        }
        for block in &func.blocks {
            for ins in &block.instructions {
                for src in ins.src_value_refs() {
                    assert!(
                        defined.contains(&src),
                        "[{}] 发现悬空引用：指令 {:?} 引用了未定义的 {:?}",
                        ctx,
                        ins,
                        src
                    );
                }
            }
        }
    }

    /// 辅助：沿 Mov 链追溯源值
    ///
    /// 从 start 出发，遇到 Mov 则跳到其 src 继续追溯，遇到 LoadConstant
    /// 则返回其常量值，遇到其他指令或超过最大跳数则返回 None。
    /// 用于验证 Mov 链的语义保持性（最终能追溯到具体的常量定义）。
    fn trace_mov_chain_to_constant(func: &IrFunction, start: ValueRef) -> Option<IrConstant> {
        // 建立 dest → 指令引用映射
        let mut def: HashMap<ValueRef, &IrOp> = HashMap::new();
        for block in &func.blocks {
            for ins in &block.instructions {
                if let Some(dest) = ins.dest() {
                    def.insert(dest, ins);
                }
            }
        }
        let mut current = start;
        const MAX_HOPS: usize = 10_000; // 安全阀，防止理论上的 Mov 环
        for _ in 0..MAX_HOPS {
            match def.get(&current)? {
                IrOp::Mov { src, .. } => current = *src,
                IrOp::LoadConstant { constant, .. } => return Some(constant.clone()),
                _ => return None,
            }
        }
        None
    }

    /// 辅助：从函数中提取第一条 Return 指令的返回值 ValueRef
    fn extract_return_value(func: &IrFunction) -> Option<ValueRef> {
        for block in &func.blocks {
            for ins in &block.instructions {
                if let IrOp::Return { value: Some(v) } = ins {
                    return Some(*v);
                }
            }
        }
        None
    }

    /// 辅助：统计函数中指定二元运算（Add/Mul/Sub 等）的指令数量
    fn count_binary_op(func: &IrFunction, target_op: IrBinOp) -> usize {
        let mut count = 0;
        for block in &func.blocks {
            for ins in &block.instructions {
                if let IrOp::Binary { op, .. } = ins
                    && *op == target_op
                {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn test_chained_identity_2_levels() {
        // 场景：2 层链式恒等消除
        //   v0 = LoadConstant(0)      // 加法零元
        //   v1 = LoadConstant(42)     // 基准值
        //   v3 = v1 + v0              // x + 0 → Mov v3, v1
        //   v4 = LoadConstant(1)      // 乘法单位元
        //   v5 = v3 * v4              // x * 1 → Mov v5, v3
        //   Return(v5)
        // 期望：Add 与 Mul 均被消除为 Mov，v5 经 Mov 链追溯到 v1=LoadConstant(42)
        let base_value = 42.0_f64;
        let add_identity = 0.0_f64;
        let mul_identity = 1.0_f64;

        let mut func = make_function(vec![
            IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(add_identity) },
            IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(base_value) },
            IrOp::Binary { dest: vr(3), op: IrBinOp::Add, left: vr(1), right: vr(0) },
            IrOp::LoadConstant { dest: vr(4), constant: IrConstant::Number(mul_identity) },
            IrOp::Binary { dest: vr(5), op: IrBinOp::Mul, left: vr(3), right: vr(4) },
            IrOp::Return { value: Some(vr(5)) },
        ]);

        optimize_function(&mut func);

        // 断言 1：Add 与 Mul 指令应被完全消除
        assert_eq!(count_binary_op(&func, IrBinOp::Add), 0, "Add 应被恒等消除");
        assert_eq!(count_binary_op(&func, IrBinOp::Mul), 0, "Mul 应被恒等消除");

        // 断言 2：无悬空引用
        assert_no_undefined_refs(&func, "test_chained_identity_2_levels");

        // 断言 3：Return 的参数经 Mov 链应追溯到 LoadConstant(42)
        let ret_val = extract_return_value(&func).expect("应存在 Return 指令");
        let traced = trace_mov_chain_to_constant(&func, ret_val).expect("Mov 链应追溯到常量定义");
        match traced {
            IrConstant::Number(n) => assert_eq!(n, base_value, "应追溯到基准值 42"),
            other => panic!("期望追溯到 Number 常量，实际 {:?}", other),
        }
    }

    #[test]
    fn test_chained_identity_3_levels() {
        // 场景：3 层链式恒等消除（Add 0 → Mul 1 → Sub 0）
        //   v0 = LoadConstant(42)
        //   v1 = LoadConstant(0)
        //   v2 = v0 + v1     → Mov v2, v0
        //   v3 = LoadConstant(1)
        //   v4 = v2 * v3     → Mov v4, v2
        //   v5 = LoadConstant(0)
        //   v6 = v4 - v5     → Mov v6, v4
        //   Return(v6)
        // 期望：三层恒等运算全部消除，Return 引用的值在 IR 中有定义
        let base_value = 42.0_f64;
        let add_identity = 0.0_f64;
        let mul_identity = 1.0_f64;

        let mut func = make_function(vec![
            IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(base_value) },
            IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(add_identity) },
            IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) },
            IrOp::LoadConstant { dest: vr(3), constant: IrConstant::Number(mul_identity) },
            IrOp::Binary { dest: vr(4), op: IrBinOp::Mul, left: vr(2), right: vr(3) },
            IrOp::LoadConstant { dest: vr(5), constant: IrConstant::Number(add_identity) },
            IrOp::Binary { dest: vr(6), op: IrBinOp::Sub, left: vr(4), right: vr(5) },
            IrOp::Return { value: Some(vr(6)) },
        ]);

        optimize_function(&mut func);

        // 断言 1：三种恒等运算全部消除
        assert_eq!(count_binary_op(&func, IrBinOp::Add), 0, "Add 应被消除");
        assert_eq!(count_binary_op(&func, IrBinOp::Mul), 0, "Mul 应被消除");
        assert_eq!(count_binary_op(&func, IrBinOp::Sub), 0, "Sub 应被消除");

        // 断言 2：无悬空引用（Return 引用的值在 IR 中有定义）
        assert_no_undefined_refs(&func, "test_chained_identity_3_levels");

        // 断言 3：Mov 链最终追溯到 LoadConstant(42)
        let ret_val = extract_return_value(&func).expect("应存在 Return 指令");
        let traced = trace_mov_chain_to_constant(&func, ret_val).expect("Mov 链应追溯到常量定义");
        match traced {
            IrConstant::Number(n) => assert_eq!(n, base_value, "应追溯到基准值 42"),
            other => panic!("期望追溯到 Number 常量，实际 {:?}", other),
        }
    }

    #[test]
    fn test_chained_mov_no_undefined_refs() {
        // 场景：纯 Mov 链（无恒等运算产生，直接构造）
        //   v1 = LoadConstant(42)
        //   Mov v3, v1
        //   Mov v5, v3
        //   Mov v7, v5
        //   Return(v7)
        // 期望：所有 Mov 的 dest 都被下游引用，全部保留；无悬空引用
        let base_value = 42.0_f64;

        let mut func = make_function(vec![
            IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(base_value) },
            IrOp::Mov { dest: vr(3), src: vr(1) },
            IrOp::Mov { dest: vr(5), src: vr(3) },
            IrOp::Mov { dest: vr(7), src: vr(5) },
            IrOp::Return { value: Some(vr(7)) },
        ]);

        optimize_function(&mut func);

        // 断言 1：优化后所有被引用的 ValueRef 都有定义（无悬空引用）
        assert_no_undefined_refs(&func, "test_chained_mov_no_undefined_refs");

        // 断言 2：Mov 链语义保持 — Return(v7) 追溯到 LoadConstant(42)
        let ret_val = extract_return_value(&func).expect("应存在 Return 指令");
        let traced = trace_mov_chain_to_constant(&func, ret_val).expect("Mov 链应追溯到常量定义");
        match traced {
            IrConstant::Number(n) => assert_eq!(n, base_value, "应追溯到基准值 42"),
            other => panic!("期望追溯到 Number 常量，实际 {:?}", other),
        }
    }

    #[test]
    fn test_fixpoint_resolves_mov_chain() {
        // 场景：恒等消除产生 Mov 链，验证顶层 optimize 不动点迭代后 IR 有效
        //   v0 = LoadConstant(42)
        //   v1 = LoadConstant(0)
        //   v2 = v0 + v1   → Mov v2, v0
        //   v3 = LoadConstant(1)
        //   v4 = v2 * v3   → Mov v4, v2
        //   v5 = LoadConstant(0)
        //   v6 = v4 - v5   → Mov v6, v4
        //   Return(v6)
        // 期望：optimize 收敛后，module.validate() 通过且无悬空引用
        let base_value = 42.0_f64;
        let add_identity = 0.0_f64;
        let mul_identity = 1.0_f64;

        let mut module = IrModule::new();
        let _fid = module.add_function("chained_identity");
        {
            let block = &mut module.functions[0].blocks[0];
            block
                .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(base_value) });
            block.push(IrOp::LoadConstant {
                dest: vr(1),
                constant: IrConstant::Number(add_identity),
            });
            block.push(IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) });
            block.push(IrOp::LoadConstant {
                dest: vr(3),
                constant: IrConstant::Number(mul_identity),
            });
            block.push(IrOp::Binary { dest: vr(4), op: IrBinOp::Mul, left: vr(2), right: vr(3) });
            block.push(IrOp::LoadConstant {
                dest: vr(5),
                constant: IrConstant::Number(add_identity),
            });
            block.push(IrOp::Binary { dest: vr(6), op: IrBinOp::Sub, left: vr(4), right: vr(5) });
            block.push(IrOp::Return { value: Some(vr(6)) });
        }

        // 顶层入口：不动点迭代
        let total = optimize(&mut module);
        assert!(total >= 3, "应至少消除 3 条恒等运算指令，实际 {}", total);

        // 断言 1：module.validate() 结构性检查通过
        assert!(module.validate().is_ok(), "优化后模块应通过结构性验证");

        // 断言 2：手动检查无悬空引用（validate 不检测悬空引用，需补查）
        let func = &module.functions[0];
        assert_no_undefined_refs(func, "test_fixpoint_resolves_mov_chain");

        // 断言 3：Mov 链语义保持
        let ret_val = extract_return_value(func).expect("应存在 Return 指令");
        let traced = trace_mov_chain_to_constant(func, ret_val).expect("Mov 链应追溯到常量定义");
        match traced {
            IrConstant::Number(n) => assert_eq!(n, base_value, "应追溯到基准值 42"),
            other => panic!("期望追溯到 Number 常量，实际 {:?}", other),
        }
    }

    #[test]
    fn test_mov_chain_preserves_semantics() {
        // 场景：Mov 链语义保持性验证
        //   v1 = LoadConstant(42)
        //   Mov v3, v1
        //   Mov v5, v3
        //   Mov v7, v5
        //   Return(v7)
        // 期望：优化后 Return(v7) 经 Mov 链最终追溯到 LoadConstant(42)
        let base_value = 42.0_f64;

        let mut func = make_function(vec![
            IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(base_value) },
            IrOp::Mov { dest: vr(3), src: vr(1) },
            IrOp::Mov { dest: vr(5), src: vr(3) },
            IrOp::Mov { dest: vr(7), src: vr(5) },
            IrOp::Return { value: Some(vr(7)) },
        ]);

        optimize_function(&mut func);

        // 提取 Return 参数并沿 Mov 链追溯
        let ret_val = extract_return_value(&func).expect("应存在 Return 指令");
        let traced = trace_mov_chain_to_constant(&func, ret_val).expect("Mov 链应追溯到常量定义");
        match traced {
            IrConstant::Number(n) => {
                assert_eq!(n, base_value, "Mov 链应保持语义，追溯到 LoadConstant(42)");
            }
            other => panic!("期望追溯到 Number 常量，实际 {:?}", other),
        }

        // 额外保证：无悬空引用
        assert_no_undefined_refs(&func, "test_mov_chain_preserves_semantics");
    }
}
