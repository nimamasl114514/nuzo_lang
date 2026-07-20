# Checklist: 三大 TODO 质量门禁

## 编译与测试门禁

- [ ] `cargo check --workspace` 通过
- [ ] `cargo test --workspace --all-targets` 通过
- [ ] 测试结果记录：总 N / 通过 N / 失败 N = ____ / ____ / ____
- [ ] `cargo clippy --workspace -- -D warnings` 无 warning
- [ ] `just callgraph` 通过（CALL_GRAPH.md 一致）

## TODO A 回归测试

- [ ] `test_error_collector_vm_observer_integration` 通过（触发错误→drain_sunk 非空→message/ip 正确）
- [ ] `test_sink_normal_warning_info_levels` 通过（error/warning/info 三级覆盖）
- [ ] `test_sink_disabled_collector` 通过（disabled 时 sink 不入队，边界）
- [ ] `test_sink_concurrent_access` 通过（多线程 crossbeam 无锁队列安全，并发/错误条件）
- [ ] signal_integration.rs:245 的 `#[cfg(any())]` 已移除，桩测试已替换

## TODO B 回归测试

- [ ] **4 处 import_tests TODO 全部恢复断言**（276/302/346/395，v1 延迟发射点保证一次性通过）
- [ ] `test_init_module_loads_once` 通过（多次调用 foo()，"init" 恰好一次）
- [ ] `test_init_module_output_order` 通过（before→lazy_init→99 顺序）
- [ ] `test_init_module_circular_import` 通过（循环 import 报 CircularImport，错误条件）
- [ ] import_tests.rs:276 "init" 一次断言已恢复
- [ ] import_tests.rs:302 init_count==1 断言已恢复
- [ ] import_tests.rs:346 输出顺序断言已恢复
- [ ] import_tests.rs:395 lazy_init 出现断言已恢复

## TODO C 回归测试

- [ ] `test_register_spill_on_overflow` 通过（>MAX_FUNCTION_LOCALS 触发 spill 无 panic）
- [ ] `test_spill_load_restores_value` 通过（spill 后 SpillLoad 恢复值正确）
- [ ] `test_spill_slot_count_propagated` 通过（Chunk.spill_slot_count>0 且 VM spill_stack 长度匹配）
- [ ] `test_spill_jump_offset_correct` 通过（spill 插桩后跳转目标正确）
- [ ] `test_spill_disabled_when_no_overflow` 通过（无 spill 时 spill_slot_count=0）
- [ ] h1_t1/h1_t3 的 `#[ignore]` 已移除（若存在）并改为 spill 触发测试
- [ ] SpillLoad/SpillStore `decode()` 可逆向（disasm 可显示 `SpillLoad R{reg}, [{slot}]` / `SpillStore R{reg}, [{slot}]`，与 encode 可逆）

## 依赖管理

- [ ] `crossbeam-queue = "0.3"` 已添加到根 Cargo.toml `[workspace.dependencies]`
- [ ] `nuzo_error/Cargo.toml` 用 `crossbeam-queue.workspace = true` 继承（未独立声明版本）
- [ ] `cargo tree -p nuzo_error` 验证传递依赖仅 crossbeam-utils，无其他污染
- [ ] 权衡已记录：外部依赖换热路径零锁竞争（违反 7.2 但用户明确选择）

## 硬约束遵守

- [ ] 无新增 `unwrap()` 在生产代码（除非有 `expect("reason")` 理由）
- [ ] L1/L2 分层未破坏（`nuzo_core` 不依赖 `nuzo_values`）
- [ ] `nuzo_error` 不依赖 `nuzo_vm`（`cargo tree -p nuzo_error` 验证）
- [ ] `nuzo_error` 不依赖 `nuzo_compiler`（层级约束）
- [ ] VM stack 保持 8MB 上限
- [ ] Arena 对象用 `Vec::remove(idx)`（非 swap_remove）
- [ ] 编译错误保留源码位置（无降级为 C0000）
- [ ] 新增错误变体（CircularImport 等）保留 line/column

## 设计决策落实

- [ ] TODO A：ErrorSink trait 在 nuzo_error(L5) 定义，ErrorCollector 用 crossbeam-queue 无锁队列（SegQueue）
- [ ] TODO A：ErrorSinkObserver adapter 在 nuzo_vm(L6)，VmErrorInfo→ErrorEvent 转换在 adapter
- [ ] TODO A：collect_error/collect_nuzo_error 旧 `&mut self` API 保留（向后兼容）
- [ ] TODO B：InitModule 复用常量池字符串（ConstIdx）+ globals[init_flag_slot] 状态机
- [ ] TODO B：编译期 DAG 环检测，CircularImport 保留源码位置
- [ ] TODO B：v1 实现延迟发射点（精确 lazy 顺序 before→lazy_init→99，不留 v1.1）
- [ ] TODO C：SpillLoad/SpillStore 直接写 Opcode 字节（不经 Instruction，对齐 opcode.rs:652）
- [ ] TODO C：SpillLoad/SpillStore decode() 已补全（disasm 可逆向显示）
- [ ] TODO C：victim 选择基于 use_count 升序
- [ ] TODO C：跳转偏移修正复用 patch_jump（patch_list.rs:43）
- [ ] TODO C：spill_stack 挂栈帧并纳入 GC roots

## 收尾

- [ ] CALL_GRAPH.md 已更新（新增 ErrorSink/ErrorSinkObserver/InitModule dispatch/SpillLoad/SpillStore dispatch）
- [ ] 无遗留 `#[cfg(any())]` 桩测试
- [ ] 无遗留注释掉的断言（import_tests.rs 4 处已恢复）
- [ ] project_memory.md Bug Archive 已归档（HLLM 机制）
