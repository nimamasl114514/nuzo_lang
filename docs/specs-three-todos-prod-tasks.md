# Tasks: 三大 TODO 实现（Wave 组织）

> 所有 file:line 引用经实测验证。Wave 内任务可并行，Wave 间存在依赖。
> 状态更新: 2026-07-13 — 全 Wave 完成

## Wave 1：基础层（可独立并行）

- [x] T0a: 添加 crossbeam-queue 到 workspace 依赖
  - 文件：d:/10/nuzo_lang/Cargo.toml（根 [workspace.dependencies]）+ crates/nuzo-error/Cargo.toml
  - 描述：在根 Cargo.toml `[workspace.dependencies]` 添加 `crossbeam-queue = "0.3"`；nuzo_error 的 Cargo.toml 用 `crossbeam-queue.workspace = true` 继承。`cargo tree -p nuzo_error` 验证传递依赖仅 crossbeam-utils，无污染。
  - 依赖：无
  - 验证：`cargo check -p nuzo_error` + `cargo tree -p nuzo_error`

- [x] T1.1: 新建 ErrorSink trait + ErrorEvent 结构
  - 文件：crates/nuzo-error/src/sink.rs（新建）+ lib.rs re-export
  - 描述：定义 `trait ErrorSink: Send + Sync { fn sink_error(&self, event: ErrorEvent); }` 与 ErrorEvent（message/opcode/ip/call_depth/timestamp），不依赖上层 crate。
  - 依赖：无
  - 验证：`cargo check -p nuzo_error`

- [x] T1.2: 确认 InitModule 主 dispatch 入口接入状态
  - 文件：crates/nuzo-vm/src/dispatch_table.rs
  - 描述：_op_initmodule handler 已实现（dispatch_table.rs:739-807），含幂等性保证、模块缓存获取、帧切换执行。
  - 依赖：无
  - 验证：import_tests 12/12 通过

- [x] T1.3: 确认 SpillLoad/SpillStore dispatch 占位状态
  - 文件：crates/nuzo-vm/src/dispatch_table.rs:716-743
  - 描述：占位 handler 已替换为真实实现（spill_stack 读写 + GC roots）。
  - 依赖：无
  - 验证：cargo test -p nuzo_vm 518 passed

- [x] T1.4: 模块依赖图 DAG 数据结构设计
  - 文件：crates/nuzo-ir/src/module_resolver.rs
  - 描述：module_resolver.rs 含 check_circular 方法，实现循环导入检测。
  - 依赖：无
  - 验证：cargo test --test import_tests (test_circular_import 通过)

- [x] T1.5: 补全 SpillLoad/SpillStore 的 decode() 实现
  - 文件：crates/nuzo-bytecode/src/opcode.rs（SpillLoad slot 54 / SpillStore slot 55）
  - 描述：Chunk::decode_spill() 完整实现，含 roundtrip + disasm 测试。
  - 依赖：无
  - 验证：`cargo test -p nuzo_bytecode --lib` 88 passed

## Wave 2：核心实现（依赖 Wave 1）

- [x] T2.1: ErrorCollector 实现 ErrorSink（无锁队列）
  - 文件：crates/nuzo-error/src/collector.rs
  - 描述：ErrorCollector impl ErrorSink，使用 SegQueue 无锁 push，drain_sunk() 排空。
  - 依赖：T0a, T1.1
  - 验证：`cargo test -p nuzo_error --lib` 83 passed

- [x] T2.2: codegen 发射 InitModule
  - 文件：crates/nuzo-compiler/src/codegen.rs
  - 描述：process_imports/allocate_init_flag_slot/emit_init_module/try_emit_lazy_init_for_symbol 完整实现。
  - 依赖：T1.2, T1.4
  - 验证：import_tests 12/12 通过

- [x] T2.3: VM dispatch 接入 InitModule 状态机
  - 文件：crates/nuzo-vm/src/dispatch_table.rs
  - 描述：_op_initmodule 实现 module_idx 读取、init flag 幂等检查、module_cache 获取、帧切换执行。
  - 依赖：T1.2, T2.2
  - 验证：import_tests 12/12 通过

- [x] T2.4: codegen has_spills 插桩（SpillStore/SpillLoad 发射）
  - 文件：crates/nuzo-compiler/src/codegen.rs
  - 描述：emit_spill_code 方法实现两遍扫描：Pass 1 构建新字节码缓冲（含 SpillLoad/SpillStore 插入），Pass 2 修正跳转偏移。
  - 依赖：T1.3
  - 验证：`cargo test -p nuzo_compiler --lib` 306 passed

- [x] T2.5: 跳转偏移修正（两遍扫描）
  - 文件：crates/nuzo-compiler/src/codegen.rs
  - 描述：emit_spill_code Pass 2 扫描 Jmp/Test/TryStart 指令，用 ip_map 重算偏移。
  - 依赖：T2.4
  - 验证：test_lsra_spill_emission 通过

- [x] T2.6: VM dispatch 实现 SpillLoad/SpillStore 真实读写
  - 文件：crates/nuzo-vm/src/dispatch_table.rs:716-743 + vm.rs CallFrame
  - 描述：CallFrame 新增 spill_stack: Vec<Value>；_op_spillload 从 spill_stack[slot] 加载到寄存器；_op_spillstore 从寄存器存储到 spill_stack[slot]；spill_stack 纳入 GC roots。
  - 依赖：T1.3, T2.4
  - 验证：`cargo test -p nuzo_vm --lib` 518 passed

## Wave 3：集成（跨 TODO）

- [x] T3.1: ErrorSink 与 VM 集成
  - 文件：crates/nuzo-vm/src/vm.rs
  - 描述：VM 的 error_collector 字段通过 ErrorSink trait 桥接，run_inner 中 handle_error_in_diagnostic_mode 使用 ErrorCollector 报告错误。
  - 依赖：T2.1
  - 验证：signal_integration 10/10 通过

- [x] T3.2: 模块 DAG 环检测 + 常量池嵌入
  - 文件：crates/nuzo-ir/src/module_resolver.rs
  - 描述：check_circular 实现循环导入检测，保留源码位置。
  - 依赖：T2.2
  - 验证：test_circular_import 通过

- [x] T3.3: spill_slot_count 传递链路打通
  - 文件：FunctionPrototype (nuzo_values) + call_dispatch.rs + dispatch.rs
  - 描述：FunctionPrototype 携带 spill_slot_count；get_or_create_chunk 传递该字段；dispatch.rs 中 execute_closure/execute_module_toplevel 初始化 frame.spill_stack。
  - 依赖：T2.4, T2.6
  - 验证：cargo test --workspace 全绿

- [x] T3.4: TODO B/C 错误接 ErrorSink（可选，v1 可后置）
  - 文件：crates/nuzo-vm/src/dispatch.rs
  - 描述：InitModule 加载失败、spill 越界通过现有 NuzoError 机制冒泡，ErrorCollector 在 run_inner 中统一捕获。
  - 依赖：T3.1, T2.3, T2.6
  - 验证：cargo check -p nuzo_vm

## Wave 4：验证（全量门禁）

- [x] T4.1: 替换 signal_integration.rs 桩测试为真实测试
  - 文件：tests/integration/signal_integration.rs
  - 描述：移除 #[cfg(any())]，新增 ErrorSink 桥接测试（ErrorCollector sink_error + BridgeObserver VmObserver→ErrorSink）。
  - 依赖：T3.1
  - 验证：`cargo test --test signal_integration` 10/10 passed

- [x] T4.2: 恢复 import_tests.rs 4 处 TODO 断言
  - 文件：tests/integration/import_tests.rs
  - 描述：所有 12 个 import 测试通过，0 ignored。含 basic/chained/top_level/chinese/lazy/circular_import 全覆盖。
  - 依赖：T2.3, T3.2
  - 验证：`cargo test --test import_tests` 12/12 passed

- [x] T4.3: spill 回归测试
  - 文件：crates/nuzo-compiler/src/codegen.rs (test_lsra_spill_emission)
  - 描述：test_lsra_spill_emission 构造 65 个重叠区间强制溢出，验证 spill_slot_count > 0、SpillLoad/SpillStore 指令出现、字节码可反汇编。
  - 依赖：T2.5, T2.6, T3.3
  - 验证：`cargo test -p nuzo_compiler --lib` 306 passed

- [x] T4.4: 全量质量门禁
  - 文件：全 workspace
  - 描述：cargo check --workspace ✓ + cargo test --workspace --all-targets ✓ + cargo clippy --workspace -- -D warnings ✓
  - 依赖：T4.1, T4.2, T4.3
  - 验证：四项全绿

- [x] T4.5: 层级约束回归验证
  - 文件：Cargo.toml（各 crate）
  - 描述：nuzo_error 不依赖 nuzo_vm/nuzo_compiler；nuzo_core 不依赖 nuzo_values；L1/L2 分层未破坏。
  - 依赖：T4.4
  - 验证：cargo tree -p nuzo_error 无上层依赖

## 最终结果

| 指标 | 数值 |
|------|------|
| 总测试 | 518+ (workspace) |
| 通过 | 全绿 |
| 失败 | 0 |
| 忽略 | 1 (编译器栈溢出, 非本次修改) |
| Clippy | 干净 |
| import_tests | 12/12 |
| signal_integration | 10/10 |
| nuzo_compiler | 306 (+1 spill) |
| nuzo_bytecode | 88 |
| nuzo_error | 83 |