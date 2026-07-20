# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.6.0] - 2026-07-20

### Added

- **ErrorSink trait + ErrorCollector** (`nuzo_error`): 结构化错误收集系统。`ErrorSink` trait 定义错误事件处理接口，`ErrorCollector` 使用 crossbeam 无锁队列（SegQueue）实现零锁竞争的错误传递。`ErrorSinkObserver` adapter 在 `nuzo_vm` 中桥接 VM 信号总线与错误收集器。
- **OP_INIT_MODULE lazy import** (`nuzo_bytecode`/`nuzo_compiler`/`nuzo_vm`): 新增 InitModule opcode（code=30），支持延迟模块初始化。编译器在首次引用模块符号时发射 InitModule，运行时通过 name-based init flag 确保模块仅初始化一次。编译期 DAG 环检测，CircularImport 保留源码位置。
- **Register Spill 机制** (`nuzo_bytecode`/`nuzo_compiler`/`nuzo_vm`): 新增 SpillLoad（code=54）和 SpillStore（code=55）opcode，处理寄存器分配溢出。spill_stack 挂载到 CallFrame 并纳入 GC roots。编译器使用 DualPool 寄存器分配器 + LSRA spill 决策，两阶段插入 spill 指令并修正跳转偏移。
- **ExprVisitor trait** (`nuzo_frontend/src/ast.rs`): 泛型 AST 遍历器 trait，统一所有对 `Expr` 树的递归遍历逻辑。使用泛型 + 单态化实现零开销抽象。`nuzo_ir::builder::FreeVarCollector`、`nuzo_ir::builder::AssignedVarCollector`、`nuzo_compiler::functions::IdentifierCollector`、`nuzo_compiler::functions::CompilerAssignedVarCollector` 均已迁移至此接口。
- **运算符映射 SSOT** (`nuzo_ir/src/types.rs`): 新增 `From<ast::BinaryOp> for IrBinOp` 和 `From<ast::UnaryOp> for IrUnaryOp` 实现，作为 AST 运算符到 IR 运算符的唯一映射定义。编译器层和 IR 层均通过 `.into()` 调用此映射，消除重复的 inline match。
- **新增公开 API** (`nuzo-values`): `ValueExt::try_as_smi(&self) -> Result<i64, NuzoError>`——L2 扩展方法,作为 `as_smi`(panic 版)的安全替代;`is_nil` 已实现于 `nuzo-core`(通过 re-export 暴露),本次补测试验证
- **回归测试**: 新增 11 个回归测试覆盖本次技术债清理修复(nuzo-values 6 / nuzo-compiler 3 / nuzo-proc-core 2)
- **Nuzo Playground 在线试用环境** (`crates/nuzo-playground-wasm` + `playground-web/`): 浏览器内编辑并运行 Nuzo 脚本，CodeMirror 6 语法高亮 + Web Worker 隔离执行。wasm-pack 输出 `nuzo_playground_wasm.js`/`.wasm`，前端 Vite + 原生 JS（无框架）。GitHub Pages CI 自动部署（`.github/workflows/playground-pages.yml`）。e2e 验证 4/4 通过：Hello World / 语法错误 / 运行时错误 / Unicode 输出。
- **ModuleResolver trait 抽象** (`nuzo-run/src/module_resolver.rs`): 解耦 `Engine` 对 `std::fs` 的直接依赖。`FsResolver`（原生默认）保留原有行为；`MemoryResolver`（wasm）支持通过 `add_module(path, source)` 注入虚拟模块。`Engine::builder().with_resolver(resolver)` 注入。
- **OutputSink trait** (`nuzo-run/src/output_sink.rs`): 抽象 stdout/stderr 输出。`StdoutSink`（原生）+ `StringSink`（wasm 捕获）。Engine::eval 内部复用 `OutputSink::new_capture()` 自动捕获输出到 `Vec<String>`。
- **nuzo-run wasm feature** (`nuzo-run/Cargo.toml`): 新增 `[features] wasm = []`，配合 `#[cfg(target_arch = "wasm32")]` gate fs/env 调用，使 nuzo_run 可在 wasm32-unknown-unknown 编译。
- **CallGraph 工具同步** (`Cargo.toml`/`CALL_GRAPH.md`): workspace 新增 `nuzo-playground-wasm` 成员；CALL_GRAPH 重新生成（1509 函数 / 1070 边）。

### Changed

- **Smi 算术统一**: `nuzo_vm::zero_unbox` 中的 `smi_add`、`smi_sub`、`smi_mul`、`smi_to_i64` 改为从 `nuzo_core::tag` 重导出，消除 L1 与 L5 之间的逻辑重复。
- **AST 遍历去重**: 消除 `nuzo_ir::builder` 和 `nuzo_compiler::functions` 中约 320 行重复的手写递归 AST 遍历代码，统一使用 `ExprVisitor` trait。
- **ARCHITECTURE.md 更新**: 运算符添加指南从 5 步扩展至 7 步（新增 IR `From` trait 和编译器映射步骤），Smi 文档注明 `nuzo_core::tag` 为权威源。
- **Bug Archive 补归档**: 在 `project_memory.md` 新建 `## Bug Archive` 段并归档 8 条代码标记但 memory 未归档的孤儿（BUG-001/002/003/004/005/A/B/D + P2 BUG-signal-emit_count-clone + P2 BUG-connection-no-drop）。注：此前 memory 中 Bug Archive 段不存在，本次为首次建立。
- **`nuzo-values` 数组/字符串索引热路径**: `heap.rs` 与 `traits.rs` 中的索引校验路径由 `as_smi`(panic 版)改用 `try_as_smi`,错误路径不再 panic
- **`nuzo-proc-core` error_kind 错误信息**: 缺失 `category` 属性的错误信息从混淆的 "missing #[error(...)]" 改为精确的 "missing required attribute `category` in #[error(...)]"
- **`nuzo-compiler` codegen `rewrite_regs_with_lsra` 重写**: 用 `decode_operand_fields` 替代手动偏移累加,增加 `is_remappable_reg` 范围检查防御 ConstIdx 误判为 Reg

### Fixed

- **nuzo-values**: `tag_registry::check_conflict_inner` Rule 2 用 `new_tag & new_mask` 归一化修复 Smi 与 String 标签冲突检测漏检(原 `== new_tag` 未归一化导致漏报)
- **nuzo-compiler**: codegen P2.4 TODO(指令编码无操作数字段类型)已缓解:新增 `OperandField` 类型化标注 + `is_remappable_reg` 防御性范围检查;完整修复需跨 crate 审计 `Opcode::operands()`,已转 backlog
- **nuzo-proc-core**: error_kind 缺失 required 属性时未报告具体属性名(BUG #7)——引入 `has_error_attr` 标志精确区分"无 #[error] 属性"与"#[error] 缺 category"
- **nuzo-proc-core**: error_kind 未报告 missing `#[error(...)]` 原因(BUG #8)——错误信息追加 "required to generate Display impl, default_severity(), and default_category()"
- **bench_signal 并发 emit 误伤** (`nuzo-signal/src/signal.rs`): 旧版 `emitting: Arc<AtomicBool>` 共享于所有线程，`swap(true, SeqCst)` 在跨线程并发 emit 时误判为"递归"，导致 `bench_concurrent_emit` 实际仅约 21% 成功。改为 `thread_local` 静态变量 `EMITTING: Cell<bool>`，仅同线程可见，完美匹配递归保护的单线程语义。不同线程 emit 互不干扰。
- **BUG-CALL-CURRENT-BASE 寄存器源地址漏偏移** (`nuzo-vm/src/dispatch/calls.rs`): `setup_closure_frame` (L155) 和 `execute_closure_fast` (L226) 的参数寄存器源地址计算遗漏 `current_base` 偏移，导致嵌套调用（`current_base > 0`）时 `src_start = func_reg + 1` 指向调用者帧之外的旧寄存器，读到 nil 或错误值。顶层调用巧合正确掩盖了 bug。修复：使用 `caller_func_reg_abs + 1` 替代 `func_reg + 1`（参考 `tail_call.rs:195` 既有正确写法）。新增 3 个回归测试在 `vm_tests.rs:2204-2290`。
- **playground-web vite worker format 配置缺失** (`playground-web/vite.config.js`): 模块 Worker（`new Worker(..., { type: 'module' })`）触发 code-splitting，但 Vite 默认 worker format 为 `iife` 不支持 code-splitting，导致 `npm run build` 报 `Invalid value "iife" for option "output.format"`。新增 `worker: { format: 'es' }` 配置。

## [0.5.0] - 2026-07-04

### Added

- **TurboSlab 自研精简 Slab 分配器** (`crates/nuzo-values/src/turboslab/`): 替代旧 HEAP_POOL，提供槽位回收扩展点（去掉 SIMD AVX2 / NUMA / 自适应着色以避免超量工程）
- **文档同步基础设施**: `doc-sync-macros` 与 `auto-update-docs` spec 落地，建立代码 ↔ 文档自动同步链路
- **e2e 测试集成**: `fix-low-hanging-fruit-all` / `fix-readme-accuracy` 推进端到端测试稳定化与回归运行器
- **类型推断工具与 `.nuzo.stub` 接口存根**: `type-infer-tool` / `extract-generic-abstractions` spec 落地
- **Opcode 自动生成链路**: `opcode-auto-gen` spec，SSOT 宏驱动 opcode 全链路同步

### Changed

- **VM 审查 8 项修复**: 强化错误报告 (`strengthen-error-reporting`)、运行时错误源位置 (`runtime-error-source-location`)、统一调试系统 (`unified-debug-system`) 等 8 项审查整改
- **文档刷新**: `refresh-documentation` spec 全面校准 README / ARCHITECTURE

### Fixed

- **BUG-002**: `HEAP_POOL` 孤立 `Arc<HeapObject>` 条目堆积导致内存泄漏与索引空间耗尽——TurboSlab 根治，引入 free_list 与 `reclaim_orphaned` 槽位回收
- **BUG-003**: `op_test` 跳转目标未无条件验证（与 `op_jmp` 行为不一致），现统一校验
- **BUG-004**: 寄存器越界 (`RegisterOutOfBounds`) 边界处理在 `variable_ops.rs` / `dispatch.rs` 中加固
- **BUG-005**: `ArrayNew` 元素计数操作数为 `u16`，count = `u16::MAX` 时返回 `RegisterOverflow` 而非截断；同步修复 `collect_identifiers_from_expr` 嵌套函数体遍历不对称

## [0.4.0] - 2026-06-XX

### Added

- **类型系统 v7 设计与 `.nuzo.stub` 接口存根文件**: 面向静态类型推断的存根机制
- **IR 层抽象**: `ir-layer` spec，独立 `nuzo_ir` crate 沉淀中端 IR 与 builder
- **NuzoClass 面向对象扩展**: `nuzo-class` spec v2，class / method / inherit 语法支持

### Changed

- **编译器优化研究统一**: `unify-all-perf-optimizations` / `research-compiler-optimizations` 整合优化策略
- **LSRA 寄存器分配调优**: `lsra-integration-255-tuning` / `nud-enhanced-lsra` 调整线性扫描分配器
- **信号槽全工作区采用**: `adopt-signal-slot-everywhere` 统一事件总线
- **xxHash 替换 HashMap 哈希**: `adopt-xxhash-for-hashmap` 提升散列性能

### Fixed

- **代码异味 P0/P1 修复**: `fix-code-smells-p0-p1` 整治可维护性问题

### Removed

- **弃用 API 生命周期清理**: `deprecation-lifecycle-cleanup` 移除历史遗留接口

## [0.3.0] - 2026-05-XX

### Added

- **IR 优化层**: `nuzo_ir` crate 沉淀中端 IR 与 builder / FreeVarCollector / AssignedVarCollector
- **Hot Trace JIT 热点检测引擎**: `vm_hot_trace.rs` 循环热点检测与内联缓存命中统计
- **MLIC 多态内联缓存**: `vm_lic.rs` IC 快速路径，CallSiteState FVM + FNV 哈希查找
- **Incremental GC 增量回收**: `integrate-gc-mainline` 推进 GC 主线化
- **TurboSlab allocator 原型**: 早期实验性 slab 分配器（v0.5.0 才正式落地）
- **TCO 尾调用优化**: `tco-gc-optimization` spec 落地

### Changed

- **运行时主循环重构**: 集成 hot_trace + LIC 调度进入执行循环
- **GC 重写**: 主垃圾回收器重写（+892 行）
- **分发表直接跳转**: Direct Dispatch Table 替代 39 路 match 分发

### Fixed

- **e2e / GC / SP / Value 多类运行时 bug 修复**: `fix-e2e-gc-sp-and-value-bugs`
- **数组 watermark 热修复**: `hotfix-array-watermark`
- **Nuzo 脚本运行时 bug 修复**: `fix-nuzo-script-runtime-bugs`
- **5 项 stress e2e 测试修复**: `fix-five-stress-e2e-tests` / `fix-stress-perf-e2e-tests`

## [0.2.0] - 2026-04-XX

### Added

- **早期 Opcode 系统建立**: `nuzo_bytecode` / `nuzo_opcode` crate 分离
- **Bytecode 编解码模块**: `bytecode/constants.rs` / `operand.rs` / `instructions.rs` 集中编码常量与类型安全操作数
- **`define_opcodes!` SSOT 宏**: 单一真理源驱动 opcode 定义、编解码、分发
- **proc-macro 双 crate 架构**: `nuzo_proc` (过程宏入口) + `nuzo_proc_core` (共享逻辑)，规避循环依赖
- **错误系统分层**: `nuzo_error` 独立 crate（禁止反向依赖 `nuzo_compiler`）

### Changed

- **寄存器宽度 u8 → u16**: 支持最多 65535 个寄存器
- **编译器 / VM 指令格式一致性统一**

### Fixed

- **编译器根因修复**: `fix-compiler-root-cause` spec
- **错误系统分层化**: `fix-error-system` spec
- **架构 bug A2 / A6 修复**: `fix-architecture-bugs-a2-a6` spec
- **循环性能 bug**: `fix-loop-performance` spec

## [0.1.5] - 2026-05-30

### Added

- **Direct Dispatch Table** (`dispatch_table.rs`): Function-pointer array replacing 39-way match dispatch — O(1) indexed dispatch with optimal branch prediction
- **Hot Trace Engine** (`vm_hot_trace.rs`): Loop hotspot detection, inline cache hit statistics, hot-path optimization decisions (967 lines)
- **Inline Cache** (`vm_lic.rs`): IC fast path for property/index access with CallSiteState FVM and FNV hash lookup (1353 lines)
- **Helpers Standard Library**: 7 new modules providing built-in functions for Nuzo programs:
  - `helpers/array.rs`: map, filter, reduce, sort, join, push, pop, reverse, slice
  - `helpers/string.rs`: split, trim, replace, format, upper, lower, contains, len
  - `helpers/math.rs`: floor, ceil, round, abs, min, max, clamp, sqrt, pow, log, sin, cos
  - `helpers/convert.rs`: to_int, to_float, to_string, to_bool, parse_int, parse_float
  - `helpers/io.rs`: read_file, write_file, file_exists, print, input
  - `helpers/time.rs`: clock, sleep, time, date
  - `helpers/debug.rs`: type_of, inspect, assert, assert_eq, benchmark
- **Performance Regression Test Framework**: Statistics-driven performance regression detection:
  - `testkit/perf_regression.rs` (2446 lines): t-test significance testing, confidence intervals, baseline comparison
  - `testkit/statistics.rs` (1466 lines): mean/variance/stddev/percentiles/Cohen's d effect size
  - `testkit/timeout_alarm.rs`: Test timeout alarm mechanism
  - `testkit/baseline.rs`: Performance baseline read/write management
- **Standardized Integration Tests**: 20 `.nuzo` test cases covering basic types, arithmetic, comparison, logic, variables, control flow, functions, recursion, closures, arrays, dicts, strings, builtins, Chinese keywords, error handling, comprehensive tests, GC pressure, stress tests, and benchmarks
- **Bytecode Assert Test Suite** (`tests/bytecode_assert_tests.rs`, 572 lines): Compile → disassemble → assert verification pipeline
- **CI Workflow**: GitHub Actions performance regression workflow (`perf-regression.yml`)
- **Utility Scripts**: `fix_build_rs.py`, `patch_build_rs.py`, `fix_registry.py`, `fix_set_register.py`, `update_checksums.py`

### Changed

- **GC Rewrite** (`gc.rs`, +892 lines): Major garbage collector overhaul with improved stability
- **VM Main Loop Refactor** (`vm.rs`, +1144 lines): Integrated hot_trace + LIC scheduling into execution loop
- **Dispatch Adaptation** (`dispatch.rs`, +208 lines): Legacy dispatch logic adapted for new direct-dispatch table
- **Lexer Enhancement** (`lexer.rs`, +214 lines): Extended tokenizer capabilities
- **Opcode Extension** (`opcode.rs`, +50 lines): Added Mod/Pow opcode support
- **Workspace Reorganization**: Migrated from monolithic crate to multi-crate workspace (13 crates)

### Fixed

- **BOM Encoding**: Removed UTF-8 BOM from `dispatch_table.rs` causing compilation failure

## [0.1.2] - 2026-05-17

### Added

- **Value Module Unification**: Merged 6 sub-modules (errors, function, heap, memory, nuzo_dict) back into unified `mod.rs`
- **CALL_GRAPH.md**: Complete architecture call graph documentation (1413 lines, Mermaid diagrams)
- **Test Toolkit** (`src/testkit/`): bytecode_assert, e2e_runner, tracer, inspector, error_replay, nuzo_test_macro
- **Nuzo Integration Tests** (`nuzo_tests/`): 30+ Nuzo language integration test cases
- **Diagnostic Tools**: deep_diagnosis, diagnose_bytecode_diff, gc_bench examples
- **Watch Scripts**: Development auto-rebuild scripts (`scripts/watch.ps1`, `watch.sh`)

### Changed

- **Compiler Improvements**: Register allocation, expression compilation, error handling optimization
- **VM Enhancements**: Dispatch scheduling, GC stability, error collector improvements
- **Cargo Workspace Split**: From single crate to 13 independent crates

### Removed

- Deprecated docs: BUG_REPORT.md, ARCHITECTURE_OPTIMIZATION.md, linear-scan design documents
- Nuzo runtime experimental code

## [0.1.1] - 2026-05-17

### Added

- **Bytecode Encoding Module**: Created a new `bytecode` module to centralize all encoding-related data structures and constants:
  - `bytecode/constants.rs`: Encoding constants (register limits, instruction sizes, etc.)
  - `bytecode/operand.rs`: Operand format definitions with type-safe enum
  - `bytecode/instructions.rs`: Instruction metadata with complete descriptions
  - `bytecode/mod.rs`: Module entry with re-exports

### Changed

- **Error Handling Enhancement**: Added disassembly display when runtime errors occur, helping developers debug bytecode issues
- **Register Support**: Expanded register support from u8 to u16, enabling up to 65535 registers
- **Code Organization**: Improved code structure for better maintainability

### Fixed

- **Instruction Format Consistency**: Fixed inconsistencies in instruction encoding between compiler and VM
- **Disassembly Accuracy**: Updated disassembler to correctly parse 16-bit register operands

## [0.1.0] - Initial Release

- Core Nuzo runtime implementation
- Bytecode compiler and VM
- Basic standard library functions
- REPL and script runner utilities

<!-- 版本链接锚点 -->
[Unreleased]: https://github.com/nuzo-lang/nuzo_lang/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/nuzo-lang/nuzo_lang/releases/tag/v0.5.0
[0.4.0]: https://github.com/nuzo-lang/nuzo_lang/releases/tag/v0.4.0
[0.3.0]: https://github.com/nuzo-lang/nuzo_lang/releases/tag/v0.3.0
[0.2.0]: https://github.com/nuzo-lang/nuzo_lang/releases/tag/v0.2.0
[0.1.5]: https://github.com/nuzo-lang/nuzo_lang/releases/tag/v0.1.5
[0.1.2]: https://github.com/nuzo-lang/nuzo_lang/releases/tag/v0.1.2
[0.1.1]: https://github.com/nuzo-lang/nuzo_lang/releases/tag/v0.1.1
[0.1.0]: https://github.com/nuzo-lang/nuzo_lang/releases/tag/v0.1.0