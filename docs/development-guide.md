# Nuzo Lang 开发指南 (Development Guide)

> 版本：0.6.0 | 22 packages（21 members + root facade）| L0-L6 严格分层 + root facade
>
> 面向 Nuzo Lang 仓库贡献者的实操指南：架构总览、执行链、添加新功能的流程、调试技巧与陷阱速查。

---

## 1. 架构总览

### 1.1 扁平化目录结构

```
nuzo_lang/
├── crates/                     # 扁平化布局，所有 crate 直接在 crates/ 下
│   ├── nuzo-proc-core/      L0     proc-macro 核心工具
│   ├── nuzo-codegen/        L0     代码生成（builtin/opcode/dispatch）
│   ├── nuzo-config/         L0     配置管理
│   ├── nuzo-class-macros/   L0     class 宏实现
│   ├── nuzo-core/           L1     常量、编码、SourceLocation
│   ├── nuzo-proc/           L1     proc-macro 入口
│   ├── nuzo-class/          L1     Rust 侧类语法糖
│   ├── nuzo-values/         L2-L3  NaN-tagged 值系统
│   ├── nuzo-opcode/         L2     opcode 定义框架
│   ├── nuzo-signal/         L2     信号槽系统
│   ├── nuzo-bytecode/       L3     字节码格式
│   ├── nuzo-frontend/       L3     lexer + parser + AST
│   ├── nuzo-helpers/        L3     builtin 函数注册表
│   ├── nuzo-ir/             L4     中间表示
│   ├── nuzo-error/          L4     错误诊断
│   ├── nuzo-compiler/       L5     AST→Bytecode 编译器
│   ├── nuzo-vm/             L5     虚拟机 + GC
│   ├── nuzo-run/            L6     Engine + Session + CLI（唯一公共 API）
│   └── nuzo-gui/            -      GUI 工具
├── tests/              集成测试 + e2e
├── scripts/            Python 同步脚本
├── tools/              nuzo_explore / perf-codegen 等独立工具
├── docs/               设计文档
├── examples/           .nuzo 示例
├── benchmarks/         基准测试
├── justfile            just 命令入口
├── Cargo.toml          workspace 根
├── DEVELOPMENT.md           项目约定文档
└── CALL_GRAPH.md       自动生成调用图（勿手改）
```

### 1.2 21 crate 一句话职责

| Crate | 层级 | 一句话职责 | 关键类型 |
|-------|------|----------|---------|
| nuzo_proc_core | L0 | proc-macro 共享工具与硬编码常量管理 | `HardcodeMap` |
| nuzo_config | L0 | 零依赖分层 TOML 配置管理 | `Config` |
| nuzo_class_macros | L0 | nuzo_class 的 proc-macro | `class_macro` |
| nuzo_codegen | L0 | 编译时代码生成（builtin/opcode/dispatch） | — |
| nuzo_core | L1 | 常量、编码、宏、源码位置 | `SourceLocation`, `InternalError` |
| nuzo_proc | L1 | proc-macro 入口（含 `define_opcodes!`） | `define_opcodes` |
| nuzo_class | L1 | Rust 端 class 语法糖 | `NuzoClass` |
| nuzo_opcode | L2 | 声明式 opcode 定义框架 | `Opcode`, `OperandKind` |
| nuzo_signal | L2 | 类型安全的信号槽系统 | `SignalBus` |
| nuzo_values | L2-L3 | NaN-tagged 值系统 | `Value`, `HeapObject` |
| nuzo_bytecode | L3 | 字节码格式与指令集定义 | `Chunk`, `Instruction` |
| nuzo_frontend | L3 | Lexer / Parser / AST | `Token`, `TokenKind`, `ast::Program` |
| nuzo_helpers | L3 | builtin 函数注册与实现 | `BuiltinRegistry` |
| nuzo_ir | L4 | 中间表示（IR） | `Ir`, `IrBuilder` |
| nuzo_error | L4 | 错误收集与诊断渲染 | `NuzoError`, `DiagnosticRenderer` |
| nuzo_compiler | L5 | AST → Bytecode 编译器 | `Compiler` |
| nuzo_vm | L5 | 寄存器机 VM + GC + 运行时 | `VM`, `dispatch_opcode_fast` |
| nuzo_run | L6 | Engine + Session + CLI + Bench + Test 统一入口 | `Engine`, `Session` |
| nuzo | root | Root facade crate（thin re-export） | re-exports |

层级依赖关系（违反 = 编译失败）见 [DEVELOPMENT.md §层级依赖](./DEVELOPMENT.md)。

---

## 2. 真实执行链

从命令行到字节码执行的完整路径（含文件路径 + 关键行号）：

```
nuzo_run::main()
    │
    │  crates/nuzo-run/src/main.rs:35
    │  fn main() -> 解析 args，分发 Command
    │
    ▼
Engine::quick() / Engine::builder().build()
    │
    │  crates/nuzo-run/src/engine.rs:160
    │  pub fn builder() -> EngineBuilder<WantsConfig>
    │  持有 SignalBus、VM、Config
    │
    ▼
Session::eval(source)
    │
    │  crates/nuzo-run/src/session.rs:46
    │  pub fn eval(&mut self, source: &str) -> NuzoResult<Output>
    │  清空输出捕获 → 编译 → 执行 → 计时
    │
    ▼
Compiler::compile_with_bus(source, bus)
    │
    │  crates/nuzo-compiler/src/compiler.rs:229
    │  pub fn compile_with_bus(source, bus) -> Result<Chunk, CompileError>
    │  ─ Parser::parse_with_timing()       → ast::Program
    │  ─ IrBuilder::build()                → Ir
    │  ─ optimize::optimize()              → Ir（优化后）
    │  ─ codegen::generate()               → Chunk
    │
    ▼
VM::run(chunk)
    │
    │  crates/nuzo-vm/src/vm.rs:517
    │  pub fn run(&mut self, chunk: Chunk) -> Result<Value, NuzoError>
    │  reset_and_load_chunk → registers.activate → run_inner
    │
    ▼
dispatch.rs::execute_inner(opcode)
    │
    │  crates/nuzo-vm/src/dispatch.rs:1419
    │  pub(crate) fn execute_inner(&mut self, opcode: Opcode) -> Result<(), NuzoError>
    │  调用 dispatch_opcode_fast(self, opcode) 进入 51 opcodes 的 match 分发
    │
    ▼
Value 返回 → Session 包装为 Output { value, stdout, duration }
```

**关键信号点**：
- `COMPILE_STARTED_KEY` 信号在 `compile_with_bus` 入口 emit
- `registers.activate()` / `deactivate()` 包裹 `run_inner`，管理寄存器上下文
- `execution_timeout_ms` 在 `run()` 入口记录起始时间，每个 opcode 检查

---

## 3. crate 职责表（完整版）

| Crate | 层级 | 路径 | 一句话职责 | 关键类型 |
|-------|------|------|----------|---------|
| nuzo_proc_core | L0 | `crates/nuzo-proc-core` | proc-macro 共享工具，硬编码常量管理 | `HardcodeMap` |
| nuzo_config | L0 | `crates/nuzo-config` | 零依赖分层 TOML 配置 | `Config`, `Layer` |
| nuzo_class_macros | L0 | `crates/nuzo-class-macros` | class 语法的 proc-macro | `class` |
| nuzo_codegen | L0 | `crates/nuzo-codegen` | 编译时代码生成（builtin/opcode/dispatch） | — |
| nuzo_core | L1 | `crates/nuzo-core` | 常量、编码、源码位置、内部错误 | `SourceLocation`, `InternalError` |
| nuzo_proc | L1 | `crates/nuzo-proc` | proc-macro 入口，提供 `define_opcodes!` | `define_opcodes` |
| nuzo_class | L1 | `crates/nuzo-class` | Rust 端 class 语法糖，桥接 Rust 类型到 Nuzo | `NuzoClass` |
| nuzo_opcode | L2 | `crates/nuzo-opcode` | 声明式 opcode 定义框架（仅宏与类型） | `Opcode`, `OperandKind`, `DispatchKind` |
| nuzo_signal | L2 | `crates/nuzo-signal` | 类型安全的信号槽系统 | `SignalBus`, `Signal` |
| nuzo_values | L2-L3 | `crates/nuzo-values` | NaN-tagged 值系统与堆对象 | `Value`, `HeapObject`, `CaptureMode` |
| nuzo_bytecode | L3 | `crates/nuzo-bytecode` | 字节码格式与指令集（调用 `define_opcodes!`） | `Chunk`, `Instruction` |
| nuzo_frontend | L3 | `crates/nuzo-frontend` | Lexer + Parser + AST（CJK 感知） | `Token`, `TokenKind`, `ast::Program` |
| nuzo_helpers | L3 | `crates/nuzo-helpers` | builtin 函数注册中心与 7 个 domain 实现 | `BuiltinRegistry`, `BuiltinFn` |
| nuzo_ir | L4 | `crates/nuzo-ir` | 中间表示，编译器优化阶段使用 | `Ir`, `IrBuilder`, `optimize` |
| nuzo_error | L4 | `crates/nuzo-error` | 错误收集、分类、诊断渲染 | `NuzoError`, `DiagnosticRenderer`, `StackFrameInfo` |
| nuzo_compiler | L5 | `crates/nuzo-compiler` | AST → IR → Bytecode 编译主流程 | `Compiler`, `compile_with_bus` |
| nuzo_vm | L5 | `crates/nuzo-vm` | 寄存器机 VM + GC + dispatch + 运行时 | `VM`, `dispatch_opcode_fast`, `Gc` |
| nuzo_run | L6 | `crates/nuzo-run` | Engine + Session + CLI + Bench + Test 统一入口 | `Engine`, `Session`, `EngineBuilder` |
| nuzo | root | `.` (root) | Root facade crate，thin re-export of nuzo_run | re-exports |

---

## 4. 添加新 opcode 流程（10 步）

> 参考：`crates/nuzo-opcode/src/lib.rs` 的 `define_opcodes!` 宏文档。

1. **定义 Instruction 变体**：在 `crates/nuzo-bytecode/src/opcode.rs` 的 `nuzo_proc::define_opcodes! { ... }` 块中添加新变体：

   ```rust
   #[opcode(code = 47, size = 7, operands = [Reg, Reg, Reg], disasm = "{dst} = foo {lhs}, {rhs}", desc = "Foo operation", summary = "")]
   FooOp,
   ```

   - `code`：u8 opcode 编号，必须唯一
   - `size`：指令总字节数 = 1(opcode) + Σ operand.byte_size()
   - `operands`：操作数类型列表（`Reg` / `Const` / `Imm8` / `Imm16` / `Imm32` 等）
   - `disasm`：反汇编格式字符串

2. **加 `#[opcode]` 属性**：宏会自动生成 `Opcode::FooOp` 枚举变体 + 编解码方法 + 反汇编实现。

3. **跑 sync-opcode-apply**：

   ```powershell
   just sync-opcode-apply
   ```

   `scripts/sync_opcode.py` 会检查 opcode 表一致性、生成常量表、更新文档片段。

4. **实现 dispatch handler**：在 `crates/nuzo-vm/src/dispatch.rs` 的 `dispatch_opcode_fast` match 中加分支：

   ```rust
   Opcode::FooOp { dst, lhs, rhs } => {
       let l = self.cx.registers.get(lhs);
       let r = self.cx.registers.get(rhs);
       let result = /* 你的实现 */;
       self.cx.registers.set(dst, result);
   }
   ```

5. **加 codegen**：在 `crates/nuzo-compiler/src/codegen/` 中处理对应 AST 节点，emit `Opcode::FooOp` 指令。

6. **接入 AST 分发**：在 `crates/nuzo-compiler/src/compiler.rs` 中找到对应 AST 节点的 visit 方法（如 `visit_binary_expr`），调用第 5 步的 codegen。

7. **如需新 AST 节点**：先改前端 → `token.rs`（加 TokenKind） → `lexer.rs`（扫描逻辑） → `parser.rs`（解析为 AST 节点） → `ast.rs`（节点结构）。

8. **加单元测试**：在 `crates/nuzo-vm/tests/` 或 `#[cfg(test)] mod tests` 加测试。

9. **加 e2e 测试**：在 `tests/e2e/` 加一个 `.nuzo` 文件，覆盖新 opcode 的语义。

10. **跑 sync-tests-apply + 全量验证**：

    ```powershell
    just sync-tests-apply
    just check && just test && just lint && just callgraph
    ```

---

## 5. 添加新 builtin 流程（8 步）

1. **选 domain 模块**：根据功能选择 `crates/nuzo-helpers/src/{array,string,math,io,time,convert,debug}.rs` 之一。

2. **定义函数**：在该 domain 文件中实现：

   ```rust
   pub fn my_func(args: &[Value]) -> Result<Value, NuzoError> {
       let x = args.get(0).ok_or_else(|| NuzoError::new("TypeError", "expected 1 arg"))?;
       // ... 实现
       Ok(result)
   }
   ```

3. **在 `register()` 中注册**：编辑 `crates/nuzo-helpers/src/builtins.rs` 的 `register` 函数：

   ```rust
   registry.register("my_func", BuiltinFn::new(my_func), 1);  // 1 = arity
   ```

4. **加入 `builtin_names()`**：如需 IR 识别（如管道操作符左侧），在 `crates/nuzo-helpers/src/lib.rs` 的 `builtin_names()` 中加名字。

5. **加单元测试**：在 domain 文件的 `#[cfg(test)] mod tests` 中测试函数行为。

6. **加 e2e 测试**：在 `tests/e2e/` 加一个 `.nuzo` 文件验证端到端行为。

7. **跑 sync-tests-apply**：

   ```powershell
   just sync-tests-apply
   ```

   `scripts/sync_tests.py` 会从 `CALL_GRAPH.md` 与 builtin 注册表生成测试桩。

8. **全量验证**：

   ```powershell
   just check && just test && just lint && just callgraph
   ```

---

## 6. 添加新 crate 流程（5 步）

1. **创建目录与 `Cargo.toml`**：

   ```
   crates/<layer>/nuzo_<name>/
   ├── Cargo.toml
   └── src/
       └── lib.rs
   ```

   `Cargo.toml` 参考：

   ```toml
   [package]
   name = "nuzo_<name>"
   version.workspace = true
   edition.workspace = true
   publish = false

   [dependencies]
   nuzo_core.workspace = true    # 按需添加，遵守层级
   ```

2. **注册到 workspace.members**：编辑根 `Cargo.toml`，在 `[workspace] members` 数组中按层级分组添加：

   ```toml
   members = [
       # base: L0-L1
       ...
       "crates/<layer>/nuzo_<name>",
       ...
   ]
   ```

3. **注册到 workspace.dependencies**：

   ```toml
   [workspace.dependencies]
   nuzo_<name> = { path = "crates/<layer>/nuzo_<name>" }
   ```

4. **更新 DEVELOPMENT.md**：在「Crate 列表」表格末尾添加一行。**勿手动重排层级表**——`scripts/sync_development.py` 会自动同步层级依赖图。

5. **跑同步与验证**：

   ```powershell
   just sync
   just callgraph
   just check && just test
   ```

> **层级硬约束**：低层（L0-L2）禁止依赖高层。`nuzo_error`（L4）不能依赖 `nuzo_compiler`（L5）。违反会被 Cargo 编译期拒绝或被 `scripts/sync_development.py` 检测出。

---

## 7. 调试技巧

### 7.1 cargo expand（看宏展开）

```powershell
cargo install cargo-expand
cargo expand -p nuzo_vm::dispatch execute_inner
cargo expand -p nuzo_bytecode opcode
```

`define_opcodes!` / `#[opcode]` / `HeapObject` 的 `MatchSync` 派生宏展开后能看清实际生成的 match 分支与编解码逻辑。

### 7.2 RUST_LOG 日志级别

项目使用 `log` crate。设置环境变量控制日志：

```powershell
$env:RUST_LOG="nuzo_vm=debug,nuzo_compiler=trace,nuzo_frontend::parser=info"
just run examples/your_file.nuzo
```

关键模块：
- `nuzo_vm::dispatch` — opcode 分发
- `nuzo_vm::gc` — GC 标记/扫描
- `nuzo_compiler::codegen` — 字节码生成
- `nuzo_frontend::parser` — AST 构建

### 7.3 NUZO_DUMP_IR

```powershell
$env:NUZO_DUMP_IR=1
just run examples/your_file.nuzo
```

会向 stderr 打印 IR 优化前 + 优化后两份 dump，对照可验证 IR 优化 pass 是否生效。

### 7.4 GDB / LLDB 断点

```
break nuzo_vm::vm::VM::run
break nuzo_vm::dispatch::execute_inner
break nuzo_compiler::compiler::Compiler::compile_with_bus
break nuzo_frontend::parser::Parser::parse
```

主循环在 `crates/nuzo-vm/src/vm.rs::VM::run_inner()`，单指令分发在 `crates/nuzo-vm/src/dispatch.rs:1419`。

### 7.5 VEH handler（Windows 栈溢出捕获）

VM 启用 VEH 捕获栈溢出 / 访问违例。调试时如需禁用：

```powershell
$env:NUZO_DISABLE_VEH=1
```

栈溢出通常意味着无限递归且未触发 TCO——优先检查 `compiler/functions.rs` 是否生成 `TailCall` opcode。

### 7.6 nuzo_callgraph 工具

独立工具 crate（位于 `d:\10\nuzo_callgraph`，非 workspace 成员）：

```powershell
just callgraph
```

生成 `CALL_GRAPH.md`，包含全 workspace 公开函数的调用关系。定位死代码 / 影响面分析 / 依赖回路的权威来源。

### 7.7 调试服务器

复杂运行时问题可通过调试服务器收集日志，遵循 假设 → 插桩 → 复现 → 分析 → 修复 → 验证 流程。

---

## 8. 测试目录结构

```
tests/
├── integration/                Rust 集成测试（.rs 文件）
│   ├── advanced_abstractions_tests.rs
│   ├── bug_boundary_tests.rs
│   ├── coverage_gaps.md
│   ├── ir_integration_tests.rs
│   ├── module_tests.rs
│   ├── pow_instruction_tests.rs
│   └── signal_integration.rs
├── e2e/                        端到端 .nuzo 测试（78 个文件）
│   ├── 01_basic.nuzo
│   ├── 02_arithmetic.nuzo
│   ├── 03_comparison.nuzo
│   ├── 04_data_structures.nuzo
│   ├── 05_array_index_assign_v2.nuzo
│   ├── 06_control_flow_if.nuzo
│   ├── 07_control_flow_loop.nuzo
│   ├── 08_functions.nuzo
│   ├── 09_recursion.nuzo
│   ├── 10_closures.nuzo
│   ├── 11_arrays.nuzo
│   ├── 12_dicts.nuzo
│   ├── 13_strings.nuzo
│   ├── 14_builtin_functions.nuzo
│   ├── 15_chinese_keywords.nuzo
│   ├── 16_defensive_programming.nuzo
│   ├── 17_comprehensive.nuzo
│   ├── 18_gc_pressure.nuzo
│   ├── 19_stress_100.nuzo
│   ├── 20_flat_200lines.nuzo
│   ├── advanced/               进阶特性
│   ├── debug/                  调试相关
│   ├── diagnostic/             诊断路径
│   ├── modules/                模块系统
│   ├── other/                  杂项
│   ├── perf/                   性能基准
│   ├── regression/             回归测试（bug1_xxx, bug2_xxx 等）
│   ├── stress/                 压力测试
│   └── supplement/             补充用例
├── generated/                  sync_tests.py 自动生成的测试桩
└── generated_stubs.rs          生成测试桩的 Rust 入口
```

### 8.1 添加 e2e 测试

1. 在 `tests/e2e/<category>/` 加 `your_test.nuzo`（命名遵循 `<编号>_<描述>.nuzo` 风格）。
2. 在 `.nuzo` 文件中写代码 + 期望输出（注释 `// EXPECT: <output>`）。
3. 运行 `just sync-tests-apply` 生成对应 Rust 测试桩。
4. `just test` 验证。

### 8.2 集成测试

Rust 集成测试位于 `tests/integration/*.rs`，通过根 `Cargo.toml` 的 `[[test]]` 注册：

```toml
[[test]]
name = "module_tests"
path = "tests/integration/module_tests.rs"
```

---

## 9. 同步脚本说明

`scripts/` 目录下的 Python 同步脚本，是 Nuzo Lang「单一真相源」策略的核心：

| 脚本 | 职责 | 何时运行 |
|------|------|---------|
| `sync_opcode.py` | 检查 `define_opcodes!` 宏定义的 opcode 表一致性，生成常量表与文档片段 | 改了 opcode 定义后 |
| `sync_tests.py` | 从 `CALL_GRAPH.md` + builtin 注册表生成 e2e 测试桩（Rust 包装代码） | 改了 builtin / opcode / 加了 .nuzo 测试后 |
| `sync_development.py` | 同步 `DEVELOPMENT.md` 的层级依赖图与 Crate 列表（基于 Cargo.toml 实际拓扑） | 改了 Cargo.toml workspace.members 后 |
| `generate_changelog.py` | 从 commit 信息生成 CHANGELOG.md | 发布版本前 |
| `run_regression.py` | 运行回归测试套件并比对结果 | 性能优化 / GC 改动后 |
| `beautify_directory.py` | 目录结构美化与一致性检查 | 一次性 / 偶发使用 |
| `reorganize.py` | 大规模目录重组工具 | 一次性 / 偶发使用 |
| `_common.py` | 共享工具（路径解析、日志、文件 IO） | 被其他脚本 import |

### 9.1 dry-run vs apply

每个 sync 脚本都支持 dry-run（默认，仅报告不修改）与 apply 模式：

```powershell
just sync-opcode              # dry-run，看报告
just sync-opcode-apply        # 实际修改文件
just sync-tests               # dry-run
just sync-tests-apply         # 实际生成
just sync                     # 一键 apply 全部 + callgraph + fmt
```

### 9.2 watch-sync 模式

```powershell
just watch-sync
```

监听文件变化，自动跑 `sync-opcode-apply` + `sync-tests-apply` + `check`。开发新 opcode / builtin 时的内环工具。

---

## 10. 常见陷阱（必读）

### 10.1 VM stack 必须 8 MB

`.cargo/config.toml` 中固定 `/STACK:8388608`。**不要**在开发机随意改：

- VM 递归调用 / 闭包嵌套深度依赖 8 MB 栈
- 改小 → 栈溢出 VEH 触发概率上升，正常用例也会崩
- 改大 → Windows 链接器拒绝或可执行文件膨胀

### 10.2 Arena 对象用 `Vec::remove(idx)`，不能用 `swap_remove`

堆对象池（Arena）按索引访问，**索引顺序敏感**：

```rust
// 正确
pool.remove(idx);              // O(n)，保持后续索引不变

// 错误（会破坏其他引用）
pool.swap_remove(idx);         // 把最后一个搬到 idx，导致 dangling 引用
```

GC remap 阶段会更新索引，但 swap_remove 后的中间状态会破坏不变量。

### 10.3 `-0.0` 必须 normalize

NaN-tagged 值系统中，`-0.0` 与 `0.0` 的位模式不同：

```rust
let v: f64 = -0.0;
let v = if v == 0.0 { 0.0 } else { v };   // normalize
```

未 normalize 会导致 `==` 比较异常 / GC mark 阶段位模式冲突。`Value::from_number()` 内部已处理，**禁止 transmute**。

### 10.4 `nuzo_error` 不能依赖 `nuzo_compiler`

层级硬约束：`nuzo_error`（L4）禁止依赖 `nuzo_compiler`（L5）。反向依赖导致：

- Cargo workspace 编译失败
- `scripts/sync_development.py` 检测出并报错
- 拉低层级后导致其他 crate 间接污染

错误类型走 `nuzo_error::NuzoError`，编译器内部错误用 `CompileError`（在 `nuzo_compiler` 内定义）。

### 10.5 CALL_GRAPH.md 勿手改

`CALL_GRAPH.md` 由 `just callgraph` 调用 `nuzo_callgraph` 工具自动生成。手改的内容会在下次同步时被覆盖。如发现调用图不准确，**改源代码**而非改 markdown。

### 10.6 GC 安全点间禁止持有裸指针

dispatch 间隙是 GC 安全点。安全点之间：

```rust
// 错误
let ptr = heap_obj as *const HeapObject;
do_something();              // ← 此处可能触发 GC，ptr 失效
(*ptr).field                 // ← dangling

// 正确
let idx = heap_obj.index();  // 持有索引而非指针
do_something();
let obj = pool.get(idx);     // 重新解析
```

新增 `HeapObject` 变体必须实现 `HeapObjectOps::trace_gc_refs()`，否则 GC 漏标记 → 内存损坏。

### 10.7 编译错误必须保留源码位置

编译错误（`CompileError`）必须带 `line` + `column` + 文件路径：

```rust
// 正确
return Err(CompileError::ParseError {
    message: "expected ')'",
    line: 12,
    column: 5,
});

// 错误（违反硬约束）
return Err(CompileError::Generic("syntax error"));   // 缺位置，渲染为 C0000
```

`C0000` 是「无位置错误」的兜底码，仅用于内部 bug，不应出现在用户可见错误中。

### 10.8 不要在 PR 中混入大量 `unwrap()` / `clone()`

clippy 严格模式 (`-D warnings`) 会拒绝：

- 非测试代码中的 `unwrap()` / `expect()`（除非有 `// SAFETY:` 注释证明不变量）
- 不必要的 `clone()`（性能瓶颈来源）
- `pub use xxx::*`（破坏 API 边界）

如果 clippy 报错难以解决，优先重构而非 `#[allow(...)]` 抑制。

---

## 11. 进一步阅读

| 文档 | 内容 |
|------|------|
| [DEVELOPMENT.md](./DEVELOPMENT.md) | 项目约定文档，含跨文件修改链与反模式清单 |
| [CALL_GRAPH.md](../CALL_GRAPH.md) | 自动生成的完整调用图（唯一权威） |
| [ARCHITECTURE.md](./ARCHITECTURE.md) | 架构总览与编译管线详解 |
| [language-spec.md](./language-spec.md) | Nuzo Lang 语言规范 |
| [CONTRIBUTING.md](./CONTRIBUTING.md) | 贡献指南（含 just 命令速查） |
| [standard-library.md](./standard-library.md) | 标准库 builtin 函数清单 |
