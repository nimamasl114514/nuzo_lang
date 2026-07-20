# DEVELOPMENT.md — Nuzo Lang 开发指南

> 项目约定文档 | 包含硬约束、分层规则、文件导航

---

## 项目身份

- **Nuzo Lang**：自研编程语言，Rust 实现，NaN-tagged 值系统，寄存器机 VM
- **22 packages (21 workspace members + root nuzo package)**，严格 L0-L6 分层 + root facade（见下方依赖图）
- **构建工具**：`just`（命令见下方）
- **包管理**：Cargo workspace，serde 锁定 `=1.0.197`/`=1.0.115`
- **中文关键词支持 (CJK)**：lexer/parser 全部 CJK 感知

## 构建与验证

| 命令 | 用途 |
|------|------|
| `just check` | 全量编译检查 |
| `just test` | 运行全部测试 |
| `just lint` | clippy 静态分析 |
| `just callgraph` | 重生成 CALL_GRAPH.md |

**铁律**：任何代码修改后必须执行 `just callgraph`。

## 架构速览（5 阶段管线）

```
.nuzo 源码 → Lexer → Parser → Compiler → VM(51 opcodes) → Value
```

| 阶段 | Crate | 入口 | 产出 |
|------|-------|------|------|
| 词法 | `nuzo_frontend` | `lexer::Lexer::new()` → `scan_all()` | `Result<Vec<(Token, &str)>, LexerError>` |
| 语法 | `nuzo_frontend` | `parser::Parser::parse()` | `Result<ast::Program, ParseError>` |
| 编译 | `nuzo_compiler` | `Compiler::compile()` | `Result<bytecode::Chunk, CompileError>` |
| 执行 | `nuzo_vm` | `VM::run()` | `Result<Value, NuzoError>` |

## 层级依赖（违反 = 编译失败）

```
L0: nuzo_proc_core, nuzo_config, nuzo_class_macros, nuzo_codegen
 ↓
L1: nuzo_core, nuzo_proc, nuzo_class
 ↓
L2: nuzo_values, nuzo_opcode, nuzo_signal
 ↓
L3: nuzo_bytecode, nuzo_frontend, nuzo_helpers
 ↓
L4: nuzo_ir, nuzo_error
 ↓
L5: nuzo_compiler, nuzo_vm
 ↓
L6: nuzo_run (Engine + Session + CLI + Bench + Test)
 ↓
root facade: nuzo (thin re-export)
```

**改低层(L0-L2) 必须全量 `cargo check --workspace`。**

## 关键硬约束

### 值系统
- NaN-tagged 8 字节统一编码。用 `from_number()`/`from_bool()` 等构造器
- **禁止 transmute 或手写位模式**
- Smi 范围 [-2^47, 2^47)，溢出自动提升 Float

### Opcode 系统
- **只改 `define_opcodes!` 宏调用**，其余自动生成
- `size = 1 + Σ operand.byte_size()`
- 新增 Opcode 同步 4 处：宏定义 → dispatch.rs → dispatch_table.rs → compiler/
- SpillLoad/SpillStore 使用直接字节码发射（不经 Instruction 枚举）

### GC 安全
- 新增 HeapObject 变体**必须实现 trace()**
- GC 安全点间**禁止持有裸指针**

### API 规范
- Builder 链式：`with_xxx()` + `build()`
- 枚举替代 bool/string
- Result 返回，禁止非测试 unwrap()
- 参数 ≤3（多用结构体）
- 禁止 `pub use xxx::*`

### Windows 特殊配置
- 栈 8MB (`/STACK:8388608`) | 链接器 rust-lld

## 跨文件修改链（改 A 必须同步改 B）

| 改什么 | 必须同步改哪里 |
|--------|--------------|
| 新增运算符 | token.rs + lexer.rs + parser.rs + opcode + dispatch.rs |
| 新增 builtin | helpers/<domain>.rs + register() |
| 新增数据类型 | constants.rs + value.rs + heap.rs + traits.rs + dispatch.rs |
| 修改 GC 策略 | gc.rs + vm.rs + value.rs(trace) + core/constants.rs |
| 修改闭包捕获 | parser.rs + compiler/functions.rs + heap.rs + dispatch.rs |

## 文件导航速查

### Crate 目录结构（扁平化）

所有 crate 直接在 `crates/` 下，按层级排列：

| Crate | 层级 | 说明 |
|-------|------|------|
| `crates/nuzo-proc-core/` | L0 | proc-macro 核心工具 |
| `crates/nuzo-codegen/` | L0 | 代码生成（builtin/opcode/dispatch） |
| `crates/nuzo-config/` | L0 | 配置管理 |
| `crates/nuzo-class-macros/` | L0 | class 宏实现 |
| `crates/nuzo-core/` | L1 | 常量、编码、SourceLocation |
| `crates/nuzo-proc/` | L1 | proc-macro 入口 |
| `crates/nuzo-class/` | L1 | Rust 侧类语法糖 |
| `crates/nuzo-values/` | L2 | NaN-tagged 值系统 |
| `crates/nuzo-opcode/` | L2 | opcode 定义框架 |
| `crates/nuzo-signal/` | L2 | 信号槽系统 |
| `crates/nuzo-bytecode/` | L3 | 字节码格式 |
| `crates/nuzo-frontend/` | L3 | lexer + parser + AST |
| `crates/nuzo-helpers/` | L3 | builtin 函数注册表 |
| `crates/nuzo-ir/` | L4 | 中间表示 |
| `crates/nuzo-error/` | L4 | 错误诊断 |
| `crates/nuzo-compiler/` | L5 | AST→Bytecode 编译器 |
| `crates/nuzo-vm/` | L5 | 虚拟机 + GC |
| `crates/nuzo-run/` | L6 | Engine + Session + CLI |
| `crates/nuzo-gui/` | - | GUI 工具 |

### 任务导航

| 你想做什么 | 从这里开始 | 相关文件 |
|-----------|----------|---------|
| 加新语法特性 | `crates/nuzo-frontend/src/token.rs` | lexer.rs → parser.rs → compiler/ → opcode → dispatch.rs |
| 加新 builtin 函数 | `crates/nuzo-helpers/src/builtins.rs` | 对应 domain.rs + register() |
| 改 VM 行为 | `crates/nuzo-vm/src/dispatch.rs` | vm.rs → gc.rs → values/ |
| 改值系统/类型 | `crates/nuzo-values/src/value.rs` | heap.rs → traits.rs → tag_registry.rs |
| 改错误处理 | `crates/nuzo-error/src/diagnostic.rs` | classifier.rs → formatter.rs → collector.rs |
| 性能优化 | `crates/nuzo-vm/src/vm_hot_trace.rs` | cache.rs → trf.rs → vm_lic.rs |
| 写测试 | `tests/integration/` 或各 crate 的 `#[cfg(test)] mod tests` |

## 反模式清单（看起来对但会炸）

1. **在 nuzo_proc_core 中引入运行时依赖** → 编译失败（L0 不允许向下依赖）
2. **新增 HeapObject 不实现 trace()** → GC 时崩溃
3. **用 transmute 构造 Value** → 在 NaN-tagged 系统中产生非法位模式
4. **手动编辑 CALL_GRAPH.md** → 下次 `just callgraph` 覆盖你的改动
5. **跳过 `just check` 直接提交** → 低层变更可能破坏上层 crate
6. **在热路径使用 `unwrap()`** → 生产环境 panic

## 详细文档索引

| 文档 | 内容 |
|------|------|
| [CALL_GRAPH.md](../CALL_GRAPH.md) | 自动生成的完整调用图（唯一权威） |
| [ARCHITECTURE.md](./ARCHITECTURE.md) | 详细架构文档 |
| [CONTRIBUTING.md](./CONTRIBUTING.md) | 贡献指南与开发规范 |

## 层级依赖

> 自动生成 by `scripts/sync_development.py`，请勿手动编辑。

```
L0 (nuzo_class_macros, nuzo_codegen, nuzo_config, nuzo_proc_core) → L1 (nuzo_class, nuzo_core, nuzo_proc) → L2-L3 (nuzo_opcode, nuzo_signal, nuzo_values) → L3 (nuzo_bytecode, nuzo_frontend, nuzo_helpers) → L4 (nuzo_error, nuzo_ir) → L5 (nuzo_compiler, nuzo_vm) → L6 (nuzo_run)
```

## Crate 列表

> 自动生成 by `scripts/sync_development.py`，请勿手动编辑。

| Crate | 版本 | 层级 | 描述 |
|-------|------|------|------|
| nuzo_class_macros | 0.6.0 | L0 | Proc-macros for nuzo_class |
| nuzo_proc_core | 0.6.0 | L0 | Core utilities for Nuzo procedural macro development (includes hardcode constant management) |
| nuzo_codegen | 0.6.0 | L0 | Nuzo compile-time code generation (builtins, opcodes, dispatch) |
| nuzo_proc | 0.6.0 | L1 | Nuzo procedural macro entry point |
| nuzo_class | 0.6.0 | L1 | Rust-side class syntax sugar for Nuzo Lang |
| nuzo_config | 0.6.0 | L0 | Unified configuration management for Nuzo Lang — zero-dependency layered config with TOML support |
| nuzo_core | 0.6.0 | L1 | Core constants, encoding, macros, and source location for Nuzo |
| nuzo_opcode | 0.6.0 | L2 | Declarative opcode definition framework for VM instruction sets |
| nuzo_signal | 0.6.0 | L2 | Type-safe signal-slot system for Nuzo |
| nuzo_values | 0.6.0 | L2-L3 | NaN-tagged value system for Nuzo Runtime |
| nuzo_bytecode | 0.6.0 | L3 | Bytecode format and instruction set for Nuzo |
| nuzo_frontend | 0.6.0 | L3 | Lexer, parser, and AST for Nuzo |
| nuzo_helpers | 0.6.0 | L3 | Builtin function registry and implementations for Nuzo |
| nuzo_error | 0.6.0 | L4 | Error collection and diagnostics for Nuzo |
| nuzo_ir | 0.6.0 | L4 | Intermediate Representation (IR) for Nuzo Lang compiler |
| nuzo_compiler | 0.6.0 | L5 | Compiler (AST to Bytecode) for Nuzo |
| nuzo_vm | 0.6.0 | L5 | Virtual machine, GC, and runtime for Nuzo |
| nuzo_run | 0.6.0 | L6 | Nuzo Lang - Runtime Engine and CLI |
