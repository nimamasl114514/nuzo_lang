# ARCHITECTURE.md — Nuzo Lang 导航地图 + 架构总览

> 🗺️ **上半部分：30 秒找到你要改的文件。下半部分：详细架构与实现指南。**

---

## 目录树

```
nuzo_lang/
├── CALL_GRAPH.md                ← 自动生成的调用图
├── CHANGELOG.md                 ← 变更日志
├── docs/
│   ├── ARCHITECTURE.md          ← 你在这里（导航地图 + 详细架构）
│   ├── DEVELOPMENT.md           ← 开发约定 + 硬约束
│   ├── CONTRIBUTING.md          ← 贡献指南
│   ├── SCHF_V6_SPEC.md          ← SCHF V6 规范
│   ├── SUMMARY.md               ← 项目摘要
│   ├── coverage_gaps.md         ← 测试覆盖缺口
│   ├── development-guide.md     ← 开发指南
│   ├── language-spec.md         ← 语言规范
│   ├── opcode-reference.md      ← Opcode 参考
│   ├── standard-library.md      ← 标准库文档
│   ├── specs-three-todos-prod-*.md  ← 三件套 spec（3 文件）
│   └── auto-macro-development-report.md
├── Cargo.toml                   ← workspace 根配置
├── justfile                     ← 构建命令（just check/test/lint/callgraph）
├── README.md
├── crates/                      ← 所有 crate，扁平化
│   ├── nuzo-proc-core/          ← L0: proc-macro 核心工具
│   ├── nuzo-codegen/            ← L0: 代码生成（builtin/opcode/dispatch）
│   ├── nuzo-config/             ← L0: 配置管理
│   ├── nuzo-class-macros/       ← L0: class 宏实现
│   ├── nuzo-core/               ← L1: 常量、编码、SourceLocation
│   ├── nuzo-proc/               ← L1: proc-macro 入口
│   ├── nuzo-class/              ← L1: Rust 侧类语法糖
│   ├── nuzo-values/             ← L2: NaN-tagged 值系统
│   ├── nuzo-opcode/             ← L2: opcode 定义框架
│   ├── nuzo-signal/             ← L2: 信号槽系统
│   ├── nuzo-bytecode/           ← L3: 字节码格式
│   ├── nuzo-frontend/           ← L3: lexer + parser + AST
│   ├── nuzo-helpers/            ← L3: builtin 函数注册表
│   ├── nuzo-ir/                 ← L4: 中间表示
│   ├── nuzo-error/              ← L4: 错误诊断
│   ├── nuzo-compiler/           ← L5: AST→Bytecode 编译器
│   ├── nuzo-vm/                 ← L5: 虚拟机 + GC
│   ├── nuzo-run/                ← L6: Engine + Session + CLI（唯一公共 API）
│   └── nuzo-gui/                ← GUI 工具
├── tools/
│   ├── nuzo-explore/            ← 探索/实验工具
│   └── perf-codegen/            ← 性能代码生成工具
├── tests/
│   ├── integration/             ← 集成测试
│   └── proptest/                ← 属性测试
├── benches/                     ← 性能基准测试
└── src/                         ← 根 facade crate (nuzo)
    └── lib.rs                   ← 薄转发层
```

---

## 我想做什么？→ 去哪里改？

| 我想... | 去这里 | 关键文件 |
|---------|--------|---------|
| 加新语法/运算符 | `crates/nuzo-frontend/` | `src/token.rs` → `src/lexer.rs` → `src/parser.rs` |
| 加新 builtin 函数 | `crates/nuzo-helpers/` | `src/builtins.rs` + 对应 domain 文件 |
| 加新 opcode 指令 | `crates/nuzo-opcode/` | `src/lib.rs`（`define_opcodes!` 宏） |
| 改 VM 执行逻辑 | `crates/nuzo-vm/src/` | `dispatch.rs`（主循环） |
| 改 GC 行为 | `crates/nuzo-vm/src/gc/` | `mark.rs`（mark/sweep） |
| 改值系统/类型 | `crates/nuzo-values/src/` | `value.rs` + `heap.rs` |
| 改编译器 | `crates/nuzo-compiler/src/` | `compiler.rs`（入口） |
| 改字节码格式 | `crates/nuzo-bytecode/src/` | `opcode.rs`（define_opcodes!） |
| 改错误处理 | `crates/nuzo-error/src/` | `diagnostic.rs` |
| 改 proc-macro | `crates/nuzo-proc-core/src/` | 工具函数 + `crates/nuzo-proc/src/lib.rs`（入口） |
| 改代码生成 | `crates/nuzo-codegen/src/` | `builtin_gen.rs`, `opcode_gen.rs`, `dispatch_gen.rs` |
| 改 CLI/Engine | `crates/nuzo-run/src/` | `main.rs`（CLI）, `engine.rs`（API） |
| 改信号槽 | `crates/nuzo-signal/src/` | `signal.rs` + `slot.rs` |
| 改 IR | `crates/nuzo-ir/src/` | `types.rs` + `builder.rs` |
| 改配置系统 | `crates/nuzo-config/src/` | `config.rs` |
| 改 class 语法糖 | `crates/nuzo-class/` + `crates/nuzo-class-macros/` | `lib.rs`（两侧） |
| 写集成测试 | `tests/integration/` | 按功能分文件 |
| 写属性测试 | `tests/proptest/` | 按模块分文件 |
| 跑基准测试 | `benches/` | `bench_*.rs` |
| 看架构设计 | `docs/ARCHITECTURE.md`（本文件） | 下半部分：详细架构 |
| 看开发约定 | `docs/DEVELOPMENT.md` | 硬约束 + 规则 |

---

## 层级依赖图

```
L0: nuzo-proc-core, nuzo-codegen, nuzo-config, nuzo-class-macros  (无内部依赖)
 ↓
L1: nuzo-core, nuzo-proc, nuzo-class                              (基础)
 ↓
L2: nuzo-values, nuzo-opcode, nuzo-signal                         (类型/信号)
 ↓
L3: nuzo-bytecode, nuzo-frontend, nuzo-helpers                    (字节码/前端)
 ↓
L4: nuzo-ir, nuzo-error                                           (IR/错误)
 ↓
L5: nuzo-compiler, nuzo-vm                                        (编译/VM)
 ↓
L6: nuzo-run                                                      (Engine + CLI)
 ↓
root: nuzo (root facade)                                            (薄转发)
```

> **改低层(L0-L2) 必须全量 `cargo check --workspace`。**

---

## 关键入口点

| 入口 | 路径 | 说明 |
|------|------|------|
| CLI 入口 | `crates/nuzo-run/src/main.rs` | `nuzo` 命令行工具 |
| 公共 API | `crates/nuzo-run/src/engine.rs` | `Engine` 结构体，外部唯一入口 |
| 编译器入口 | `crates/nuzo-compiler/src/compiler.rs` | `Compiler::compile()` |
| VM 入口 | `crates/nuzo-vm/src/vm.rs` | `VM::run()` |
| Lexer 入口 | `crates/nuzo-frontend/src/lexer.rs` | `Lexer::new()` → `scan_all()` |
| Parser 入口 | `crates/nuzo-frontend/src/parser.rs` | `Parser::parse()` |
| proc-macro 入口 | `crates/nuzo-proc/src/lib.rs` | `#[proc_macro_*]` 函数 |
| 根 facade | `src/lib.rs` | 薄转发层，re-export `nuzo_run` |

---

## 5 阶段编译管线

```
.nuzo 源码 → Lexer → Parser → Compiler → VM(51 opcodes) → Value
```

| 阶段 | Crate | 入口 | 产出 |
|------|-------|------|------|
| 词法 | `nuzo-frontend` | `lexer::Lexer::new()` → `scan_all()` | `Result<Vec<(Token, &str)>, LexerError>` |
| 语法 | `nuzo-frontend` | `parser::Parser::parse()` | `Result<ast::Program, ParseError>` |
| 编译 | `nuzo-compiler` | `Compiler::compile()` | `Result<bytecode::Chunk, CompileError>` |
| 执行 | `nuzo-vm` | `VM::run()` | `Result<Value, NuzoError>` |

---

## 其他文档

| 文档 | 内容 |
|------|------|
| [DEVELOPMENT.md](./DEVELOPMENT.md) | 开发约定 + 硬约束 + 反模式 |
| [CONTRIBUTING.md](./CONTRIBUTING.md) | 贡献指南 |
| [language-spec.md](./language-spec.md) | 语言规范 |
| [opcode-reference.md](./opcode-reference.md) | Opcode 参考 |
| [standard-library.md](./standard-library.md) | 标准库文档 |
| [CHANGELOG.md](../CHANGELOG.md) | 变更日志 |
| [CALL_GRAPH.md](../CALL_GRAPH.md) | 自动生成的完整调用图 |

---

## 跨文件修改链（改 A 必须同步改 B）

| 改什么 | 必须同步改哪里 |
|--------|--------------|
| 新增运算符 | `crates/nuzo-frontend/src/token.rs` + `lexer.rs` + `parser.rs` + opcode + dispatch |
| 新增 builtin | `crates/nuzo-helpers/src/<domain>.rs` + `register()` |
| 新增数据类型 | `crates/nuzo-core/src/constants.rs` + `crates/nuzo-values/src/value.rs` + `heap.rs` + `traits.rs` + dispatch |
| 修改 GC 策略 | `crates/nuzo-vm/src/gc.rs` + `vm.rs` + `crates/nuzo-values/src/value.rs`(trace) + `crates/nuzo-core/src/constants.rs` |
| 修改闭包捕获 | `crates/nuzo-frontend/src/parser.rs` + `crates/nuzo-compiler/` + `crates/nuzo-values/src/heap.rs` + dispatch |

---

# 详细架构

> 面向开发者的实用指南，非学术论文。读一遍就能上手改代码。

---

## 1. 代码执行全链路

Nuzo Lang 采用 **5 阶段编译管线**，从源码到执行路径清晰：

```
源码 (.nuzo)
  │
  ▼
┌─────────────────────────────────────────────────┐
│ Stage 1: Lexer (词法分析)                       │
│   nuzo_frontend::lexer::Lexer                   │
│   字符 → Token（状态机扫描，CJK 感知）          │
│   入口：Lexer::new(source: &str)                │
└──────────────────┬──────────────────────────────┘
                   ▼
┌─────────────────────────────────────────────────┐
│ Stage 2: Parser (语法分析)                      │
│   nuzo_frontend::parser::Parser                 │
│   源码 → AST（内部先调 Lexer）                  │
│   入口：Parser::parse(source: &str)             │
│   产出：ast::Program                             │
└──────────────────┬──────────────────────────────┘
                   ▼
┌─────────────────────────────────────────────────┐
│ Stage 3: Compiler (编译)                        │
│   nuzo_compiler::Compiler                       │
│   AST → IR → Bytecode + Chunk (寄存器分配)      │
│   入口：Compiler::compile(source: &str)         │
│         Compiler::compile_program(&ast::Program)│
│   产出：bytecode::Chunk                          │
└──────────────────┬──────────────────────────────┘
                   ▼
┌─────────────────────────────────────────────────┐
│ Stage 4: VM Execution (字节码解释执行)          │
│   nuzo_vm::VM::run(chunk)                       │
│   寄存机 VM + 51 种 opcode dispatch             │
│   帧管理 / 寄存器文件 / builtin 调用 / GC 触发  │
└──────────────────┬──────────────────────────────┘
                   ▼
┌─────────────────────────────────────────────────┐
│ ┌─ dispatch ─→ 51 种 Opcode 分发 (dispatch.rs)  │
│ ├─ gc     ─→ 增量标记-清除 (gc/mod.rs)          │
│ ├─ values ─→ NaN-tagged 8 字节统一编码          │
│ ├─ object ─→ Shape-based 属性缓存               │
│ ├─ cache  ─→ InlineCache / StringPool / ShapeCache │
│ ├─ trf    ─→ TypedRegFile 快路径               │
│ └─ vm_lic ─→ 多级内联缓存调用系统               │
└─────────────────────────────────────────────────┘
```

**关键数据流**：

| 阶段 | 输入 | 输出 | 关键类型 |
|------|------|------|----------|
| Lexer | `&str` 源码 | `Vec<Token>` | `Token`, `TokenKind` |
| Parser | `&str` 源码（内部 Lexer） | `ast::Program` | `Expr`, `Stmt`, `Span` |
| Compiler | `ast::Program`（经 IR） | `Chunk` | `Chunk`, `CompileError` |
| VM | `Chunk` | `Result<Value>` | `VM`, `Value`, `Gc<T>` |

---

## 2. 常见任务定位指南

### 2.1 加新的运算符（如 `**` 幂运算）

**涉及 7 个文件，必须全部修改才能编译通过**：

| 序号 | 文件 | 操作 |
|------|------|------|
| 1 | `crates/nuzo-frontend/src/token.rs` | 在 `TokenKind` 枚举中新增变体（如 `Pow`） |
| 2 | `crates/nuzo-frontend/src/lexer.rs` | 在扫描分支中识别新符号（如 `**`） |
| 3 | `crates/nuzo-frontend/src/parser.rs` | 在表达式解析中设定优先级和结合性 |
| 4 | `crates/nuzo-ir/src/types.rs` | 在 `From<BinaryOp> for IrBinOp` 中新增映射分支（SSOT，编译期 exhaustive match 保证完整性） |
| 5 | `crates/nuzo-bytecode/src/opcode.rs` | 通过 `define_opcodes!` 宏新增 `Opcode::Pow` + 编解码 |
| 6 | `crates/nuzo-compiler/src/expressions.rs` | 在 `compile_binary` 中新增运算符到 opcode 的映射（宏生成） |
| 7 | `crates/nuzo-vm/src/dispatch.rs` | 在 VM 主循环中添加 Pow 的运算逻辑 |

**前置**：改之前先阅读 `crates/nuzo-frontend/src/parser.rs` 中表达式解析函数与 `crates/nuzo-vm/src/dispatch.rs` 中 dispatch 主循环，确认优先级介入点。

> **SSOT 提示**：步骤 4 中的 `From<BinaryOp> for IrBinOp` 是 AST 运算符到 IR 运算符的**唯一映射定义**。如果忘记添加新变体的映射，编译器会因非 exhaustive match 直接报错。

---

### 2.2 加新的 builtin 函数

**仅需 2 步，改后自动生效，无需碰 VM**：

| 序号 | 文件 | 操作 |
|------|------|------|
| 1 | `crates/nuzo-helpers/src/<domain>.rs` | 实现函数签名 `fn(args: &[Value]) -> Result<Value, NuzoError>` |
| 2 | `crates/nuzo-helpers/src/<domain>.rs` | 在 `<domain>::register(&mut registry)` 中调用 `registry.register("func_name", fn, arity)` |

注册入口在 `crates/nuzo-helpers/src/builtins.rs` 的 `BuiltinRegistry::new()`，各 domain 模块通过 `register()` 向注册表注册函数。

---

### 2.3 加新的数据类型（如 Range）

**涉及 5 层，从类型系统到底层表示**：

| 序号 | 文件 | 操作 |
|------|------|------|
| 1 | `crates/nuzo-values/src/constants.rs` | 新增 NaN-tag 位模式常量（如 `RANGE_TAG`） |
| 2 | `crates/nuzo-values/src/value.rs` | 实现 `is_range()` / `as_range()` / `from_range()` 方法 |
| 3 | `crates/nuzo-values/src/heap.rs` | 在 `HeapObject` 枚举中添加新变体（RangeVariant） |
| 4 | `crates/nuzo-values/src/traits.rs` | 为 Range 实现 `NuzoType` trait |
| 5 | `crates/nuzo-vm/src/dispatch.rs` | 添加 Range 类型的特定操作（如范围比较、迭代） |

---

### 2.4 改 GC 行为

| 序号 | 文件 | 操作 |
|------|------|------|
| 1 | `crates/nuzo-vm/src/gc/mod.rs` | 核心逻辑：mark/sweep/alloc |
| 2 | `crates/nuzo-vm/src/vm.rs` | 修改 `collect_gc_roots()` 根集扫描逻辑 |
| 3 | `crates/nuzo-values/src/value.rs` | 修改 `Value::trace()` 递归标记实现 |
| 4 | `crates/nuzo-core/src/constants.rs` | 调整 GC 阈值参数（如 `GC_MIN_THRESHOLD`） |

---

## 3. 层级依赖规则

项目分为 7 层（L0-L6）+ root facade，依赖方向**严格单向向下**：

```
L0 (无内部依赖)    ─ nuzo_class_macros, nuzo_codegen, nuzo_config, nuzo_proc_core
L1 (基础)         ─ nuzo_class, nuzo_core, nuzo_proc
L2 (类型/信号)    ─ nuzo_opcode, nuzo_signal, nuzo_values
L3 (字节码/前端)  ─ nuzo_bytecode, nuzo_frontend, nuzo_helpers
L4 (错误/IR)      ─ nuzo_error, nuzo_ir
L5 (编译/VM)      ─ nuzo_compiler, nuzo_vm
L6 (应用/门面)    ─ nuzo_run (Engine + Session + CLI + Bench + Test)
root (根门面)     ─ nuzo (root facade, thin re-export)
```

> 根 package `nuzo`（`src/lib.rs`）是薄转发层，仅 `pub use nuzo_run` 的公共 API，概念上是 root facade 而非 L7。
> 层级由 `scripts/sync_development.py` 自动推断，DEVELOPMENT.md 中的层级依赖和 Crate 列表为自动生成。

**核心规则**：
- 只允许依赖同层或更低层
- 改低层（L0-L2）必须全量 `cargo check --workspace`
- 改高层可能影响所有上层

### 3.1 门面（Facade）

`nuzo_run` 是**唯一的公共 API 真相源**（L6）。它封装了 `Compiler + VM` 管线，对外暴露：

- `Engine` — 主门面结构体（`run()` / `eval()` / `compile()` / `run_file()`）
- `EngineBuilder` — 链式配置（`with_config()` / `with_config_file()` / `with_env_config()` / `trace()` / `trace_registers()` / `plugin()`）
- `Session` — 单次执行上下文（`run()` / `eval()`）
- 受控自省方法（通过 `Session` 访问 VM 状态）
- `NuzoPlugin` — 插件扩展 trait
- `BenchHarness` / `TestHarness` — 基准测试与测试工具

**门面不暴露**：`&VM` / `&mut VM` / `TraceConfig` / `TraceResult` 等内部类型。

根 package `nuzo` 仅转发 `nuzo_run` 的公共 API，不再整体 re-export 子 crate。外部代码（含测试和示例）如需访问内部 crate（如 `VM`、`Compiler`、`Lexer`），应通过 dev-dependencies 直接依赖对应 crate，而非通过根 package 间接访问。

**例外**：`nuzo_run` 的 `nuzo_run` 二进制因需要 `VM::init_gc_with_config()`（从 nuzo.toml 加载配置），保留对 `nuzo_vm` / `nuzo_config` / `nuzo_frontend` 的直接依赖作为有注释的例外。

---

## 4. 模块职责速查表

| 模块 (crate) | 职责 | 关键类型 | 入口点 |
|--------------|------|----------|--------|
| `nuzo_core` | 零依赖根基：常量、编码、位置 | `SourceLocation`, `Encoding` | `source_location::SourceLocation` |
| `nuzo_values` | NaN-tagged 值系统：8 字节统一编码 | `Value`, `HeapObject`, `FunctionPrototype` | `value::Value`, `heap::HeapObject` |
| `nuzo_opcode` | 声明式 opcode 宏框架 | `Opcode` (macro-generated), `OperandKind`, `DispatchKind` | `define_opcodes!` proc-macro |
| `nuzo_bytecode` | 字节码容器：51 种 opcode、常量池 | `Chunk`, `Scope`, `Instruction` | `bytecode::Chunk` |
| `nuzo_frontend` | 词法 + 语法分析 | `Token`, `Lexer`, `Parser`, `AST` | `lexer::Lexer`, `parser::Parser` |
| `nuzo_ir` | 中间表示层（IR） | `ValueRef`, `BasicBlock`, `IrModule` | `builder::IrBuilder::build()` |
| `nuzo_compiler` | 递归下降编译器 | `Compiler`, `CompileError`, `CodeGenerator` | `compiler::Compiler::compile()` |
| `nuzo_vm` | 寄存机 VM + GC + 缓存系统 | `VM`, `Gc<T>`, `Shape` | `vm::VM::run(chunk)` |
| `nuzo_signal` | 线程安全信号槽系统 | `Signal<Args>`, `SignalBus`, `Connection` | `signal::Signal`, `bus::SignalBus` |
| `nuzo_helpers` | 内置函数注册表 | `BuiltinRegistry` | `builtins::BuiltinRegistry` |
| `nuzo_error` | 结构化错误处理 | `NuzoError`, `NuzoErrorKind` | `error::NuzoError` |
| `nuzo_config` | 运行时配置加载与解析 | `Config` | `config::Config` |
| `nuzo_run` | 应用入口：Engine/Session/CLI/Bench/Test | `Engine`, `Session` | `engine::Engine::builder()` |
| `nuzo` | 根门面（薄转发层） | re-export | `lib.rs` |
| `nuzo_proc` | proc-macro 工具集 | `define_opcodes!` | proc-macro 入口 |
| `nuzo_class` | Rust 侧类语法糖（宏 re-export） | `class`, `class_impl` 等属性宏 | `class`, `class_impl` |
| `nuzo_proc_core` | proc-macro 核心类型 | 属性解析、诊断 | 内部类型 |
| `nuzo_class_macros` | `nuzo_class` 的过程宏实现 | `class`, `class_impl` 等 | proc-macro 入口 |
| `nuzo_codegen` | 代码生成：opcode/builtin/dispatch 自动生成 | `OpcodeGen`, `BuiltinGen`, `DispatchGen` | `codegen::generate()` |

---

## 5. 关键技术设计

### 5.1 NaN-tagged 值表示

`nuzo_values` 采用 IEEE 754 Quiet NaN 载荷空间实现动态类型标记：

```
┌──────────────────────────────────────────────────────┐
│ 位模式范围                    │ 类型     │ 说明       │
├──────────────────────────────────────────────────────┤
│ 0x7FF8_0000_0000_000[1-3]      │ 特殊值  │ nil/false/true │
│ 0x7FF8_4000_XXXX_XXXX          │ 堆对象  │ 数组/字典/闭包 │
│ 0x7FF8_8000_XXXX_XXXX          │ 字符串  │ 池化字符串引用 │
│ 0x7FF9_XXXX_XXXX_XXXX          │ Smi     │ 小整数 [-2^47, 2^47) │
│ 所有其他模式                   │ Float   │ 标准 f64        │
└──────────────────────────────────────────────────────┘
```

所有值统一为 8 字节 `Value`（`f64` 大小），类型检测 O(1) 位掩码测试，无需查表。

### 5.2 声明式 Opcode 系统

`nuzo_opcode` + `nuzo_proc` 通过 proc-macro 实现"定义一次，到处生成"：

```rust
nuzo_proc::define_opcodes! {
    #[opcode(code = 0, size = 1, operands = [], disasm = "halt")]
    Halt,
    #[opcode(code = 1, size = 7, operands = [Reg, Reg, Reg], disasm = custom)]
    Add,
    // ... 51 条
}
```

自动获得：枚举变体、指令大小、操作数类型、解码、反汇编模板、编译期大小校验。

### 5.3 增量 GC 架构

`nuzo_vm::gc` 采用 Region-Bump + SoA + ERSA 划痕区的增量标记-清除策略：
- **Region-Bump**：小对象快速分配
- **SoA**：结构体数组布局，利于扫描
- **ERSA**：划痕区用于安全访问已标记对象
- 通过 `Value::trace()` 递归标记根可达对象
- VM 在分配路径上检查阈值触发 `collect_gc_roots()`

### 5.4 性能优化栈

VM 层集成了多层性能优化：

| 层级 | 模块 | 技术 |
|------|------|------|
| L1 | `zero_unbox` | Smi 快路径，避免装箱（Smi 算术权威源在 `nuzo_core::tag`，此处 re-export） |
| L2 | `trf` | TypedRegFile，类型感知寄存器文件 |
| L3 | `cache` | InlineCache / StringPool / ShapeCache |
| L4 | `vm_lic` | 多级内联缓存调用系统 |
| L5 | `vm_hot_trace` | 热路径 trace 批量执行 |
| L6 | `object` | Shape-based 属性缓存 |

---

## 6. 快速修改清单

| 修改类型 | 最少文件数 | 典型耗时 | 风险等级 |
|----------|-----------|---------|---------|
| 新增 builtin 函数 | 2 | 10 min | 低 |
| 新增运算符 | 7 | 30 min | 中 |
| 新增数据类型 | 5 | 60 min | 中高 |
| 调整 GC 参数 | 2-4 | 15 min | 低 |
| 添加新 opcode | 4 | 30 min | 中 |
| 性能优化 | 视范围 | 可变 | 视范围 |