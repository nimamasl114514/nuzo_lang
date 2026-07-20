#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Nuzo Lang Opcode 跨 crate 同步脚本

扫描 opcode.rs 的 Instruction 枚举变更，同步到:
- dispatch_table.rs: 检查 _op_xxx handler 函数是否齐全
- codegen.rs: 检查 IR->Instruction 映射是否齐全

工作模式:
- dry-run（默认）: 只报告缺失项，打印骨架代码，不修改文件
- 写入模式 (--no-dry-run): 在 AUTO-GENERATED 区域插入骨架代码

用法:
    python scripts/sync_opcode.py --project . --dry-run
    python scripts/sync_opcode.py --project . --no-dry-run

设计原则:
- SSOT: opcode.rs 的 Instruction 枚举是唯一数据源
- 幂等性: 多次运行结果一致
- 不破坏手写代码: 只在 AUTO-GENERATED 标记区域插入
"""

from __future__ import annotations

import argparse
from pathlib import Path

from _common import (
    AUTO_BEGIN,
    AUTO_END,
    InstructionVariant,
    SyncReport,
    find_auto_region,
    parse_codegen_instructions,
    parse_handler_functions,
    parse_instruction_enum,
    parse_skip_ssot_variants,
    read_file_utf8,
    replace_auto_region,
    setup_utf8_output,
    write_file_utf8,
)

# ============================================================================
# 路径常量（相对于项目根，非硬编码绝对路径）
# ============================================================================

OPCODE_RS = Path("crates/nuzo-bytecode/src/opcode.rs")
DISPATCH_RS = Path("crates/nuzo-vm/src/dispatch_table.rs")
CODEGEN_RS = Path("crates/nuzo-compiler/src/codegen.rs")

# AUTO-GENERATED 区域标记（用于 dispatch_table.rs 中的 handler 骨架）
DISPATCH_MARKER = "sync_opcode_handlers"


# ============================================================================
# 解析函数（任务接口）
# ============================================================================


def parse_instruction_enum_file(opcode_rs: Path) -> list[InstructionVariant]:
    """解析 opcode.rs 的 Instruction 枚举，返回变体列表。

    Args:
        opcode_rs: opcode.rs 文件路径

    Returns:
        InstructionVariant 列表
    """
    content = read_file_utf8(opcode_rs)
    return parse_instruction_enum(content, "Instruction")


def parse_dispatch_handlers(dispatch_rs: Path) -> list[str]:
    """解析 dispatch_table.rs 的 _op_xxx handler 函数名列表。

    Args:
        dispatch_rs: dispatch_table.rs 文件路径

    Returns:
        handler 函数名列表（如 ["_op_loadk", ...]）
    """
    content = read_file_utf8(dispatch_rs)
    return parse_handler_functions(content)


def parse_codegen_mappings(codegen_rs: Path) -> list[str]:
    """解析 codegen.rs 中使用的 Instruction::Xxx 引用列表。

    Args:
        codegen_rs: codegen.rs 文件路径

    Returns:
        Instruction 变体名列表（去重，按首次出现顺序）
    """
    content = read_file_utf8(codegen_rs)
    return parse_codegen_instructions(content)


def parse_skip_ssot_variants_file(opcode_rs: Path) -> set[str]:
    """解析 opcode.rs 中标注了 #[opcode_meta(skip_ssot)] 的变体名。

    实现 SSOT：从 opcode.rs 动态提取 skip_ssot 变体，避免与硬编码列表失同步。

    Args:
        opcode_rs: opcode.rs 文件路径

    Returns:
        标注了 skip_ssot 的变体名集合（如 {"Halt", "Capture"}）
    """
    content = read_file_utf8(opcode_rs)
    return parse_skip_ssot_variants(content, "Instruction")


# ============================================================================
# 骨架代码生成
# ============================================================================


def _escape_rust_str(s: str) -> str:
    """转义字符串使其安全地嵌入 Rust 字符串字面量（"..."）。

    防止字段值包含 " 或 \ 导致生成的 Rust 代码语法错误。
    """
    return s.replace("\\", "\\\\").replace('"', '\\"')


def _escape_rust_comment(s: str) -> str:
    """转义字符串使其安全地嵌入 Rust 行注释（// ...）。

    防止字段值包含换行符导致注释被截断（后续行不会被当作注释）。
    """
    return s.replace("\r\n", " ").replace("\n", " ").replace("\r", " ")


def generate_handler_skeleton(variant: InstructionVariant) -> str:
    """为缺失的 Instruction 变体生成 dispatch handler 骨架代码。

    骨架使用 todo!() 占位，返回 Result<(), NuzoError>。
    用户需手动实现具体逻辑后移除 todo!()。

    Args:
        variant: Instruction 变体

    Returns:
        Rust 源码字符串（含注释和函数体）
    """
    # 转义字段防止特殊字符破坏生成的 Rust 语法
    name = _escape_rust_comment(variant.name)
    handler = _escape_rust_comment(variant.handler_name)
    fields = _escape_rust_comment(variant.field_summary)
    # todo!() 字符串字面量需要额外的字符串转义（处理 " 和 \）
    name_str = _escape_rust_str(variant.name)
    lines = [
        f"/// TODO: implement handler for {name}",
        f"/// Fields: {fields}",
        f"/// NOTE: 此骨架假设 `super::VM` 和 `NuzoError` 在作用域内。",
        f"///       请根据 dispatch_table.rs 实际导入情况调整路径",
        f"///       （如 `crate::vm::VM`、`crate::error::NuzoError`）。",
        f"fn {handler}(_vm: &mut super::VM) -> Result<(), NuzoError> {{",
        f"    // TODO: implement {name} handler",
        f'    todo!("implement {name_str} handler")',
        f"}}",
    ]
    return "\n".join(lines)


def generate_codegen_stub(variant: InstructionVariant) -> str:
    """为缺失的 codegen 映射生成注释提示（不生成可执行代码）。

    codegen 的 IrOp->Instruction 映射需要人工设计，只生成提示注释。
    """
    # 转义字段防止换行符截断注释
    name = _escape_rust_comment(variant.name)
    fields = _escape_rust_comment(variant.field_summary)
    return (
        f"// TODO: codegen.rs 缺少 Instruction::{name} 的发射逻辑\n"
        f"// Fields: {fields}"
    )


# ============================================================================
# 核心同步逻辑
# ============================================================================


def _check_dispatch_handlers(
    variants: list[InstructionVariant],
    handlers: list[str],
    report: SyncReport,
) -> list[InstructionVariant]:
    """检查 dispatch_table.rs 的 handler 覆盖情况。

    Returns:
        缺失 handler 的变体列表
    """
    handler_set = set(handlers)
    missing: list[InstructionVariant] = []
    for v in variants:
        if v.handler_name not in handler_set:
            missing.append(v)
            report.add_op(
                f"dispatch 缺失 handler: {v.handler_name} (for Instruction::{v.name})"
            )
    if not missing:
        report.add_op(f"dispatch handler 覆盖完整 ({len(variants)} 个变体)")
    return missing


def _check_codegen_mappings(
    variants: list[InstructionVariant],
    codegen_refs: list[str],
    report: SyncReport,
    special_cases: set[str],
) -> list[InstructionVariant]:
    """检查 codegen.rs 的 Instruction 引用覆盖情况。

    注意: 某些变体（如 Halt）由特殊路径处理，不一定在 codegen 中直接引用。
    此检查为信息性，缺失项标记为 warning 而非 error。

    Args:
        variants: Instruction 变体列表
        codegen_refs: codegen.rs 中引用的变体名列表
        report: 同步报告
        special_cases: 由特殊路径处理的变体名集合（从 opcode.rs 的
            #[opcode_meta(skip_ssot)] 属性动态解析，实现 SSOT）

    Returns:
        codegen 中未引用的变体列表
    """
    ref_set = set(codegen_refs)
    missing: list[InstructionVariant] = []
    for v in variants:
        if v.name in ref_set:
            continue
        if v.name in special_cases:
            report.add_op(
                f"codegen 未引用 Instruction::{v.name}（特殊路径处理，可接受）"
            )
        else:
            missing.append(v)
            report.add_warn(
                f"codegen 未引用 Instruction::{v.name}（可能缺失 IR->Instruction 映射）"
            )
    if not missing:
        report.add_op(f"codegen 映射覆盖完整（排除 {len(special_cases)} 个特殊变体）")
    return missing


def _write_handler_skeletons(
    dispatch_rs: Path,
    missing: list[InstructionVariant],
    report: SyncReport,
) -> None:
    """将 handler 骨架写入 dispatch_table.rs 的 AUTO-GENERATED 区域。

    如果区域不存在，报告警告并跳过（避免破坏文件结构）。
    """
    if not missing:
        return

    content = read_file_utf8(dispatch_rs)
    region = find_auto_region(content, DISPATCH_MARKER)

    if region is None:
        report.add_warn(
            f"{dispatch_rs} 未找到 '{AUTO_BEGIN} {DISPATCH_MARKER}' 区域，"
            f"请手动添加标记后重试，或手动插入以下骨架代码:"
        )
        for v in missing:
            skeleton = generate_handler_skeleton(v)
            report.add_op(f"--- 骨架 ---\n{skeleton}\n---")
        return

    # 生成骨架代码块
    skeletons = []
    for v in missing:
        skeletons.append(generate_handler_skeleton(v))
    new_body = "\n\n".join(skeletons)

    new_content = replace_auto_region(content, new_body, DISPATCH_MARKER)
    if new_content != content:
        write_file_utf8(dispatch_rs, new_content)
        report.add_op(
            f"已写入 {len(missing)} 个 handler 骨架到 {dispatch_rs} "
            f"(区域: {DISPATCH_MARKER})"
        )
    else:
        report.add_warn(f"{dispatch_rs} 内容未变化（骨架可能已存在）")


def sync_opcode(project_root: Path, dry_run: bool = True) -> SyncReport:
    """主同步函数: 扫描 opcode.rs 并检查 dispatch/codegen 同步状态。

    Args:
        project_root: 项目根目录
        dry_run: True=只报告不修改, False=写入骨架代码

    Returns:
        SyncReport 同步报告
    """
    report = SyncReport(dry_run=dry_run, script_name="sync_opcode")

    # 解析三个文件
    opcode_rs = project_root / OPCODE_RS
    dispatch_rs = project_root / DISPATCH_RS
    codegen_rs = project_root / CODEGEN_RS

    # 检查文件存在性（预编译检查）
    # 这些源文件应由 cargo build 生成或已存在于源码树中。
    # 若缺失通常是 project_root 错误或未执行预编译步骤。
    for f in (opcode_rs, dispatch_rs, codegen_rs):
        if not f.exists():
            report.add_err(
                f"文件不存在: {f}\n"
                f"  请确认 project_root 正确（当前: {project_root}），\n"
                f"  或运行 `cargo build --workspace` 执行预编译步骤生成所需文件。"
            )
            return report

    variants = parse_instruction_enum_file(opcode_rs)
    handlers = parse_dispatch_handlers(dispatch_rs)
    codegen_refs = parse_codegen_mappings(codegen_rs)
    # P2-1: 从 opcode.rs 动态解析 skip_ssot 变体，实现 SSOT
    special_cases = parse_skip_ssot_variants_file(opcode_rs)

    report.add_op(f"Instruction 枚举: {len(variants)} 个变体")
    report.add_op(f"dispatch handlers: {len(handlers)} 个")
    report.add_op(f"codegen 引用: {len(codegen_refs)} 个 Instruction 变体")
    report.add_op(
        f"skip_ssot 特殊变体: {sorted(special_cases) if special_cases else '(无)'}"
    )

    # 检查 dispatch handler 覆盖
    missing_handlers = _check_dispatch_handlers(variants, handlers, report)

    # 检查 codegen 映射覆盖
    missing_codegen = _check_codegen_mappings(
        variants, codegen_refs, report, special_cases
    )

    # dry-run 模式下打印骨架代码
    if dry_run:
        if missing_handlers:
            report.add_op(f"--- 缺失 handler 骨架 ({len(missing_handlers)} 个) ---")
            for v in missing_handlers:
                report.add_op(generate_handler_skeleton(v))
        if missing_codegen:
            report.add_op(f"--- 缺失 codegen 映射提示 ({len(missing_codegen)} 个) ---")
            for v in missing_codegen:
                report.add_op(generate_codegen_stub(v))
    else:
        # 写入模式: 将骨架写入 AUTO-GENERATED 区域
        if missing_handlers:
            _write_handler_skeletons(dispatch_rs, missing_handlers, report)
        if missing_codegen:
            report.add_warn(
                "codegen 映射需人工设计，未自动写入。请参考上述提示手动实现。"
            )

    return report


# ============================================================================
# CLI 入口
# ============================================================================


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Nuzo Lang Opcode 跨 crate 同步脚本",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "示例:\n"
            "  python scripts/sync_opcode.py --project . --dry-run\n"
            "  python scripts/sync_opcode.py --project . --no-dry-run\n"
        ),
    )
    parser.add_argument(
        "--project",
        default=".",
        help="项目根目录（默认: 当前目录）",
    )
    sync_group = parser.add_mutually_exclusive_group()
    sync_group.add_argument(
        "--dry-run",
        action="store_true",
        default=True,
        help="只报告不修改（默认行为）",
    )
    sync_group.add_argument(
        "--no-dry-run",
        dest="dry_run",
        action="store_false",
        help="执行写入（在 AUTO-GENERATED 区域插入骨架）",
    )
    args = parser.parse_args()

    setup_utf8_output()

    project_root = Path(args.project).resolve()
    if not project_root.exists():
        print(f"错误: 项目根目录不存在: {project_root}")
        return 1

    report = sync_opcode(project_root, dry_run=args.dry_run)
    print(report)
    return 1 if report.errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
