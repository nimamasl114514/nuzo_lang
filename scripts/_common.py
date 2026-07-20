#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Nuzo Lang 脚本公共工具模块

为 sync_opcode.py 和 sync_tests.py 提供共享的基础设施：
- UTF-8 文件读写（Windows 兼容，无 BOM）
- AUTO-GENERATED 区域查找/替换
- Instruction 枚举解析（opcode.rs SSOT）
- CALL_GRAPH.md 解析
- Rust 测试函数扫描
- 同步报告（SyncReport）

设计原则:
- SSOT: 所有解析基于源文件结构，不硬编码变体列表
- 幂等性: 多次运行结果一致
- Windows 兼容: UTF-8 无 BOM，路径用 pathlib.Path
"""

from __future__ import annotations

import io
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterator, Optional


# ============================================================================
# 常量
# ============================================================================

AUTO_BEGIN = "// AUTO-GENERATED"
AUTO_END = "// END-AUTO-GENERATED"


# ============================================================================
# UTF-8 输出/文件 IO（Windows 兼容）
# ============================================================================

def setup_utf8_output() -> None:
    """配置 stdout/stderr 为 UTF-8，解决 Windows 控制台中文输出问题。"""
    if sys.platform == "win32":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
            sys.stderr.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, io.UnsupportedOperation):
            pass


def read_file_utf8(path: Path) -> str:
    """以 UTF-8 编码读取文件内容。

    Args:
        path: 文件路径

    Returns:
        文件内容字符串

    Raises:
        FileNotFoundError: 文件不存在
        UnicodeDecodeError: 文件不是合法 UTF-8
    """
    return path.read_text(encoding="utf-8")


def write_file_utf8(path: Path, content: str) -> None:
    """以 UTF-8 编码（无 BOM）写入文件内容。

    Args:
        path: 文件路径
        content: 要写入的内容
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8", newline="\n")


# ============================================================================
# 数据类
# ============================================================================

@dataclass
class InstructionVariant:
    """Instruction 枚举变体的解析结果。

    Attributes:
        name: 变体名（如 "LoadK"）
        fields: 字段列表，每项为 (字段名, 类型字符串)
        handler_name: 对应的 dispatch handler 函数名（如 "_op_loadk"）
    """

    name: str
    fields: list[tuple[str, str]] = field(default_factory=list)

    @property
    def field_summary(self) -> str:
        """字段摘要字符串（如 "dest: Reg, const_idx: ConstIdx"）。"""
        return ", ".join(f"{n}: {t}" for n, t in self.fields)

    @property
    def handler_name(self) -> str:
        """对应的 dispatch handler 函数名（蛇形转换 + _op_ 前缀）。

        例如: LoadK → _op_loadk, StringBuild → _op_string_build
        """
        snake = re.sub(r"([A-Z0-9]+)([A-Z][a-z])", r"\1_\2", self.name)
        snake = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", snake)
        return f"_op_{snake.lower()}"


@dataclass
class PubFn:
    """CALL_GRAPH.md 中提取的 pub fn 条目。

    Attributes:
        path: 函数完整路径（如 "VM::run"）
        module: 模块名（如 "nuzo_vm"）
        source_file: 源文件相对路径
        line_range: 行号范围（如 "100-120"）
        signature: 函数签名
    """

    path: str
    module: str
    source_file: str
    line_range: str
    signature: str

    @property
    def test_name(self) -> str:
        """对应的测试函数名（如 "test_vm_run"）。"""
        sanitized = re.sub(r"[^a-zA-Z0-9_]", "_", self.path)
        sanitized = re.sub(r"_+", "_", sanitized).strip("_")
        return f"test_{sanitized.lower()}"


# ============================================================================
# AUTO-GENERATED 区域查找/替换
# ============================================================================

def find_auto_region(content: str, marker: str) -> Optional[tuple[int, int]]:
    """查找 AUTO-GENERATED 区域的行范围。

    区域格式:
        // AUTO-GENERATED: <marker>
        ... 内容 ...
        // END-AUTO-GENERATED: <marker>

    Args:
        content: 文件内容
        marker: 区域标记名

    Returns:
        (起始行索引, 结束行索引) 的元组，未找到返回 None
    """
    begin_pattern = f"{AUTO_BEGIN}: {marker}"
    end_pattern = f"{AUTO_END}: {marker}"

    lines = content.splitlines(keepends=True)
    begin_idx = None
    end_idx = None

    for i, line in enumerate(lines):
        if begin_pattern in line and begin_idx is None:
            begin_idx = i + 1  # 内容从下一行开始
        elif end_pattern in line and begin_idx is not None:
            end_idx = i
            break

    if begin_idx is not None and end_idx is not None:
        return (begin_idx, end_idx)
    return None


def replace_auto_region(content: str, new_body: str, marker: str) -> str:
    """替换 AUTO-GENERATED 区域的内容。

    Args:
        content: 原文件内容
        new_body: 新的区域内容
        marker: 区域标记名

    Returns:
        替换后的完整文件内容
    """
    region = find_auto_region(content, marker)
    if region is None:
        return content

    lines = content.splitlines(keepends=True)
    begin_idx, end_idx = region

    # 保留标记行，替换中间内容
    before = "".join(lines[:begin_idx])
    after = "".join(lines[end_idx:])

    # 确保新内容以换行结尾
    body = new_body.rstrip("\n") + "\n"

    return before + body + after


# ============================================================================
# Instruction 枚举解析（opcode.rs SSOT）
# ============================================================================

def parse_instruction_enum(content: str, enum_name: str = "Instruction") -> list[InstructionVariant]:
    """解析 Rust 枚举定义，提取变体列表。

    支持:
    - 带字段变体: `Variant { field: Type, ... }`
    - 单元变体: `Variant,`

    Args:
        content: 源文件内容
        enum_name: 枚举名（如 "Instruction"）

    Returns:
        InstructionVariant 列表
    """
    variants: list[InstructionVariant] = []

    # 定位枚举定义范围
    enum_start = re.search(
        rf"^\s*pub\s+enum\s+{re.escape(enum_name)}\s*(?::\s*[^{{]+)?\{{",
        content,
        re.MULTILINE,
    )
    if enum_start is None:
        return variants

    # 找到匹配的闭合大括号
    brace_depth = 0
    enum_body_start = enum_start.end()
    enum_body_end = enum_body_start

    for i in range(enum_start.end() - 1, len(content)):
        if content[i] == "{":
            brace_depth += 1
        elif content[i] == "}":
            brace_depth -= 1
            if brace_depth == 0:
                enum_body_end = i
                break

    enum_body = content[enum_body_start:enum_body_end]

    # 解析变体（跳过注释和属性）
    lines = enum_body.splitlines()
    current_variant: Optional[InstructionVariant] = None
    field_buffer: list[str] = []

    for line in lines:
        stripped = line.strip()

        # 跳过空行、注释、属性
        if not stripped or stripped.startswith("//") or stripped.startswith("#["):
            continue

        # 带字段变体开始: `Variant {`
        field_start_match = re.match(
            r"^([A-Z][A-Za-z0-9_]*)\s*\{(.*)$",
            stripped,
        )
        if field_start_match:
            name = field_start_match.group(1)
            rest = field_start_match.group(2)
            current_variant = InstructionVariant(name=name)
            field_buffer.append(rest)
            # 如果同一行闭合
            if rest.count("}") >= rest.count("{") + 1:
                fields_str = " ".join(field_buffer)
                fields_str = fields_str.rsplit("}", 1)[0]
                current_variant.fields = _parse_fields(fields_str)
                variants.append(current_variant)
                current_variant = None
                field_buffer = []
            continue

        # 单元变体: `Variant,`
        unit_match = re.match(r"^([A-Z][A-Za-z0-9_]*)\s*,\s*$", stripped)
        if unit_match and current_variant is None:
            variants.append(InstructionVariant(name=unit_match.group(1)))
            continue

        # 带字段变体续行
        if current_variant is not None:
            if stripped == "}" or stripped.startswith("},"):
                fields_str = " ".join(field_buffer)
                fields_str = re.sub(r"\}.*$", "", fields_str)
                current_variant.fields = _parse_fields(fields_str)
                variants.append(current_variant)
                current_variant = None
                field_buffer = []
            else:
                field_buffer.append(stripped)

    return variants


def _parse_fields(fields_str: str) -> list[tuple[str, str]]:
    """解析变体字段定义字符串。

    例如: "dest: Reg, const_idx: ConstIdx" → [("dest", "Reg"), ("const_idx", "ConstIdx")]
    """
    fields: list[tuple[str, str]] = []
    for part in fields_str.split(","):
        part = part.strip()
        if not part:
            continue
        match = re.match(r"^([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*(.+)$", part)
        if match:
            fields.append((match.group(1), match.group(2).strip()))
    return fields


def parse_handler_functions(content: str) -> list[str]:
    """解析 dispatch_table.rs 中的 _op_xxx handler 函数名。

    Args:
        content: 源文件内容

    Returns:
        handler 函数名列表（如 ["_op_loadk", "_op_add", ...]）
    """
    pattern = r"^\s*fn\s+(_op_[a-z0-9_]+)\s*\("
    return re.findall(pattern, content, re.MULTILINE)


def parse_codegen_instructions(content: str) -> list[str]:
    """解析 codegen.rs 中引用的 Instruction::Xxx 变体名列表。

    Args:
        content: 源文件内容

    Returns:
        变体名列表（去重，按首次出现顺序）
    """
    pattern = r"Instruction::([A-Z][A-Za-z0-9_]*)"
    seen: set[str] = set()
    result: list[str] = []
    for match in re.finditer(pattern, content):
        name = match.group(1)
        if name not in seen:
            seen.add(name)
            result.append(name)
    return result


def parse_skip_ssot_variants(content: str, enum_name: str = "Instruction") -> set[str]:
    """解析 opcode.rs 中标注了 #[opcode_meta(skip_ssot)] 的变体名。

    实现 SSOT：从 opcode.rs 动态提取 skip_ssot 变体，避免硬编码。

    Args:
        content: 源文件内容
        enum_name: 枚举名

    Returns:
        标注了 skip_ssot 的变体名集合
    """
    skip_variants: set[str] = set()

    # 匹配 #[opcode_meta(skip_ssot)] 后面的变体定义
    pattern = re.compile(
        r"#\[opcode_meta\([^)]*skip_ssot[^)]*\)\]\s*\n\s*([A-Z][A-Za-z0-9_]*)",
    )
    for match in pattern.finditer(content):
        skip_variants.add(match.group(1))

    return skip_variants


# ============================================================================
# CALL_GRAPH.md 解析
# ============================================================================

def parse_callgraph(content: str) -> list[PubFn]:
    """解析 CALL_GRAPH.md，提取 pub fn 列表。

    CALL_GRAPH.md 格式（由 nuzo_callgraph 生成）:
        ## Module: nuzo_vm
        ### `VM::run`
        - **Signature**: `pub fn run(&mut self, chunk: Chunk) -> Result<Value, NuzoError>`
        - **Source**: crates/nuzo-vm/src/vm.rs:100-200

    Args:
        content: CALL_GRAPH.md 内容

    Returns:
        PubFn 列表
    """
    fns: list[PubFn] = []
    current_module = ""
    current_path = ""
    current_signature = ""
    current_source = ""

    for line in content.splitlines():
        line = line.strip()

        # 模块标题: ## Module: nuzo_vm
        module_match = re.match(r"^##\s+Module:\s*(\S+)", line)
        if module_match:
            current_module = module_match.group(1)
            continue

        # 函数标题: ### `VM::run` 或 ### VM::run
        fn_match = re.match(r"^###\s+`?([^`]+)`?\s*$", line)
        if fn_match:
            # 保存前一个函数
            if current_path:
                source_file, line_range = _parse_source_ref(current_source)
                fns.append(PubFn(
                    path=current_path.strip(),
                    module=current_module,
                    source_file=source_file,
                    line_range=line_range,
                    signature=current_signature,
                ))
            current_path = fn_match.group(1)
            current_signature = ""
            current_source = ""
            continue

        # 签名: - **Signature**: `pub fn ...`
        sig_match = re.match(r"^-\s*\*\*Signature\*\*:\s*`?(.+?)`?\s*$", line)
        if sig_match:
            current_signature = sig_match.group(1)
            continue

        # 源文件: - **Source**: path:lines
        src_match = re.match(r"^-\s*\*\*Source\*\*:\s*(.+?)\s*$", line)
        if src_match:
            current_source = src_match.group(1)
            continue

    # 保存最后一个函数
    if current_path:
        source_file, line_range = _parse_source_ref(current_source)
        fns.append(PubFn(
            path=current_path.strip(),
            module=current_module,
            source_file=source_file,
            line_range=line_range,
            signature=current_signature,
        ))

    return fns


def _parse_source_ref(source: str) -> tuple[str, str]:
    """解析 Source 引用字符串为 (文件路径, 行号范围)。

    例如: "crates/nuzo-vm/src/vm.rs:100-200" → ("crates/nuzo-vm/src/vm.rs", "100-200")
    """
    if not source:
        return ("", "")
    # 匹配 path:line-range 或 path:line
    match = re.match(r"^(.+?):(\d+(?:-\d+)?)\s*$", source)
    if match:
        return (match.group(1), match.group(2))
    return (source, "")


# ============================================================================
# Rust 测试函数扫描
# ============================================================================

def collect_rust_files(scan_dirs: list[Path]) -> Iterator[Path]:
    """递归收集目录下所有 .rs 文件。

    Args:
        scan_dirs: 要扫描的目录列表

    Yields:
        .rs 文件路径
    """
    for d in scan_dirs:
        if not d.exists():
            continue
        for path in sorted(d.rglob("*.rs")):
            # 跳过 target/ 目录
            if "target" in path.parts:
                continue
            yield path


def parse_test_functions(content: str) -> set[str]:
    """解析 Rust 源码中的测试函数名。

    匹配 #[test] 和 #[tokio::test] 标注的函数。

    Args:
        content: 源文件内容

    Returns:
        测试函数名集合
    """
    names: set[str] = set()

    # 匹配 #[test] 或 #[tokio::test] 后面的 fn name(
    pattern = re.compile(
        r"#\[(?:tokio::)?test[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*\s*fn\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(",
    )
    for match in pattern.finditer(content):
        names.add(match.group(1))

    return names


# ============================================================================
# 同步报告
# ============================================================================

class SyncReport:
    """同步操作报告，收集 ops/warns/errs 并格式化输出。"""

    def __init__(self, dry_run: bool = True, script_name: str = ""):
        self.dry_run = dry_run
        self.script_name = script_name
        self.ops: list[str] = []
        self.warns: list[str] = []
        self.errors: list[str] = []

    def add_op(self, msg: str) -> None:
        self.ops.append(msg)

    def add_warn(self, msg: str) -> None:
        self.warns.append(msg)

    def add_err(self, msg: str) -> None:
        self.errors.append(msg)

    @property
    def has_errors(self) -> bool:
        return len(self.errors) > 0

    def __str__(self) -> str:
        mode = "DRY-RUN" if self.dry_run else "WRITE"
        lines = [
            f"{'=' * 60}",
            f"  {self.script_name} Report [{mode}]",
            f"{'=' * 60}",
        ]

        if self.errors:
            lines.append(f"\n[ERRORS] ({len(self.errors)}):")
            for msg in self.errors:
                lines.append(f"  ✗ {msg}")

        if self.warns:
            lines.append(f"\n[WARNINGS] ({len(self.warns)}):")
            for msg in self.warns:
                lines.append(f"  ⚠ {msg}")

        if self.ops:
            lines.append(f"\n[OPERATIONS] ({len(self.ops)}):")
            for msg in self.ops:
                lines.append(f"  ✓ {msg}")

        lines.append(f"\n{'=' * 60}")
        lines.append(
            f"  Summary: {len(self.ops)} ops, {len(self.warns)} warns, "
            f"{len(self.errors)} errors"
        )
        lines.append(f"{'=' * 60}")

        return "\n".join(lines)
