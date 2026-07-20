//! 引用计数扫描器 -- 统计每个 ValueRef 作为源操作数的总使用次数
//!
//! 核心算法：
//! 1. 遍历函数内所有指令的 `src_value_refs()`，统计每个 ValueRef 被使用的次数
//! 2. 循环保护：对回边目标块（循环头）中使用的值，引用计数 +1，
//!    确保循环不变量不会被循环体内的 consume_use 过早释放
//!
//! ## 数据流
//!
//! ```text
//! IrFunction ──count_usages()──→ HashMap<u32, u32>  (ValueRef.id → 使用次数)
//!                 │
//!                 └──find_back_edges()──→ Vec<(BasicBlockId, BasicBlockId)>
//!                         │
//!                         └──apply_loop_protection()──→ 修改 counts
//! ```

use std::collections::{HashMap, HashSet};

use nuzo_ir::types::{BasicBlockId, IrFunction, IrOp};

// ---------------------------------------------------------------------------
// 公共 API
// ---------------------------------------------------------------------------

/// 扫描 IR 函数体，统计每个 ValueRef 作为源操作数的总使用次数
///
/// 返回 `HashMap<ValueRef.id, 使用次数>`。
/// 若某个 ValueRef 从未被引用，则不出现在返回值中。
pub(crate) fn count_usages(func: &IrFunction) -> HashMap<u32, u32> {
    let mut counts: HashMap<u32, u32> = HashMap::new();
    for block in &func.blocks {
        for op in &block.instructions {
            for src in op.src_value_refs() {
                *counts.entry(src.0).or_insert(0) += 1;
            }
        }
    }
    counts
}

/// 识别 CFG 中的回边（back edges）
///
/// 回边定义：边 (src -> dst) 中，dst 在 DFS 遍历中是 src 的祖先。
/// 使用 DFS 栈方法：从入口块深度优先遍历，若发现后继块在当前 DFS
/// 栈上，则 (当前块, 后继块) 为回边。
pub(crate) fn find_back_edges(func: &IrFunction) -> Vec<(BasicBlockId, BasicBlockId)> {
    let mut back_edges = Vec::new();
    let mut visited = HashSet::new();
    let mut on_stack = HashSet::new();

    if !func.blocks.is_empty() {
        dfs_back_edges(func.entry_block, func, &mut visited, &mut on_stack, &mut back_edges);
    }
    back_edges
}

/// 循环保护：对回边目标块（循环头）中使用的值，引用计数 +1
///
/// 确保循环不变量不会被循环体内的 `consume_use` 过早释放。
/// 对于循环头中作为源操作数引用的每个 ValueRef，额外加 1。
pub(crate) fn apply_loop_protection(
    counts: &mut HashMap<u32, u32>,
    func: &IrFunction,
    back_edges: &[(BasicBlockId, BasicBlockId)],
) {
    let loop_headers: HashSet<BasicBlockId> = back_edges.iter().map(|(_, dst)| *dst).collect();
    for header_id in &loop_headers {
        if let Some(header_block) = find_block(func, *header_id) {
            for op in &header_block.instructions {
                for src in op.src_value_refs() {
                    *counts.entry(src.0).or_insert(0) += 1;
                }
            }
        }
    }
}

/// 便捷函数：count_usages + apply_loop_protection 一步完成
pub(crate) fn count_usages_with_loop_protection(func: &IrFunction) -> HashMap<u32, u32> {
    let mut counts = count_usages(func);
    let back_edges = find_back_edges(func);
    apply_loop_protection(&mut counts, func, &back_edges);
    counts
}

// ---------------------------------------------------------------------------
// 内部辅助
// ---------------------------------------------------------------------------

/// 根据 BasicBlockId 在函数中查找基本块
fn find_block(func: &IrFunction, id: BasicBlockId) -> Option<&nuzo_ir::types::BasicBlock> {
    func.blocks.iter().find(|b| b.id == id)
}

/// 从终止指令中提取后继块的 BasicBlockId
///
/// 返回固定大小数组 `(长度, [Option<BasicBlockId>; 2])`，
/// 最多 2 个后继（JumpIf 有 then/else 两个分支）。
/// 避免堆分配和外部依赖。
fn successors_of(op: &IrOp) -> (usize, [Option<BasicBlockId>; 2]) {
    match op {
        IrOp::Jump { target } => (1, [Some(*target), None]),
        IrOp::JumpIf { then_target, else_target, .. } => {
            (2, [Some(*then_target), Some(*else_target)])
        }
        // Return / Out 是叶子节点，无后继
        _ => (0, [None, None]),
    }
}

/// DFS 递归查找回边
fn dfs_back_edges(
    current: BasicBlockId,
    func: &IrFunction,
    visited: &mut HashSet<BasicBlockId>,
    on_stack: &mut HashSet<BasicBlockId>,
    back_edges: &mut Vec<(BasicBlockId, BasicBlockId)>,
) {
    visited.insert(current);
    on_stack.insert(current);

    if let Some(block) = find_block(func, current) {
        // 只需检查终止指令的后继
        if let Some(term) = block.instructions.last() {
            let (len, succs) = successors_of(term);
            for &succ_opt in succs.iter().take(len) {
                let succ = match succ_opt {
                    Some(s) => s,
                    None => continue,
                };
                if on_stack.contains(&succ) {
                    // 后继在当前 DFS 栈上 → 回边
                    back_edges.push((current, succ));
                } else if !visited.contains(&succ) {
                    dfs_back_edges(succ, func, visited, on_stack, back_edges);
                }
            }
        }
    }

    on_stack.remove(&current);
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use nuzo_ir::types::{
        BasicBlock, BasicBlockId, IrBinOp, IrConstant, IrFunction, IrFunctionId, IrOp, IrUnaryOp,
        ValueRef,
    };

    use super::*;

    /// 辅助：创建空 IrFunction（仅入口块）
    fn make_func() -> IrFunction {
        IrFunction::new(IrFunctionId::new(0), "test")
    }

    /// 辅助：创建 ValueRef
    fn vr(id: u32) -> ValueRef {
        ValueRef::new(id)
    }

    /// 辅助：创建 BasicBlockId
    fn bb(id: u32) -> BasicBlockId {
        BasicBlockId::new(id)
    }

    // ── count_usages 测试 ──

    #[test]
    fn test_count_usages_straight_line() {
        // 直线代码：v0 = LoadConst, v3 = Binary(v1, v2), Return(v3)
        let mut func = make_func();
        func.blocks[0].instructions = vec![
            IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) },
            IrOp::Binary { dest: vr(3), op: IrBinOp::Add, left: vr(1), right: vr(2) },
            IrOp::Return { value: Some(vr(3)) },
        ];

        let counts = count_usages(&func);
        // v1: 1次, v2: 1次, v3: 1次（Return引用）, v0: 无引用（LoadConstant是dest不是src）
        assert_eq!(counts.get(&1), Some(&1));
        assert_eq!(counts.get(&2), Some(&1));
        assert_eq!(counts.get(&3), Some(&1));
        assert_eq!(counts.get(&0), None); // LoadConstant dest, not a source
    }

    #[test]
    fn test_count_usages_multiple_refs() {
        // v1 被引用两次：Binary(left) + Unary(operand)
        let mut func = make_func();
        func.blocks[0].instructions = vec![
            IrOp::Binary { dest: vr(3), op: IrBinOp::Add, left: vr(1), right: vr(2) },
            IrOp::Unary { dest: vr(4), op: IrUnaryOp::Neg, operand: vr(1) },
            IrOp::Return { value: Some(vr(4)) },
        ];

        let counts = count_usages(&func);
        assert_eq!(counts.get(&1), Some(&2)); // v1 被引用 2 次
        assert_eq!(counts.get(&2), Some(&1));
        assert_eq!(counts.get(&4), Some(&1));
    }

    #[test]
    fn test_count_usages_no_sources() {
        // 仅有无源操作数的指令
        let mut func = make_func();
        func.blocks[0].instructions = vec![
            IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(42.0) },
            IrOp::Return { value: None },
        ];

        let counts = count_usages(&func);
        assert!(counts.is_empty());
    }

    #[test]
    fn test_count_usages_empty_function() {
        let func = make_func();
        let counts = count_usages(&func);
        assert!(counts.is_empty());
    }

    #[test]
    fn test_count_usages_call_with_args() {
        let mut func = make_func();
        func.blocks[0].instructions = vec![
            IrOp::Call { dest: Some(vr(5)), callee: vr(10), args: vec![vr(1), vr(2), vr(3)] },
            IrOp::Return { value: Some(vr(5)) },
        ];

        let counts = count_usages(&func);
        // callee(v10) + 3 args = 4 个源操作数
        assert_eq!(counts.get(&10), Some(&1));
        assert_eq!(counts.get(&1), Some(&1));
        assert_eq!(counts.get(&2), Some(&1));
        assert_eq!(counts.get(&3), Some(&1));
        assert_eq!(counts.get(&5), Some(&1)); // Return 引用
    }

    // ── find_back_edges 测试 ──

    #[test]
    fn test_find_back_edges_no_loop() {
        // 直线代码：entry → bb1(返回)
        let func = make_func();
        let back_edges = find_back_edges(&func);
        assert!(back_edges.is_empty());
    }

    #[test]
    fn test_find_back_edges_with_loop() {
        // 构建 while 循环 CFG:
        //   bb0 (entry): JumpIf cond → bb1 (body) / bb2 (exit)
        //   bb1 (body):  Jump → bb0 (回边)
        //   bb2 (exit):  Return
        let mut func = make_func();

        // bb0 已存在（entry），添加 JumpIf
        func.blocks[0].instructions =
            vec![IrOp::JumpIf { cond: vr(0), then_target: bb(1), else_target: bb(2) }];

        // bb1: 循环体，跳回 bb0
        let mut bb1 = BasicBlock::new(bb(1));
        bb1.instructions = vec![IrOp::Jump { target: bb(0) }];
        func.blocks.push(bb1);

        // bb2: 退出
        let mut bb2 = BasicBlock::new(bb(2));
        bb2.instructions = vec![IrOp::Return { value: None }];
        func.blocks.push(bb2);

        let back_edges = find_back_edges(&func);
        assert_eq!(back_edges.len(), 1);
        assert_eq!(back_edges[0], (bb(1), bb(0))); // bb1 → bb0 是回边
    }

    #[test]
    fn test_find_back_edges_nested_loop() {
        // 嵌套循环：
        //   bb0: JumpIf → bb1 / bb3
        //   bb1: JumpIf → bb2 / bb0 (内层循环回到 bb1，外层回到 bb0)
        //   bb2: Jump → bb1 (内层回边)
        //   bb3: Return
        let mut func = make_func();

        // bb0: 外层循环头
        func.blocks[0].instructions =
            vec![IrOp::JumpIf { cond: vr(0), then_target: bb(1), else_target: bb(3) }];

        // bb1: 内层循环头
        let mut bb1 = BasicBlock::new(bb(1));
        bb1.instructions =
            vec![IrOp::JumpIf { cond: vr(1), then_target: bb(2), else_target: bb(0) }];
        func.blocks.push(bb1);

        // bb2: 内层循环体
        let mut bb2 = BasicBlock::new(bb(2));
        bb2.instructions = vec![IrOp::Jump { target: bb(1) }];
        func.blocks.push(bb2);

        // bb3: 退出
        let mut bb3 = BasicBlock::new(bb(3));
        bb3.instructions = vec![IrOp::Return { value: None }];
        func.blocks.push(bb3);

        let back_edges = find_back_edges(&func);
        assert_eq!(back_edges.len(), 2);
        // bb2 → bb1 (内层回边)
        // bb1 → bb0 (外层回边) — 注意 bb1 的 else_target 是 bb0，
        //   但 bb0 在 DFS 中是 bb1 的祖先，所以这是回边
        assert!(back_edges.contains(&(bb(2), bb(1))));
        assert!(back_edges.contains(&(bb(1), bb(0))));
    }

    // ── apply_loop_protection 测试 ──

    #[test]
    fn test_loop_protection_increments_header_sources() {
        // 循环头 bb0 中使用了 v0，循环保护后 v0 的计数 +1
        let mut func = make_func();
        func.blocks[0].instructions = vec![
            IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) },
            IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) },
            IrOp::JumpIf { cond: vr(2), then_target: bb(1), else_target: bb(2) },
        ];

        let mut bb1 = BasicBlock::new(bb(1));
        bb1.instructions = vec![IrOp::Jump { target: bb(0) }];
        func.blocks.push(bb1);

        let mut bb2 = BasicBlock::new(bb(2));
        bb2.instructions = vec![IrOp::Return { value: None }];
        func.blocks.push(bb2);

        let mut counts = count_usages(&func);
        let v0_before = counts.get(&0).copied().unwrap_or(0);

        let back_edges = find_back_edges(&func);
        apply_loop_protection(&mut counts, &func, &back_edges);

        let v0_after = counts.get(&0).copied().unwrap_or(0);
        assert_eq!(v0_after, v0_before + 1); // v0 在循环头中额外 +1
    }

    // ── count_usages_with_loop_protection 集成测试 ──

    #[test]
    fn test_integrated_with_loop_protection() {
        let mut func = make_func();
        func.blocks[0].instructions = vec![
            IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) },
            IrOp::JumpIf { cond: vr(2), then_target: bb(1), else_target: bb(2) },
        ];

        let mut bb1 = BasicBlock::new(bb(1));
        bb1.instructions = vec![IrOp::Jump { target: bb(0) }];
        func.blocks.push(bb1);

        let mut bb2 = BasicBlock::new(bb(2));
        bb2.instructions = vec![IrOp::Return { value: None }];
        func.blocks.push(bb2);

        let counts_plain = count_usages(&func);
        let counts_protected = count_usages_with_loop_protection(&func);

        // 保护后每个在循环头中使用的值计数都比 plain 版多 1
        // bb0 中 src: v0, v1, v2
        for id in [0u32, 1u32, 2u32] {
            let plain = counts_plain.get(&id).copied().unwrap_or(0);
            let protected = counts_protected.get(&id).copied().unwrap_or(0);
            assert_eq!(protected, plain + 1, "v{} should be incremented by loop protection", id);
        }
    }

    // ── successors_of 测试 ──

    #[test]
    fn test_successors_jump() {
        let op = IrOp::Jump { target: BasicBlockId::new(5) };
        let (len, succs) = successors_of(&op);
        assert_eq!(len, 1);
        assert_eq!(succs[0], Some(BasicBlockId::new(5)));
        assert_eq!(succs[1], None);
    }

    #[test]
    fn test_successors_jump_if() {
        let op = IrOp::JumpIf {
            cond: ValueRef::new(0),
            then_target: BasicBlockId::new(1),
            else_target: BasicBlockId::new(2),
        };
        let (len, succs) = successors_of(&op);
        assert_eq!(len, 2);
        assert_eq!(succs[0], Some(BasicBlockId::new(1)));
        assert_eq!(succs[1], Some(BasicBlockId::new(2)));
    }

    #[test]
    fn test_successors_return() {
        let op = IrOp::Return { value: Some(ValueRef::new(0)) };
        let (len, succs) = successors_of(&op);
        assert_eq!(len, 0);
        assert_eq!(succs[0], None);
        assert_eq!(succs[1], None);
    }
}
