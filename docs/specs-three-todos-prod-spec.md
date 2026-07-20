# Spec: 三大 TODO 生产级实现

> 范围：TODO A（VmObserver 集成 ErrorCollector）、TODO B（OP_INIT_MODULE lazy import）、TODO C（寄存器 spill 机制）
> 所有 file:line 引用均经 Grep+Read 实测验证（2026-07-12）。

## 1. 总体目标

将 nuzo_lang 中三个长期悬挂的 TODO 推进到生产级，使错误诊断、模块延迟加载、寄存器溢出三条链路端到端贯通，并恢复被 `#[cfg(any())]` / 注释 / `#[ignore]` 屏蔽的回归测试断言。

三个 TODO 共享一个隐含前提：**opcode 层已就绪**。经实测，`InitModule`(slot 30)、`SpillLoad`(slot 54)、`SpillStore`(slot 55) 均已在 [opcode.rs](file:///d:/10/nuzo_lang/crates/nuzo-bytecode/src/opcode.rs) 完整定义（含 operands/size/disasm）。因此工作重心在 **codegen 发射 + VM dispatch + 状态机 + 测试恢复**，而非新增 opcode。

## 2. 层级与硬约束（全篇适用）

| 约束 | 说明 | 来源 |
|------|------|------|
| L5 不能依赖 L6 | `nuzo_error`(L5) 不得依赖 `nuzo_vm`(L6) | project_rules 第八节 |
| nuzo_error 不依赖 nuzo_compiler | 反向依赖禁止 | project_rules 第八节 |
| L1/L2 严格分层 | `nuzo_core`(L1) 不依赖 `nuzo_values`(L2) | project_rules 8.1 |
| VM stack 8MB | 栈溢出阈值 | project_rules 第八节 |
| Arena 用 Vec::remove(idx) | 禁止 swap_remove | project_rules 第八节 |
| 编译错误保留源码位置 | 禁止降级为 C0000 | project_rules 第八节 |
| 不新增 unwrap() | 生产代码用 expect("reason") | checklist |

## 3. TODO A：VmObserver 集成 ErrorCollector

### 3.1 现状（实测）

- `VmObserver` trait 定义于 [vm.rs:113-118](file:///d:/10/nuzo_lang/crates/nuzo-vm/src/vm.rs#L113-L118)，仅 2 方法且都有默认空实现，bound `Send + Sync`：
  - `on_will_execute(&self, _opcode: u8, _ip: usize)`
  - `on_error(&self, _info: &VmErrorInfo)`
- `VmErrorInfo` 定义于 [nuzo_signal/src/types.rs:275-283](file:///d:/10/nuzo_lang/crates/nuzo-signal/src/types.rs#L275-L283)，字段：`error_message: String`、`opcode: Option<u8>`、`ip: usize`、`call_depth: usize`。`nuzo_vm` 通过 `use nuzo_signal::VmErrorInfo`（vm.rs:25）引入。
- `ErrorCollector` 定义于 [collector.rs:231-277](file:///d:/10/nuzo_lang/crates/nuzo-error/src/collector.rs#L231-L277)，位于 `nuzo_error`(L5)。已有方法：
  - `collect_error(&mut self, error: NuzoError, context: ExecutionContext, call_stack: Vec<StackFrameInfo>) -> bool`（collector.rs:454）
  - `collect_nuzo_error(&mut self, error: NuzoError, context: ExecutionContext, call_stack: Vec<StackFrameInfo>, diagnosis: Option<VmDiagnosis>) -> bool`（collector.rs:510）
- 桩测试 [signal_integration.rs:243-262](file:///d:/10/nuzo_lang/tests/integration/signal_integration.rs#L243-L262) 用 `#[cfg(any())]` 永不编译，引用 `nuzo_signal::VmErrorInfo`，调用不存在的 `collector.disconnect_signal()`。
- 现有观察者实现：`NoopVmObserver`(vm.rs:121)、`TestObserver`(vm_tests.rs:2791)、`CountingObserver`。

### 3.2 核心矛盾

| 矛盾 | 细节 |
|------|------|
| 借用冲突 | `VmObserver::on_error` 是 `&self`，`ErrorCollector::collect_error` 是 `&mut self`。ErrorCollector 不能直接 impl VmObserver。 |
| 层级冲突 | ErrorCollector 在 L5，VmObserver 在 L6。nuzo_error 不能依赖 nuzo_vm，故 ErrorCollector 无法 `impl VmObserver`。 |
| 信息缺失 | VmErrorInfo 只有 message/opcode/ip/call_depth，而 collect_error 还需 ExecutionContext + call_stack。 |

### 3.3 设计：反转依赖 + 内部可变性

采用 **Sink trait 反转依赖** 模式（对齐 Anthropic 主从隔离与 Microsoft Connected Agent 数据 handoff 最佳实践）：

1. **在 `nuzo_error`(L5) 定义 `ErrorSink` trait**（新建文件 `crates/nuzo-error/src/sink.rs`）：
   - `trait ErrorSink: Send + Sync { fn sink_error(&self, event: ErrorEvent); }`
   - `ErrorEvent` 结构（定义在 nuzo_error，**不依赖** nuzo_signal）：`message: String`、`opcode: Option<u8>`、`ip: usize`、`call_depth: usize`、`timestamp: u64`。
   - 依赖方向：nuzo_error 不引入任何上层依赖，层级安全。

2. **ErrorCollector 实现 ErrorSink**（无锁队列）：
   - ErrorCollector 内部错误缓冲区用 `crossbeam_queue::SegQueue<DiagnosticError>`（无锁 MPMC 队列）包裹，使 `sink_error(&self)` 能在 `&self` 下无锁入队，消除 Mutex 锁竞争开销（on_error 在热路径）。
   - `sink_error` 将 ErrorEvent 构造为最小 DiagnosticError 后 push 入队；保留现有 `collect_error`/`collect_nuzo_error` 的 `&mut self` 公开 API 不变（向后兼容）。
   - 提供 `drain_sunk() -> Vec<DiagnosticError>` 供测试与宿主取出积压错误（循环 pop 直到队列空）。

3. **在 `nuzo_vm`(L6) 定义 `ErrorSinkObserver` adapter**（新建 `crates/nuzo-vm/src/observer_sink.rs`）：
   - `struct ErrorSinkObserver { sink: Arc<dyn ErrorSink> }`
   - `impl VmObserver for ErrorSinkObserver`：`on_error` 把 `&VmErrorInfo` 转换为 `ErrorEvent` 后调用 `sink.sink_error(event)`；转换逻辑在 nuzo_vm（L6 可依赖 L5 的 ErrorEvent 与 L1 的 VmErrorInfo）。
   - `on_will_execute` 默认空实现（ErrorCollector 不关心每条指令）。

4. **替换桩测试**：移除 signal_integration.rs:245 的 `#[cfg(any())]`，删除 `disconnect_signal()` 调用，改为构造 `ErrorCollector + ErrorSinkObserver`，注入 VM，触发一个运行时错误，断言 `drain_sunk()` 非空且 message/ip 正确。

### 3.4 关键决策

- D-A1：ErrorEvent 不复用 VmErrorInfo，避免 nuzo_error→nuzo_signal 耦合（虽层级允许，但不必要）。
- D-A2：转换在 adapter 层（nuzo_vm），保持 nuzo_error 纯净。
- D-A3：保留 `&mut self` API 向后兼容，新增 `&self` 的 sink 路径并行存在。
- D-A4：内部可变性用 `crossbeam_queue::SegQueue`（无锁 MPMC，非 Mutex/RefCell），满足 `Send + Sync`，热路径零锁竞争。备选：如外部依赖带来维护负担，可回退到 `Mutex`（性能略降但零外部依赖）。

### 3.5 依赖管理

- 在根 `Cargo.toml` 的 `[workspace.dependencies]` 添加 `crossbeam-queue = "0.3"`。
- `nuzo_error` 的 `Cargo.toml` 用 `crossbeam-queue.workspace = true` 继承版本，单点管理。
- **权衡说明**：此为引入外部依赖，违反 project_rules 7.2「优先复用 workspace crates」原则。但用户明确选择 crossbeam 无锁队列以消除 TODO A 热路径的 Mutex 竞争开销。spec 记录此权衡：用外部依赖换取热路径零锁竞争性能。`cargo tree -p nuzo_error` 须验证仅新增 crossbeam-queue（及其子依赖 crossbeam-utils），不引入其他传递依赖污染。

## 4. TODO B：OP_INIT_MODULE（lazy import）

### 4.1 现状（实测）

- **opcode 已定义**：[opcode.rs:487-490](file:///d:/10/nuzo_lang/crates/nuzo-bytecode/src/opcode.rs#L487-L490) `InitModule { module_idx: ConstIdx, init_flag_slot: U16 }`，Opcode slot 30、size 5、operands `[Const, U16]`（opcode.rs:810）。语义注释（opcode.rs:480-486）：检查 `globals[init_flag_slot]`，已初始化则 no-op，否则加载模块+执行顶层+置标志。
- **VM 辅助方法已就绪**：
  - `get_module_chunk(&self, path) -> Result<Arc<Chunk>>`（[dispatch.rs:238](file:///d:/10/nuzo_lang/crates/nuzo-vm/src/dispatch.rs#L238)）
  - `execute_module_toplevel(&mut self, chunk) -> Result<()>`（[dispatch.rs:273](file:///d:/10/nuzo_lang/crates/nuzo-vm/src/dispatch.rs#L273)）：帧切换执行模块顶层，return_address 保存，OP_RETURN 自动恢复。
  - `set_init_flag(&mut self, slot) -> Result<()>`（[dispatch.rs:301-305](file:///d:/10/nuzo_lang/crates/nuzo-vm/src/dispatch.rs#L301-L305)）：在 init_flag_slot 写 true。
  - `module_cache: HashMap<String, Arc<Chunk>>`（[vm.rs:299](file:///d:/10/nuzo_lang/crates/nuzo-vm/src/vm.rs#L299)），`register_module`(vm.rs:574) 注入。
- **codegen 缺口**：[compiler.rs:236-237, 309-310](file:///d:/10/nuzo_lang/crates/nuzo-compiler/src/compiler.rs#L236-L237) 注释 "Imports are resolved during IR building (IrBuilder::resolve_imports)"，codegen 阶段对 `ast::Stmt::Import` 直接跳过，**不发射 InitModule**。
- **4 处 TODO 测试**：[import_tests.rs:276, 302, 346, 395](file:///d:/10/nuzo_lang/tests/integration/import_tests.rs#L276)，期望 "init" 出现一次、恢复 init_count==1、输出顺序 before→lazy_init→99、恢复 lazy_init 断言。

### 4.2 缺口

1. codegen 对 `ast::Stmt::Import` 发射 `InitModule { module_idx, init_flag_slot }`。
2. 编译期为每个 import 分配 `init_flag_slot`（全局变量槽），module_idx 指向常量池中的模块路径字符串。
3. 主 dispatch 表接入 `Opcode::InitModule`：读 module_idx→取路径→查 module_cache→检查 init_flag→未初始化则 execute_module_toplevel + set_init_flag。
4. 模块依赖图（DAG）编译期构建，嵌入常量池（创新点 v1）。
5. 恢复 4 处 TODO 断言。

### 4.3 设计

- **module_idx**：指向常量池 `Value::String`（模块路径），复用 `get_module_path_from_constant`（dispatch.rs:227）。
- **init_flag_slot**：编译期由 `reg_manager` 或全局符号表分配一个全局槽位，初始值 nil/false。每个被 import 的模块独占一个 slot。
- **模块状态机**（载体 = globals[init_flag_slot]）：
  - `Unloaded`（slot 为 falsy）→ 执行 `execute_module_toplevel` → 置 slot=true（`Loaded`）。
  - `Loaded`（slot 为 truthy）→ no-op，直接跳过（保证顶层副作用只执行一次）。
  - `Failed`（模块加载抛错）→ 错误冒泡，slot 保持 falsy，允许重试或由上层捕获。
- **lazy import 语义**：`lazy import` 与普通 `import` 共用 InitModule opcode；区别在 codegen 发射位置——普通 import 在文件顶部立即发射，lazy import 延迟到首次引用处发射。**v1 实现精确 lazy 顺序**（before→lazy_init→99）：IR builder 在解析 `lazy import` 时标记延迟发射点（= 该模块符号首次被引用的 ast 节点），codegen 在该节点前插入 `InitModule` 发射，确保输出顺序与测试期望一致。一次性通过 4 处 import_tests TODO 断言，不留 v1.1 延迟。
- **DAG 依赖图**：编译期 IrBuilder 解析 import 时构建模块依赖 DAG，环检测（循环 import 报 `CircularImport` 错误，保留源码位置）。DAG 节点信息（路径 + 依赖列表）序列化进常量池，供 VM 按需加载。

### 4.4 关键决策

- D-B1：module_idx 复用常量池字符串（ConstIdx），不新增操作数类型。
- D-B2：init_flag_slot 用 U16 全局槽位，复用现有 globals 数组，不新增 VM 数据结构。
- D-B3：状态机载体是 globals 而非独立 ModuleState 表，最小化 VM 改动。
- D-B4：v1 实现延迟发射点（精确 lazy 顺序 before→lazy_init→99），一次性通过 4 处 import_tests TODO 断言（276/302/346/395），不留 v1.1 延迟。

## 5. TODO C：寄存器 spill 机制

### 5.1 现状（实测）

- **opcode 已定义**：[opcode.rs:918](file:///d:/10/nuzo_lang/crates/nuzo-bytecode/src/opcode.rs#L918) `SpillLoad`(slot 54, operands `[Reg, U16]`, size 5)；[opcode.rs:927](file:///d:/10/nuzo_lang/crates/nuzo-bytecode/src/opcode.rs#L927) `SpillStore`(slot 55, operands `[Reg, U16]`, size 5)。
- **运行时特化 opcode**：[opcode.rs:652-654](file:///d:/10/nuzo_lang/crates/nuzo-bytecode/src/opcode.rs#L652-L654) 注释 "LSRA Spill 指令，无对应 Instruction 变体（由编译器后端直接发射 Opcode）"。即 codegen 直接写字节流，不经 Instruction 枚举 encode。
- **codegen 缺口**：[codegen.rs:1677-1695](file:///d:/10/nuzo_lang/crates/nuzo-compiler/src/codegen.rs#L1677-L1695) `has_spills = intervals.iter().any(|iv| iv.reg.is_none())`，检测到 spill 后仅设 `spill_slot_count=0` 并回退，**未插桩**。
- **DualPool**：[reg_pool.rs:83-90](file:///d:/10/nuzo_lang/crates/nuzo-compiler/src/reg_pool.rs#L83-L90)（top 游标 + temp_free LIFO 栈）。关键方法：`acquire_persistent`(127)、`acquire_temp`、`release_temp`(166)、`peak`(197)；超限 `top >= MAX_FUNCTION_LOCALS` 触发 `RegPoolExhausted`(94-104)。
- **错误链路**：`RegPoolExhausted`→`TooManyLocals { count, line, column }`（[error.rs:74-81](file:///d:/10/nuzo_lang/crates/nuzo-compiler/src/error.rs#L74-L81)），用户消息 "line {}] too many local variables: {} (max {})"。
- **patch_jump**：[patch_list.rs:43-97](file:///d:/10/nuzo_lang/crates/nuzo-compiler/src/patch_list.rs#L43-L97) 已实现 IP 回填（解码 opcode→算 instruction_size→写 i16 偏移），spill 偏移修正可复用。
- **VM 占位 handler**：[dispatch_table.rs:716-743](file:///d:/10/nuzo_lang/crates/nuzo-vm/src/dispatch_table.rs#L716-L743) SpillLoad/SpillStore 占位 handler 直接报错 "SpillLoad emitted but LSRA spill not yet implemented"。
- **spill_slot_count**：[call_dispatch.rs:504](file:///d:/10/nuzo_lang/crates/nuzo-vm/src/call_dispatch.rs#L504) 注释 "FunctionPrototype 尚未携带此字段，旧字节码默认 0"。

### 5.2 缺口

1. codegen 在 `has_spills` 时选择 victim（基于使用频率）并发射 SpillStore/SpillLoad 字节。
2. 跳转偏移修正（spill 插桩改变字节码长度，需两遍扫描或偏移重定位表）。
3. VM dispatch 实现 SpillLoad/SpillStore 真实读写（spill_stack 区域）。
4. 栈帧扩展 spill 区域，spill_slot_count 从 codegen 传入 Chunk。
5. 移除 h1_t1/h1_t3 的 `#[ignore]`（若存在），改为 spill 触发测试。
6. 补全 SpillLoad/SpillStore 的 `decode()` 实现（opcode.rs 中当前 disasm 占位/未完整逆向），确保与 encode 可逆、disasm 可正确显示 spill 指令。

### 5.3 设计

- **victim 选择**：遍历 `intervals`，对 `reg.is_none()` 的溢出 interval，按 use_count（使用频率）升序选最早溢出点；在该 interval 的首次 use 之前发射 `SpillStore { src, slot }`，在每次 use 之前发射 `SpillLoad { dst, slot }`。slot 从 0 递增分配。
- **两遍扫描策略**（解决偏移修正难点）：
  - Pass 1：在临时缓冲区发射带 spill 的字节码，记录所有跳转指令的原始 IP 与目标 label。
  - Pass 2：根据 Pass 1 的最终长度，用 patch_jump 机制回填所有跳转偏移（复用 [patch_list.rs:43](file:///d:/10/nuzo_lang/crates/nuzo-compiler/src/patch_list.rs#L43)）。
- **VM spill_stack**：栈帧新增 `spill_stack: Vec<Value>`（长度 = chunk.spill_slot_count）。SpillStore 写 `spill_stack[slot] = R[src]`；SpillLoad 读 `R[dst] = spill_stack[slot]`。栈帧 8MB 预算内分配。
- **spill_slot_count 传递**：codegen 计算 spill slot 总数，写入 `Chunk.spill_slot_count`；VM 创建栈帧时按此值预留 spill_stack。
- **decode 补全**：检查 [opcode.rs](file:///d:/10/nuzo_lang/crates/nuzo-bytecode/src/opcode.rs) 中 SpillLoad/SpillStore 的 `decode()` 实现，确保能从字节流逆向解析出 `{reg, slot}`，disasm 输出 `SpillLoad R{reg}, [{slot}]` / `SpillStore R{reg}, [{slot}]`。v1 必须补全，否则 disasm 工具无法显示 spill 指令，阻碍调试与偏移一致性验证。

### 5.4 关键决策

- D-C1：SpillLoad/SpillStore 是特化 opcode，codegen 直接写字节流（不经 Instruction::encode），对齐 opcode.rs:652 设计。
- D-C2：victim 选择基于 use_count（线性扫描 interval 已有信息），不引入干扰图（v2 才做）。
- D-C3：两遍扫描解决偏移修正，复用 patch_jump，不新造回填机制。
- D-C4：spill_stack 挂栈帧，与 registers 同生命周期，GC 时作为 roots（variable_ops.rs:62 已有 module_cache root 收集模式可参考）。

## 6. 跨 TODO 依赖图

```
TODO A (ErrorSink)            TODO B (InitModule)          TODO C (Spill)
      │                             │                           │
      │ 提供错误冒泡通道             │ 独立                       │ 独立
      ▼                             ▼                           ▼
  [nuzo_error]                  [nuzo_bytecode]            [nuzo_bytecode]
  ErrorSink trait               InitModule opcode          SpillLoad/Store
  (L5, 无上层依赖)              (已就绪)                    (已就绪)
      │                             │                           │
      └───► nuzo_vm adapter ◄───────┼───────────────────────────┘
            (L6 依赖 L5)             │                           │
            ErrorSinkObserver        │ dispatch 接入             │ dispatch 实现
                                    ▼                           ▼
                              [nuzo_vm dispatch]          [nuzo_vm dispatch]
                              InitModule 状态机           SpillLoad/Store 读写
                                    │                           │
                                    ▼                           ▼
                              [nuzo_compiler codegen]     [nuzo_compiler codegen]
                              发射 InitModule              has_spills 插桩
```

- **A 是 B/C 的错误处理基础**：TODO B 的模块加载失败、TODO C 的 spill 越界都应通过 ErrorSink 冒泡到 ErrorCollector。但 A/B/C 可并行开发（A 提供 trait，B/C 先用现有 NuzoError 路径，后续接 ErrorSink）。
- **B 与 C 互不依赖**：分别改动 codegen 不同分支与 dispatch 不同 opcode。
- **共享质量门禁**：三者完成后统一跑 `cargo test --workspace --all-targets` + `just callgraph`。

## 7. 风险预判（每个 TODO Top 3）

### TODO A
1. **外部依赖维护负担**：引入 crossbeam-queue 违反「优先复用 workspace crates」。缓解：D-A4 已记录备选回退 Mutex 方案；`cargo tree` 验证传递依赖仅 crossbeam-utils，无污染；版本锁定 `=0.3` 避免意外升级。
2. **ErrorEvent 信息不足导致诊断降级**：VmErrorInfo 缺 ExecutionContext/call_stack，DiagnosticError 字段缺失。缓解：ErrorEvent 补充可选 context 字段；adapter 尽量从 VM 上下文补全。
3. **向后兼容破坏**：现有 collect_error &mut self 调用方众多。缓解：保留旧 API，sink 是新增并行路径（D-A3）。

### TODO B
1. **init_flag_slot 分配冲突**：全局槽位与现有 globals 冲突。缓解：reg_manager 统一分配，从 globals 末尾预留区间。
2. **循环 import 死锁**：A import B、B import A。缓解：编译期 DAG 环检测，报 CircularImport（保留源码位置）。
3. **lazy 延迟发射点实现复杂度**：v1 需在 IR builder 标记延迟发射点（首次引用 ast 节点）。缓解：参考现有 use_before_def 检测逻辑定位首次引用点；codegen 在该节点前插入 InitModule 发射；加输出顺序断言测试（before→lazy_init→99）锁定行为。

### TODO C
1. **两遍扫描复杂度**：跳转偏移重定位易错。缓解：复用 patch_jump 已验证逻辑；加偏移一致性断言测试。
2. **spill_slot_count 未传递导致 VM 崩溃**：旧字节码 spill_slot_count=0。缓解：版本化 Chunk，旧 chunk 默认 0 兼容；新增 chunk 校验 slot < spill_slot_count。
3. **victim 选择不当导致频繁 spill/reload**：性能退化。缓解：use_count 升序 + 租约保护（enhance_intervals 已有 NudConfig）；v2 引入干扰图。

## 8. 测试策略

### TODO A 回归测试（tests/integration/signal_integration.rs）
- `test_error_collector_vm_observer_integration`：构造 ErrorCollector + ErrorSinkObserver，注入 VM，触发除零错误，断言 drain_sunk() 长度≥1、message 含 "divide"、ip 匹配。
- `test_sink_normal_warning_info_levels`：覆盖 error/warning/info 三级（正常路径）。
- `test_sink_disabled_collector`：ErrorCollector disabled 时 sink 不入队（边界）。
- `test_sink_concurrent_access`：多线程下 crossbeam 无锁队列安全（错误条件/并发）。
- 替换原 `error_collector_signal_driven_mode` 桩（signal_integration.rs:247）。

### TODO B 回归测试（tests/integration/import_tests.rs）
- 恢复 [import_tests.rs:276](file:///d:/10/nuzo_lang/tests/integration/import_tests.rs#L276)：`test_top_level_executes_once` 断言 "init" 恰好出现一次。
- 恢复 import_tests.rs:302：`init_count == 1` 断言取消注释。
- 恢复 import_tests.rs:346：输出顺序 "before"→"lazy_init"→"99"。
- 恢复 import_tests.rs:395：`lazy_init` 出现断言。
- `test_init_module_loads_once`：多次调用 foo()，init 只出现一次（正常）。
- `test_init_module_circular_import`：循环 import 报 CircularImport（错误条件）。
- `test_init_module_output_order`：lazy 顺序断言（边界）。

### TODO C 回归测试（crates/nuzo-compiler/ 或 tests/integration/）
- `test_register_spill_on_overflow`：构造 >MAX_FUNCTION_LOCALS 局部变量，触发 spill，断言无 panic（正常）。
- `test_spill_load_restores_value`：spill 后 SpillLoad 恢复值正确（正常）。
- `test_spill_slot_count_propagated`：Chunk.spill_slot_count > 0 且 VM spill_stack 长度匹配（边界）。
- `test_spill_jump_offset_correct`：spill 插桩后跳转目标正确（边界，复用 patch_jump 测试模式）。
- `test_spill_disabled_when_no_overflow`：无 spill 时 spill_slot_count=0（正常）。
