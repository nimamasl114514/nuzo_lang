# CONTRIBUTING.md — Nuzo Lang 贡献指南

> 欢迎为 Nuzo Lang 贡献代码。本文档涵盖开发环境、代码风格、提交规范、PR 流程与常用命令。
>
> 版本：0.6.0 | Workspace：22 packages（21 members + root facade）

---

## 1. 开发环境搭建

### 1.1 必需工具

| 工具 | 版本 | 安装命令 | 用途 |
|------|------|---------|------|
| Rust toolchain | edition 2024，rustc ≥ 1.88 | https://rustup.rs | 编译器 |
| `just` | ≥ 1.20 | `cargo install just` | 任务运行器（项目主入口） |
| `cargo-watch` | latest | `cargo install cargo-watch` | 文件变化自动重建 |
| `cargo-llvm-cov` | latest | `cargo install cargo-llvm-cov` | 覆盖率报告（可选） |
| Python | ≥ 3.10 | https://python.org | 运行 `scripts/sync_*.py` 同步脚本 |
| `cargo-audit` / `cargo-deny` | latest | `cargo install cargo-audit cargo-deny` | 安全审计（可选） |

### 1.2 Windows 特殊配置

项目主开发环境为 Windows。PowerShell 中请确保：

```powershell
chcp 65001 > $null
$OutputEncoding = [Console]::OutputEncoding = [Text.Encoding]::UTF8
```

链接器使用 `rust-lld`，可执行文件栈大小固定为 **8 MB**（`/STACK:8388608`），见 `.cargo/config.toml`。不要在开发机随意改这一配置——VM 在递归调用 / 闭包嵌套场景需要 8 MB 栈。

### 1.3 首次拉取后必做

```powershell
git clone <repo>
cd nuzo_lang
just sync           # 同步 opcode + 测试桩 + CALL_GRAPH + fmt
just check          # 全量编译检查
just test           # 全量测试
```

如 `just sync` 报 Python 模块缺失，进入 `scripts/` 目录查看 `_common.py` 依赖。

---

## 2. 项目结构总览

21 crate 按 L0-L6 严格分层 + root facade（违反层级 = 编译失败），详见 [DEVELOPMENT.md](./DEVELOPMENT.md)。简要：

```
crates/                     # 扁平化布局，所有 crate 直接在 crates/ 下
├── nuzo-proc-core/      L0     proc-macro 核心工具
├── nuzo-codegen/        L0     代码生成（builtin/opcode/dispatch）
├── nuzo-config/         L0     配置管理
├── nuzo-class-macros/   L0     class 宏实现
├── nuzo-core/           L1     常量、编码、SourceLocation
├── nuzo-proc/           L1     proc-macro 入口
├── nuzo-class/          L1     Rust 侧类语法糖
├── nuzo-values/         L2-L3  NaN-tagged 值系统
├── nuzo-opcode/         L2     opcode 定义框架
├── nuzo-signal/         L2     信号槽系统
├── nuzo-bytecode/       L3     字节码格式
├── nuzo-frontend/       L3     lexer + parser + AST
├── nuzo-helpers/        L3     builtin 函数注册表
├── nuzo-ir/             L4     中间表示
├── nuzo-error/          L4     错误诊断
├── nuzo-compiler/       L5     AST→Bytecode 编译器
├── nuzo-vm/             L5     虚拟机 + GC
├── nuzo-run/            L6     Engine + Session + CLI（唯一公共 API）
├── nuzo-gui/            -      GUI 工具
└── (src/)               root   根 facade crate (nuzo) — 薄转发层
```

执行管线 5 阶段：`.nuzo 源码 → Lexer → Parser → Compiler → VM(51 opcodes) → Value`。

完整调用图见 [CALL_GRAPH.md](../CALL_GRAPH.md)（自动生成，**勿手改**）。

---

## 3. 代码风格规范

### 3.1 rustfmt

- 项目根使用默认 `rustfmt` 配置（无 `rustfmt.toml`）。
- 提交前必须 `just fmt` 通过 `just fmt-check`。
- 不要在 PR 中混入纯格式化 diff——单独提一个 `chore: fmt` 提交。

### 3.2 clippy 严格模式

```powershell
just lint          # 等价于 cargo clippy --workspace -- -D warnings
```

**零警告目标**：任何 PR 不得引入新 warning。如必须写 `unwrap()`，仅允许在 `#[cfg(test)]` 模块中。

### 3.3 注释规范

| 类型 | 标记 | 位置 |
|------|------|------|
| 文档注释 | `///` | 公开 item 之上，描述用法与示例 |
| 模块注释 | `//!` | 文件顶部，描述模块职责 |
| 内部注释 | `//` | 行尾或行上方，解释 *为什么*（不是 *是什么*） |
| TODO | `// TODO(name):` | 必须带作者标记，禁止裸 `// TODO` |
| 安全注释 | `// SAFETY:` | unsafe 块上方，解释不变量 |

中文 / 英文均可，但同一文件内保持一致。

### 3.4 API 设计硬约束（来自 DEVELOPMENT.md）

- Builder 链式：`with_xxx()` + `build()`
- 枚举替代 `bool` / `String` 标志
- 全部 `Result` 返回，禁止非测试 `unwrap()`
- 参数 ≤ 3，多则用结构体
- 禁止 `pub use xxx::*`

---

## 4. 提交规范

采用 **Conventional Commits** 风格：

```
<type>(<scope>): <subject>

<body>

<footer>
```

### 4.1 type 列表

| type | 用途 | 示例 |
|------|------|------|
| `feat` | 新功能 | `feat(vm): add MatchOpcode dispatch` |
| `fix` | bug 修复 | `fix(compiler): resolve closure capture leak` |
| `docs` | 文档变更 | `docs: add CONTRIBUTING.md` |
| `refactor` | 重构（无行为变化） | `refactor(values): split heap.rs into ops trait` |
| `perf` | 性能优化 | `perf(vm): inline dispatch_opcode_fast` |
| `test` | 测试新增 / 修改 | `test(e2e): add tail_call_test.nuzo` |
| `chore` | 构建 / 工具 / 杂项 | `chore: bump serde to 1.0.197` |
| `ci` | CI 配置 | `ci: enable windows-latest in matrix` |

### 4.2 scope 建议

优先使用 crate 名（去掉 `nuzo_` 前缀）：`vm` / `compiler` / `frontend` / `values` / `opcode` / `helpers` / `error` / `run` / `ir` / `bytecode` / `class` / `config` / `signal` / `proc` / `core`。

### 4.3 footer

- 破坏性变更：`BREAKING CHANGE: <说明>`
- 关联 issue：`Closes #123` / `Refs #456`

### 4.4 示例

```
feat(helpers): add string.repeat builtin

Implements `repeat(s, n)` returning `s` concatenated `n` times.
Returns empty string when n <= 0.

Closes #142
```

---

## 5. PR 流程

### 5.1 分支命名

```
<type>/<short-description>
```

例：`feat/string-repeat`、`fix/closure-capture`、`docs/lang-spec`。

不要直接在 `main` 分支开发。

### 5.2 自检清单（提交 PR 前）

```powershell
just check                  # cargo check --workspace --all-targets
just test                   # cargo test --workspace --all-targets
just lint                   # cargo clippy -- -D warnings
just fmt-check              # cargo fmt --check
just callgraph              # 重生成 CALL_GRAPH.md（如改了函数/opcode/文件）
just sync-opcode-apply      # 如改了 opcode 定义
just sync-tests-apply       # 如改了 builtin / opcode 影响测试桩
```

### 5.3 PR 描述模板

```markdown
## 改动摘要
- 一句话说明改了什么

## 动机
- 为什么改 / 解决什么 issue

## 影响面
- 涉及 crate / 文件
- 是否破坏向后兼容

## 验证
- [x] just check
- [x] just test
- [x] just lint
- [x] just callgraph（如适用）
- [x] just sync-opcode-apply（如适用）
- [x] just sync-tests-apply（如适用）

## 风险
- 已知边界 / 未覆盖场景
```

### 5.4 CALL_GRAPH 同步

任何 PR 涉及以下变更必须重新生成 `CALL_GRAPH.md` 并包含在 PR 中：

- 新增 / 删除 / 重命名函数
- 新增 / 删除 opcode
- 新增 / 删除源文件
- 新增 / 删除 crate

```powershell
just callgraph
git add CALL_GRAPH.md
git commit -m "chore: regenerate CALL_GRAPH"
```

---

## 6. just 命令速查表

| 命令 | 用途 |
|------|------|
| `just sync` | 一键同步：opcode + tests + callgraph + fmt（改动后首选） |
| `just check` | 全量编译检查（`cargo check --workspace --all-targets`） |
| `just check-lib` | 仅 lib 快速检查 |
| `just check-fast` | 启用 sccache 的快速检查 |
| `just test` | 全量测试（`cargo test --workspace --all-targets`） |
| `just test-crate <name>` | 指定 crate 测试，如 `just test-crate nuzo_vm` |
| `just test-ui` | UI compile-fail 测试（nuzo_class_macros） |
| `just test-generated` | 编译生成测试桩（不执行） |
| `just config-test` | nuzo_config 测试（单线程） |
| `just lint` | clippy 严格模式 |
| `just lint-fix` | clippy 自动修复（谨慎） |
| `just fmt` | 格式化全部代码 |
| `just fmt-check` | 检查格式（CI 用） |
| `just coverage` | HTML 覆盖率报告（输出 `target/coverage/`） |
| `just callgraph` | 重生成 `CALL_GRAPH.md` |
| `just doc` | 生成 Rust 文档并浏览器打开 |
| `just doc-serve` | 生成文档到 `target/doc/` |
| `just repl` | 启动 REPL |
| `just run <FILE>` | 运行单个 `.nuzo` 文件 |
| `just sync-opcode-apply` | 应用 opcode 同步（修改文件） |
| `just sync-tests-apply` | 应用测试桩同步（生成文件） |
| `just watch-sync` | 文件变化自动同步 + check |
| `just audit` | 安全审计（cargo-audit + cargo-deny） |
| `just check-all` | 提交前全量：check + test + lint + fmt-check + config-test |
| `just clean` | 清理产物（保留 `target/`） |
| `just clean-all` | 完全清理（含 `target/`） |

---

## 7. 添加新功能流程

### 7.1 新增 opcode（10 步）

1. 在 `crates/nuzo-bytecode/src/opcode.rs` 的 `define_opcodes!` 宏调用中添加新变体，写明 `code` / `size` / `operands` / `disasm`。
2. 运行 `just sync-opcode-apply` 自动生成 `Opcode` 枚举、编解码、反汇编代码。
3. 在 `crates/nuzo-vm/src/dispatch.rs` 添加 dispatch handler（`#[opcode]` 属性会指引你到正确位置）。
4. 在 `crates/nuzo-compiler/src/codegen/` 添加 codegen 逻辑。
5. 在 `crates/nuzo-compiler/src/compiler.rs` 的 AST 分发处接入新节点。
6. 如新 opcode 涉及新 AST 节点：先改 `nuzo_frontend/src/token.rs` + `lexer.rs` + `parser.rs` + `ast.rs`。
7. 加单元测试到对应 crate 的 `tests/` 模块。
8. 加 e2e 测试到 `tests/e2e/`（一个 `.nuzo` 文件 + 期望输出）。
9. 运行 `just sync-tests-apply` 生成测试桩。
10. `just check && just test && just lint && just callgraph`。

### 7.2 新增 builtin 函数（8 步）

1. 选定 domain 模块：`crates/nuzo-helpers/src/{array,string,math,io,time,convert,debug}.rs`。
2. 在该 domain 文件中实现函数：签名 `fn name(args: &[Value]) -> Result<Value, NuzoError>`。
3. 在 `crates/nuzo-helpers/src/builtins.rs` 的 `register()` 中注册：`registry.register("name", func, arity)`。
4. 在 `crates/nuzo-helpers/src/lib.rs` 的 `builtin_names()` 中加入名字（如需 IR 识别）。
5. 加单元测试到 domain 文件的 `#[cfg(test)] mod tests`。
6. 加 e2e 测试到 `tests/e2e/`。
7. 运行 `just sync-tests-apply`。
8. `just check && just test && just lint && just callgraph`。

### 7.3 新增 crate（5 步）

1. 创建目录：`crates/<layer>/<nuzo_xxx>/`，加 `Cargo.toml`（参考同层其他 crate）。
2. 在根 `Cargo.toml` 的 `[workspace] members` 数组中添加路径（注意保持层级分组）。
3. 在根 `Cargo.toml` 的 `[workspace.dependencies]` 中添加 `nuzo_xxx = { path = "..." }`。
4. 在 `DEVELOPMENT.md` 的「Crate 列表」表格末尾添加一行（**勿手动重排层级表**——`scripts/sync_development.py` 会处理）。
5. 运行 `just sync` + `just callgraph`。

> **重要**：必须遵守层级依赖（见 DEVELOPMENT.md）。低层（L0-L2）禁止依赖高层。`nuzo_error` 不能依赖 `nuzo_compiler`。

---

## 8. 调试技巧

### 8.1 cargo expand（看宏展开）

```powershell
cargo install cargo-expand
cargo expand -p nuzo_vm dispatch::execute_inner
```

`define_opcodes!` / `#[opcode]` 等宏展开后能看清实际生成的 match 分支。

### 8.2 日志级别

项目使用 `log` crate。设置环境变量控制日志：

```powershell
$env:RUST_LOG="nuzo_vm=debug,nuzo_compiler=info"
just run examples/fib.nuzo
```

关键模块：`nuzo_vm::dispatch` / `nuzo_vm::gc` / `nuzo_compiler::codegen` / `nuzo_frontend::parser`。

### 8.3 IR dump

```powershell
$env:NUZO_DUMP_IR=1
just run examples/your_file.nuzo
```

会打印 IR 优化前 / 优化后两份 dump 到 stderr。

### 8.4 VEH handler（Windows）

Windows 下 VM 启用 VEH（Vectored Exception Handler）捕获栈溢出 / 访问违例。调试时如需禁用：

```powershell
$env:NUZO_DISABLE_VEH=1
```

栈溢出通常意味着无限递归且未触发 TCO——优先检查 `dispatch.rs` 中 `TailCall` 是否正确生成。

### 8.5 GDB / LLDB 断点

VM 主循环在 `crates/nuzo-vm/src/vm.rs::VM::run_inner()`，单指令分发在 `crates/nuzo-vm/src/dispatch.rs::execute_inner` (line ~1419)。

```
break nuzo_vm::vm::VM::run
break nuzo_vm::dispatch::execute_inner
```

### 8.6 nuzo_callgraph 工具

独立工具 crate（位于 `d:\10\nuzo_callgraph`，非 workspace 成员），生成完整调用图：

```powershell
just callgraph
```

输出 `CALL_GRAPH.md`，是定位函数调用关系、识别死代码的权威来源。

### 8.7 常见陷阱速查

| 症状 | 可能原因 | 排查路径 |
|------|---------|---------|
| 编译期 `error:[E0277]` trait bound 不满足 | 漏实现 `HeapObjectOps` | 检查 `nuzo_values/src/heap.rs` 是否新增变体 |
| 运行时 panic `invalid NaN-tagged value` | 用 `transmute` 构造 Value | 改用 `Value::from_number()` 等构造器 |
| GC 后内存损坏 | 新 `HeapObject` 漏 `trace_gc_refs()` | 检查 `HeapObjectOps` 实现 |
| 栈溢出 crash | 递归未触发 TCO | 检查 `compiler/functions.rs` 是否生成 `TailCall` |
| 测试桩编译失败 | builtin 改了但未跑 sync | `just sync-tests-apply` |

---

## 9. 反馈与协作

- **Issues**：在 GitHub Issues 提问 / 报 bug，附最小复现代码与 `just check` 输出。
- **Discussion**：架构讨论优先开 GitHub Discussion，贴 ADR 提案（`docs/adr/` 目录待创建）。
- **不要**直接联系维护者私有邮箱——所有设计决策必须留痕。

感谢你的贡献！
