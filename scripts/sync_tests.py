#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Nuzo Lang 测试桩同步脚本

基于 CALL_GRAPH.md 提取所有 pub fn，扫描已有测试，为未覆盖的函数生成测试桩。

工作模式:
- dry-run（默认）: 只报告覆盖率缺口，不写文件
- 写入模式 (--no-dry-run): 生成测试桩 include 文件到 tests/generated/auto_sync_tests.inc
  并生成覆盖率报告到 tests/generated/coverage_gaps.md

测试桩特性:
- 标注 #[test] #[ignore]，避免污染 cargo test（需 --ignored 显式运行）
- 幂等: 多次运行结果一致（完全重写文件）
- 自动去重: 跳过已有测试覆盖的函数

用法:
    python scripts/sync_tests.py --project . --callgraph CALL_GRAPH.md
    python scripts/sync_tests.py --project . --callgraph CALL_GRAPH.md --no-dry-run

设计原则:
- 基于 CALL_GRAPH.md（唯一权威）提取 pub fn
- 不破坏手写测试: 只生成独立文件，不修改现有测试
- UTF-8 不带 BOM，Windows 兼容
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

from _common import (
    PubFn,
    SyncReport,
    collect_rust_files,
    parse_callgraph,
    parse_test_functions,
    read_file_utf8,
    setup_utf8_output,
    write_file_utf8,
)

# ============================================================================
# 路径常量（相对于项目根）
# ============================================================================

# 测试桩输出文件（相对于项目根）
TEST_STUBS_FILE = Path("tests/generated/auto_sync_tests.inc")
COVERAGE_REPORT_FILE = Path("tests/generated/coverage_gaps.md")

# 扫描已有测试的目录（相对于项目根）
EXISTING_TEST_SCAN_DIRS = [
    Path("crates"),
    Path("tests"),
    Path("src"),
]


# ============================================================================
# 解析函数（任务接口）
# ============================================================================


def parse_callgraph_file(callgraph_md: Path) -> list[PubFn]:
    """解析 CALL_GRAPH.md，提取 pub fn 列表。

    Args:
        callgraph_md: CALL_GRAPH.md 文件路径

    Returns:
        PubFn 列表
    """
    content = read_file_utf8(callgraph_md)
    return parse_callgraph(content)


def parse_existing_tests(scan_dirs: list[Path], exclude: Optional[Path] = None) -> set[str]:
    """扫描目录下所有 .rs 文件，提取已有测试函数名集合。

    扫描范围包括 #[test] 函数和 #[ignore] 函数，
    确保自动生成的测试桩不会与手写测试冲突。

    Args:
        scan_dirs: 要扫描的目录列表
        exclude: 需要排除的文件路径（如自动生成的测试桩文件本身，确保幂等性）

    Returns:
        测试函数名集合
    """
    all_tests: set[str] = set()
    exclude_resolved = exclude.resolve() if exclude else None
    for rs_file in collect_rust_files(scan_dirs):
        if exclude_resolved and rs_file.resolve() == exclude_resolved:
            continue  # 排除自动生成的文件，确保幂等性
        try:
            content = read_file_utf8(rs_file)
            all_tests.update(parse_test_functions(content))
        except (OSError, UnicodeDecodeError):
            # 跳过无法读取的文件
            continue
    return all_tests


# ============================================================================
# 测试桩生成
# ============================================================================


def generate_test_stub(fn: PubFn) -> str:
    """为单个 pub fn 生成测试桩代码。

    格式:
        #[test]
        #[ignore = "auto-generated stub: TODO implement"]
        fn test_xxx() {
            // Source: src/path.rs:lines
            // Signature: pub fn ...
            todo!("implement test for Class::method")
        }

    Args:
        fn: PubFn 条目

    Returns:
        Rust 测试桩源码字符串
    """
    lines = [
        "#[test]",
        '#[ignore = "auto-generated stub: TODO implement"]',
        f"fn {fn.test_name}() {{",
        f"    // Target: {fn.path}",
        f"    // Source: {fn.source_file}:{fn.line_range}",
        f"    // Signature: {fn.signature}",
        f'    todo!("implement test for {fn.path}")',
        "}",
    ]
    return "\n".join(lines)


def generate_test_stubs_file(
    missing_fns: list[PubFn], project_root: Path
) -> str:
    """生成完整的测试桩文件内容。

    包含文件头注释、必要的 use 语句、所有测试桩。
    文件可独立编译（不依赖项目内部类型，使用 todo!() 占位）。

    Args:
        missing_fns: 缺失测试的 pub fn 列表
        project_root: 项目根（用于生成相对路径注释）

    Returns:
        完整的 .rs 文件内容
    """
    now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")

    header = [
        "// ============================================================================",
        "// AUTO-GENERATED TEST STUBS - DO NOT EDIT MANUALLY",
        "//",
        "// 此文件由 scripts/sync_tests.py 自动生成。",
        "// 所有测试桩标注 #[ignore]，不会在默认 `cargo test` 中执行，",
        "// 可通过 `cargo test --features generated-test-stubs --test generated_stubs --no-run` 显式编译校验。",
        "//",
        f"// 生成时间: {now}",
        f"// 未覆盖 pub fn 数量: {len(missing_fns)}",
        "// 数据源: CALL_GRAPH.md",
        "//",
        "// 如需实现某个测试，请将其从此文件复制到对应 crate 的 tests/ 目录，",
        "// 移除 #[ignore] 标注，并实现测试逻辑。",
        "// ============================================================================",
        "",
    ]

    if not missing_fns:
        header.extend(
            [
                "// 所有 pub fn 均已有测试覆盖，无需生成测试桩。",
                "",
                "fn _auto_sync_no_op() {}",
            ]
        )
        return "\n".join(header)

    # 按模块分组生成测试桩
    by_module: dict[str, list[PubFn]] = {}
    for fn in missing_fns:
        by_module.setdefault(fn.module, []).append(fn)

    body: list[str] = []
    for module in sorted(by_module.keys()):
        fns = by_module[module]
        body.append(f"// ── Module: {module} ({len(fns)} uncovered) ──────────────")
        body.append("")
        for fn in fns:
            body.append(generate_test_stub(fn))
            body.append("")
        body.append("")

    return "\n".join(header + body)


def generate_coverage_report(
    all_fns: list[PubFn],
    missing_fns: list[PubFn],
    existing_count: int,
) -> str:
    """生成覆盖率缺口报告（Markdown 格式）。

    Args:
        all_fns: CALL_GRAPH 中的所有 pub fn
        missing_fns: 未覆盖的 pub fn
        existing_count: 已有测试函数总数

    Returns:
        Markdown 报告字符串
    """
    now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
    total = len(all_fns)
    missing = len(missing_fns)
    covered = total - missing
    coverage_pct = (covered / total * 100) if total > 0 else 100.0

    lines = [
        "# 测试覆盖率缺口报告",
        "",
        f"> 自动生成于 {now} | 数据源: CALL_GRAPH.md",
        "",
        "## 概览",
        "",
        f"- CALL_GRAPH pub fn 总数: **{total}**",
        f"- 已有测试函数总数: **{existing_count}**",
        f"- 已覆盖 pub fn: **{covered}**",
        f"- 未覆盖 pub fn: **{missing}**",
        f"- 覆盖率: **{coverage_pct:.1f}%**",
        "",
        "## 未覆盖 pub fn 明细",
        "",
        "| 模块 | 函数路径 | 行号 | 签名 |",
        "|------|---------|------|------|",
    ]

    # 按模块分组排序
    sorted_missing = sorted(missing_fns, key=lambda f: (f.module, f.path))
    for fn in sorted_missing:
        # 转义 Markdown 表格中的 | 字符
        sig = fn.signature.replace("|", "\\|")
        path = fn.path.replace("|", "\\|")
        lines.append(f"| {fn.module} | {path} | {fn.line_range} | {sig} |")

    lines.extend(
        [
            "",
            "## 建议",
            "",
            "1. 优先覆盖核心模块: `vm`, `compiler`, `bytecode`, `values`",
            "2. 构造函数 (`new`/`default`) 应优先测试",
            "3. 使用 `cargo test --features generated-test-stubs --test generated_stubs --no-run` 校验自动生成的测试桩可编译",
            "4. 实现测试后，将对应条目从 `tests/generated/auto_sync_tests.inc` 移除",
            "",
        ]
    )

    return "\n".join(lines)


# ============================================================================
# 核心同步逻辑
# ============================================================================


def sync_tests(
    project_root: Path, callgraph_path: Path, dry_run: bool = True
) -> SyncReport:
    """主同步函数: 基于 CALL_GRAPH 生成测试桩。

    Args:
        project_root: 项目根目录
        callgraph_path: CALL_GRAPH.md 路径（绝对或相对于 project_root）
        dry_run: True=只报告, False=写入测试桩文件

    Returns:
        SyncReport 同步报告
    """
    report = SyncReport(dry_run=dry_run, script_name="sync_tests")

    # 解析 CALL_GRAPH.md
    if not callgraph_path.is_absolute():
        callgraph_path = project_root / callgraph_path
    if not callgraph_path.exists():
        report.add_err(f"CALL_GRAPH.md 不存在: {callgraph_path}")
        return report

    all_fns = parse_callgraph_file(callgraph_path)
    report.add_op(f"CALL_GRAPH 提取 pub fn: {len(all_fns)} 个")

    # 扫描已有测试（排除自动生成的测试桩文件，确保幂等性）
    scan_dirs = [project_root / d for d in EXISTING_TEST_SCAN_DIRS]
    test_stubs_path = project_root / TEST_STUBS_FILE
    existing_tests = parse_existing_tests(scan_dirs, exclude=test_stubs_path)
    report.add_op(f"已有测试函数: {len(existing_tests)} 个")

    # 识别未覆盖的 pub fn
    # 判断逻辑: 测试函数名精确匹配（test_name 在已有测试集合中）
    # P2-2 修复: 删除宽松匹配（fn_name_lower in t.lower()），避免假阳性
    # （如 new 匹配 test_newton）。改为精确匹配，覆盖率更准确。
    missing_fns: list[PubFn] = []
    seen_test_names: set[str] = set()
    for fn in all_fns:
        if fn.test_name in existing_tests:
            continue
        # 去重：同一 test_name 只生成一次（CALL_GRAPH 可能多次列出同一函数）
        if fn.test_name in seen_test_names:
            continue
        seen_test_names.add(fn.test_name)
        missing_fns.append(fn)

    covered = len(all_fns) - len(missing_fns)
    coverage_pct = (covered / len(all_fns) * 100) if all_fns else 100.0
    report.add_op(
        f"覆盖率: {covered}/{len(all_fns)} ({coverage_pct:.1f}%), "
        f"未覆盖: {len(missing_fns)}"
    )

    # dry-run 模式: 只报告
    if dry_run:
        if missing_fns:
            report.add_op(f"--- 未覆盖 pub fn 示例（前 10 个）---")
            for fn in missing_fns[:10]:
                report.add_op(
                    f"  {fn.module} :: {fn.path} ({fn.source_file}:{fn.line_range})"
                )
            if len(missing_fns) > 10:
                report.add_op(f"  ... 还有 {len(missing_fns) - 10} 个")
        report.add_op(
            "dry-run 模式: 未写入文件。使用 --no-dry-run 生成测试桩。"
        )
        return report

    # 写入模式: 生成测试桩文件
    coverage_path = project_root / COVERAGE_REPORT_FILE

    # 确保目录存在
    test_stubs_path.parent.mkdir(parents=True, exist_ok=True)

    # 生成并写入测试桩文件
    stubs_content = generate_test_stubs_file(missing_fns, project_root)
    write_file_utf8(test_stubs_path, stubs_content)
    report.add_op(f"已生成测试桩: {test_stubs_path} ({len(missing_fns)} 个)")

    # 生成并写入覆盖率报告
    report_content = generate_coverage_report(
        all_fns, missing_fns, len(existing_tests)
    )
    write_file_utf8(coverage_path, report_content)
    report.add_op(f"已生成覆盖率报告: {coverage_path}")

    # 提示 opt-in 运行方式
    report.add_warn(
        "注意: 自动生成测试桩默认不参与编译；"
        "需通过 generated-test-stubs feature 与 generated_stubs harness 显式启用。"
    )

    return report


# ============================================================================
# CLI 入口
# ============================================================================


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Nuzo Lang 测试桩同步脚本",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "示例:\n"
            "  python scripts/sync_tests.py --project . --callgraph CALL_GRAPH.md\n"
            "  python scripts/sync_tests.py --project . --callgraph CALL_GRAPH.md --no-dry-run\n"
        ),
    )
    parser.add_argument(
        "--project",
        default=".",
        help="项目根目录（默认: 当前目录）",
    )
    parser.add_argument(
        "--callgraph",
        default="CALL_GRAPH.md",
        help="CALL_GRAPH.md 路径（相对于 --project，默认: CALL_GRAPH.md）",
    )
    sync_group = parser.add_mutually_exclusive_group()
    sync_group.add_argument(
        "--dry-run",
        action="store_true",
        default=True,
        help="只报告不写文件（默认行为）",
    )
    sync_group.add_argument(
        "--no-dry-run",
        dest="dry_run",
        action="store_false",
        help="生成测试桩文件和覆盖率报告",
    )
    args = parser.parse_args()

    setup_utf8_output()

    project_root = Path(args.project).resolve()
    if not project_root.exists():
        print(f"错误: 项目根目录不存在: {project_root}")
        return 1

    report = sync_tests(project_root, Path(args.callgraph), dry_run=args.dry_run)
    print(report)
    return 1 if report.errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
