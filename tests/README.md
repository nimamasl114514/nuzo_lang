# Nuzo Lang 测试目录

本目录包含 nuzo_lang 项目的所有测试代码，按类型分为以下子目录和文件。

## 目录结构

```
tests/
├── e2e/                  # 端到端测试用例（.nuzo 源文件，由集成测试调用）
├── integration/          # Rust 集成测试（[[test]] 注册于根 Cargo.toml）
├── proptest/             # 属性驱动测试（proptest）
├── gui/                  # GUI 端到端测试用例（.nuzo 源文件）
└── README.md             # 本文件
```

## 各子目录说明

### e2e/ -- 端到端测试用例

用 `.nuzo` 文件编写的测试用例，按类别组织：

| 子目录 | 数量 | 说明 |
|--------|------|------|
| `advanced/` | 4 | 高级语言特性（match、管道、null coalescing 等） |
| `diagnostic/` | 1 | 诊断/瓶颈分析测试 |
| `misc/` | 5 | 杂项测试（push_len、string_concat、中英文混合等） |
| `modules/` | 7 | 模块系统测试（import、嵌套模块、标准库） |
| `other/` | 22 | 基础测试（从基础类型到闭包、GC 压力等） |
| `perf/` | 7 | 性能基准测试（算术、堆、递归、GC 循环等） |
| `regression/` | 13 | 回归测试（bug 修复验证） |
| `stress/` | 6 | 压力测试（1000 行、200 行、GC 压力等） |
| `supplement/` | 4 | 补充测试（中文生态、编译错误路径、GC 边界等） |

**运行方式**：这些 `.nuzo` 文件本身不被直接执行，而是由 `tests/integration/`
下的集成测试通过 `nuzo_run::Engine::run_file` API 调用（参考 `module_tests.rs`
和 `import_tests.rs`）。如需单独运行某个 `.nuzo` 脚本，请使用：

```bash
cargo run -p nuzo_run -- path/to/script.nuzo
```

### integration/ -- Rust 集成测试

用 Rust 编写的集成测试（`*.rs` 文件），通过根 `Cargo.toml` 的 `[[test]]` 注册，
共 9 个测试目标：

| 文件 | 测试名 | 说明 |
|------|--------|------|
| `bug_boundary_tests.rs` | `bug_boundary_tests` | Bug 边界回归测试（最大文件，~45KB） |
| `advanced_abstractions_tests.rs` | `advanced_abstractions_tests` | 高级抽象测试 |
| `import_tests.rs` | `import_tests` | 模块导入测试（调用 `tests/e2e/modules/` 下 .nuzo） |
| `pow_instruction_tests.rs` | `pow_instruction_tests` | Pow 指令测试 |
| `signal_integration.rs` | `signal_integration` | 信号系统集成测试 |
| `ir_integration_tests.rs` | `ir_integration_tests` | IR 中间表示集成测试 |
| `module_tests.rs` | `module_tests` | 模块系统测试（引用 `tests/e2e/modules/` 下 .nuzo） |
| `string_build_tests.rs` | `string_build_tests` | 字符串拼接优化测试 |

**运行方式**：

```bash
# 运行所有集成测试（在根 crate）
cargo test --test '*' -p nuzo

# 运行单个测试目标
cargo test --test bug_boundary_tests
```

### proptest/ -- 属性驱动测试

基于 `proptest` crate 的属性测试，覆盖 lexer/parser/value/e2e 四个层面：

| 文件 | 测试名 | 说明 |
|------|--------|------|
| `lexer_proptest.rs` | `lexer_proptest` | 词法分析器属性测试 |
| `parser_proptest.rs` | `parser_proptest` | 语法分析器属性测试 |
| `value_proptest.rs` | `value_proptest` | 值系统属性测试 |
| `e2e_proptest.rs` | `e2e_proptest` | 端到端属性测试 |

**运行方式**：

```bash
cargo test --test lexer_proptest -- --nocapture
```

> 失败用例会自动写入 `*.proptest-regressions` 文件，提交以避免回归。

### gui/ -- GUI 端到端测试用例

`.nuzo` 源文件，由 `nuzo_gui` crate 调用，验证 GUI 渲染能力。

## 常用运行命令

```bash
# 运行所有测试（workspace 全部 crate 的单元测试 + 集成测试 + 文档测试）
cargo test --workspace --all-targets

# 仅运行根 crate 的集成测试
cargo test --test '*' -p nuzo

# 仅运行特定 crate 的单元测试
cargo test -p nuzo_vm --lib

# 运行属性驱动测试（含 ignored）
cargo test --test lexer_proptest -- --ignored --nocapture

# 运行单 .nuzo 脚本（手动验证）
cargo run -p nuzo_run -- tests/e2e/other/01_basic.nuzo
```

## 回归测试规范

每个 bug 修复必须添加至少 1 个回归测试，测试命名遵循 `test_<bug_id>_<scenario>` 格式。
测试应覆盖：正常路径 + 边界条件 + 错误条件。

详见项目根目录的 `DEVELOPMENT.md` 和 `.trae/rules/project_rules.md` 第十节。

## 关于"自动生成测试桩"

历史上曾存在 `tests/generated/auto_sync_tests.inc`（由 `scripts/sync_tests.py`
生成）作为测试桩的"待办清单"。由于对应的 harness `generated_stubs.rs` 已被移除，
该文件目前已无消费者，不再提交到仓库。如需查看未覆盖的 pub fn 列表，请运行：

```bash
python scripts/sync_tests.py --project . --callgraph CALL_GRAPH.md --dry-run
```
