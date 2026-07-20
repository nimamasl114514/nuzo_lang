#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
nuzo_run CLI 性能测试脚本

测试 nuzo_run 在各种场景下的端到端执行性能（含进程启动开销）。
每个场景运行多次取中位数，输出表格便于横向对比。

用法:
    python tools/perf_cli.py
    python tools/perf_cli.py --runs 10
    python tools/perf_cli.py --exe d:/path/to/nuzo_run.exe
"""

from __future__ import annotations

import argparse
import os
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# 配置
# ---------------------------------------------------------------------------

# 默认 exe 路径：优先环境变量 NUZO_RUN_EXE，其次基于脚本位置推导
_SCRIPT_DIR = Path(__file__).resolve().parent
_PROJECT_ROOT = _SCRIPT_DIR.parent
_DEFAULT_EXE_FROM_HERE = str(_PROJECT_ROOT / "target" / "release" / "nuzo_run.exe")
DEFAULT_EXE = os.environ.get("NUZO_RUN_EXE", _DEFAULT_EXE_FROM_HERE)
DEFAULT_RUNS = 5
TIMEOUT_S = 120  # 单次执行超时，秒

# ---------------------------------------------------------------------------
# 测试场景
# ---------------------------------------------------------------------------

# 生成 1..100 的数组字面量: [1, 2, 3, ..., 100]
_ARRAY_100 = "[" + ", ".join(str(i) for i in range(1, 101)) + "]"

# fib(20) 的 Nuzo 实现
_FIB_CODE = """fn fib(n) {
  if n < 2 { return n }
  return fib(n - 1) + fib(n - 2)
}
print(fib(20))"""

SCENARIOS: list[tuple[str, str]] = [
    ("空脚本", ""),
    ("简单加法", "print(1 + 2)"),
    ("循环 1000 次", "for i in 0..1000 { print(i) }"),
    ("循环 10000 次", "for i in 0..10000 { print(i) }"),
    ("递归 fib(20)", _FIB_CODE),
    ("数组创建 100", f"arr = {_ARRAY_100}\nprint(len(arr))"),
    ("字符串拼接", 'print("hello" + " " + "world")'),
]


# ---------------------------------------------------------------------------
# 数据结构
# ---------------------------------------------------------------------------


@dataclass
class ScenarioResult:
    """单个场景的测量结果。"""

    name: str
    median_ms: Optional[float]
    min_ms: Optional[float]
    max_ms: Optional[float]
    samples_ms: list[float]
    error: Optional[str]
    sample_output: str  # 预览首次运行的输出（截断）

    @property
    def ok(self) -> bool:
        return self.error is None and self.median_ms is not None


# ---------------------------------------------------------------------------
# 执行逻辑
# ---------------------------------------------------------------------------


def run_once(exe: str, code: str) -> subprocess.CompletedProcess:
    """执行一次 nuzo_run -e <code>，stdout 丢弃以避免 Python 缓冲开销影响计时。"""
    return subprocess.run(
        [exe, "-e", code],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        timeout=TIMEOUT_S,
    )


def measure_scenario(exe: str, name: str, code: str, runs: int) -> ScenarioResult:
    """测量单个场景：先做验证运行，再做多次计时运行。

    统一使用 -e 内联求值（Python subprocess.run 以列表传参，绕过 PowerShell
    引号解析，可正确处理空字符串、多行代码与 CJK 字符）。
    """
    # 验证运行：捕获 stdout 用于预览，捕获 stderr 用于错误诊断
    try:
        verify = subprocess.run(
            [exe, "-e", code],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=TIMEOUT_S,
        )
    except subprocess.TimeoutExpired:
        return ScenarioResult(
            name=name,
            median_ms=None,
            min_ms=None,
            max_ms=None,
            samples_ms=[],
            error=f"timeout ({TIMEOUT_S}s) during verify",
            sample_output="",
        )
    if verify.returncode != 0:
        err_msg = verify.stderr.decode("utf-8", errors="replace").strip()
        return ScenarioResult(
            name=name,
            median_ms=None,
            min_ms=None,
            max_ms=None,
            samples_ms=[],
            error=f"exit={verify.returncode}: {err_msg[:200]}",
            sample_output="",
        )

    sample_output = verify.stdout.decode("utf-8", errors="replace").strip()[:60]

    # 计时运行
    samples: list[float] = []
    for _ in range(runs):
        t0 = time.perf_counter()
        try:
            proc = run_once(exe, code)
        except subprocess.TimeoutExpired:
            return ScenarioResult(
                name=name,
                median_ms=None,
                min_ms=None,
                max_ms=None,
                samples_ms=[],
                error=f"timeout ({TIMEOUT_S}s)",
                sample_output=sample_output,
            )
        t1 = time.perf_counter()
        if proc.returncode != 0:
            err_msg = proc.stderr.decode("utf-8", errors="replace").strip()
            return ScenarioResult(
                name=name,
                median_ms=None,
                min_ms=None,
                max_ms=None,
                samples_ms=[],
                error=f"exit={proc.returncode} during timing: {err_msg[:200]}",
                sample_output=sample_output,
            )
        samples.append((t1 - t0) * 1000.0)  # 转毫秒

    return ScenarioResult(
        name=name,
        median_ms=statistics.median(samples),
        min_ms=min(samples),
        max_ms=max(samples),
        samples_ms=samples,
        error=None,
        sample_output=sample_output,
    )


# ---------------------------------------------------------------------------
# 输出格式化
# ---------------------------------------------------------------------------


def format_table(results: list[ScenarioResult]) -> str:
    """生成对齐的表格输出。"""
    headers = ["场景", "中位数(ms)", "最小(ms)", "最大(ms)", "输出预览"]
    # 计算每列宽度（考虑中文字符显示宽度）
    def display_width(s: str) -> int:
        """估算字符串显示宽度（CJK 字符算 2，其余算 1）。"""
        w = 0
        for ch in s:
            w += 2 if ord(ch) > 0x2E80 else 1
        return w

    def pad(s: str, width: int, align_left: bool = True) -> str:
        dw = display_width(s)
        fill = max(0, width - dw)
        if align_left:
            return s + " " * fill
        return " " * fill + s

    rows = [headers]
    for r in results:
        if r.ok:
            rows.append([
                r.name,
                f"{r.median_ms:.3f}",
                f"{r.min_ms:.3f}",
                f"{r.max_ms:.3f}",
                r.sample_output or "(无输出)",
            ])
        else:
            rows.append([
                r.name, "-", "-", "-", f"[失败] {r.error}",
            ])

    # 计算列宽
    col_widths = [
        max(display_width(row[i]) for row in rows) for i in range(len(headers))
    ]

    # 构建表格
    lines = []
    # 表头
    header_line = " | ".join(
        pad(headers[i], col_widths[i]) for i in range(len(headers))
    )
    sep_line = "-+-".join("-" * w for w in col_widths)
    lines.append(header_line)
    lines.append(sep_line)
    for row in rows[1:]:
        lines.append(" | ".join(
            pad(row[i], col_widths[i], align_left=(i == 0))
            for i in range(len(headers))
        ))
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="nuzo_run CLI 性能测试"
    )
    parser.add_argument("--exe", default=DEFAULT_EXE, help="nuzo_run.exe 路径")
    parser.add_argument("--runs", type=int, default=DEFAULT_RUNS, help="每个场景运行次数")
    parser.add_argument("--list", action="store_true", help="仅列出场景不执行")
    args = parser.parse_args()

    exe = os.path.abspath(args.exe)
    if not args.list:
        if not Path(exe).is_file():
            print(f"错误: 找不到可执行文件: {exe}", file=sys.stderr)
            return 2
        # 验证可执行文件能运行
        try:
            chk = subprocess.run(
                [exe, "--version"], capture_output=True, timeout=10
            )
            if chk.returncode != 0:
                print(f"错误: {exe} 无法启动 (exit={chk.returncode})", file=sys.stderr)
                return 2
            version = chk.stdout.decode("utf-8", errors="replace").strip()
            print(f"# nuzo_run 性能测试")
            print(f"# 二进制: {exe}")
            print(f"# 版本: {version}")
            print(f"# 运行次数/场景: {args.runs}")
            print(f"# 计时器: time.perf_counter (含进程启动开销)")
            print()
        except subprocess.TimeoutExpired:
            print(f"错误: {exe} 启动超时", file=sys.stderr)
            return 2
    else:
        for name, code in SCENARIOS:
            print(f"=== {name} ===")
            print(code)
            print()
        return 0

    results: list[ScenarioResult] = []
    for name, code in SCENARIOS:
        print(f"测量中: {name} ...", flush=True)
        r = measure_scenario(exe, name, code, args.runs)
        results.append(r)
    print()  # 进度行与结果表格之间的空行

    # 输出结果表格
    print(format_table(results))

    # 详细样本
    print()
    print("## 样本明细 (ms)")
    for r in results:
        if r.ok:
            samples_str = ", ".join(f"{s:.3f}" for s in r.samples_ms)
            print(f"  {r.name}: [{samples_str}]")
        else:
            print(f"  {r.name}: 跳过 — {r.error}")

    # 汇总
    ok_count = sum(1 for r in results if r.ok)
    fail_count = len(results) - ok_count
    print()
    print(f"## 汇总: {ok_count} 成功 / {fail_count} 失败 / 共 {len(results)} 场景")

    return 0 if fail_count == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
