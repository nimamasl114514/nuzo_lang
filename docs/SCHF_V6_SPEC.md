# SCHF v6 — Contiguous Frame Stack with Bump Pointer

> **目标**：用单一连续 `Vec<Value>` + bump pointer 替代 `VecDeque<CallFrame>`，
> 将 push/pop 从「分配 + 构造 + push_back」降为「usize 加法 + 写 16B FrameInfo」。
> CIP（已落地）压缩 `n_cip`，v6 消除 per-call Vec 分配——合起来给出**快 + 小**的帧栈。

---

## 1. 现状与问题

### 1.1 当前帧栈架构

```
VecDeque<CallFrame>
  ├── CallFrame {                        // 192+ bytes/帧
  │     ip, base, return_address,        //   控制字段
  │     closure: Option<Arc<HeapObject>>, //   Arc clone 开销
  │     caller_chunk: Option<Arc<Chunk>>, //   Arc clone 开销
  │     tco_history: Vec<TcoRecord>,      //   Vec 分配
  │     spill_stack: Vec<Value>,          //   vec![NIL; n] 堆分配 ← 主开销
  │     call_site, arena, kind, ...
  │   }
  └── push_back / pop_back
```

### 1.2 per-call 热路径开销分解

| 操作 | 估算开销 | v6 是否优化 |
|------|---------|------------|
| `spill_stack = vec![NIL; n]` | ~150-300ns（小 n，jemalloc cached） | **是**（改为 frame_data 切片） |
| CallFrame 结构体字段赋值 | ~30ns | **是**（改为 16B FrameInfo） |
| `VecDeque::push_back` | ~10ns（amortized） | **是**（改为 ring 索引） |
| `closure` Arc clone | ~5ns | 否 |
| `caller_chunk` Arc clone | ~5ns | 否 |
| `region.begin_frame()` | ~10ns | 否 |
| `call_site` 查找 | ~20ns | 否 |
| `frame_pager` 检查 | ~5ns | 否 |
| **总计** | **~235-385ns** | v6 可消除 ~190-340ns |

### 1.3 前三次失败教训

| 方案 | 死穴 | v6 如何规避 |
|------|------|------------|
| 帧池 (v1) | `reset_for_reuse` 30+ 行，编译器拒绝内联 push_frame | push = `top += n`，3 行可内联 |
| FrameStack (v2) | `push_reuse(&mut self) -> &mut CallFrame` 别名屏障 | 无 `&mut` 返回值，仅 usize 运算 |
| 通用 | 间接寻址链破坏 CPU 预取 | 单一 `Vec<Value>` 连续内存 + bump pointer |

**核心原则**：帧不是对象，是大数组的一个切片 `[base..base+n]`。

---

## 2. 数据结构

### 2.1 FrameInfo — 16B 环形槽

```rust
/// 帧元信息，仅控制字段。数据（locals + spill）在 frame_data 中。
/// 16 字节 = 2×u64，对齐一个缓存行的 1/4。
#[repr(C)]
#[derive(Clone, Copy)]
struct FrameInfo {
    return_address: usize,  // 8B — pop 时恢复 IP
    base: usize,            // 8B — frame_data 中的起始偏移
    // n_cip 不需要存储：pop 时用 base 回退 top 即可
}
```

**为什么 16B 够用**：
- `return_address` — pop 时恢复 IP
- `base` — pop 时 `top = base` 回退 bump pointer；也是 spill_stack 的起始偏移
- `closure` / `caller_chunk` / `call_site` / `arena` → 移到辅助结构（见 2.4）
- `n_cip` — 不需要存储，pop 时 `top = base` 即可回退

### 2.2 FrameRing — 固定 64 槽环形缓冲

```rust
/// 固定大小环形帧信息缓冲区。
/// head 指向下一个可写位置，(head - 1) & 63 是当前帧。
struct FrameRing {
    slots: [FrameInfo; 64],
    head: u8,  // 0..64，溢出时降级到 overflow_stack
}
```

**操作**：
```rust
// push — 3 条指令
ring.slots[ring.head as usize] = FrameInfo { return_address, base };
ring.head = (ring.head + 1) & 63;

// pop — 2 条指令  
ring.head = (ring.head - 1) & 63;
let info = ring.slots[ring.head as usize];
```

### 2.3 FrameData — 连续值栈

```rust
/// 单一连续值栈，帧 = data[base..base+n_cip]。
/// 预分配 1M slots（16MB），永不扩容（够 1000+ 层深度 × 平均 1000 locals）。
/// 实际按 max_stack_size 限制。
struct FrameData {
    data: Vec<Value>,  // 预分配，capacity = max_stack_size
    top: usize,        // bump pointer，指向下一个空闲 slot
}
```

### 2.4 FrameMeta — 辅助帧元数据

```rust
/// 非 hot-path 帧字段，存在 Vec 中按 frame_index 索引。
/// 仅在需要时访问（closure 调用、arena 管理、错误诊断）。
struct FrameMeta {
    closure: Option<Arc<HeapObject>>,
    caller_chunk: Option<Arc<Chunk>>,
    caller_func_reg: usize,
    arena: usize,
    call_site: Option<SourceLocation>,
    kind: FrameKind,
}
```

**为什么分离**：FrameInfo 的 16B 必须在 ring 中保持紧凑（缓存行友好）。
FrameMeta 只在冷路径访问（chunk 切换、arena 逃逸检测、错误诊断），不放入 ring。

### 2.5 OverflowStack — 环溢出降级

```rust
/// 当 ring head 回绕到已有帧时（深度 > 64），降级到 Vec<FrameInfo> + Vec<FrameMeta>。
/// 极罕见（64+ 层递归），O(n) 可接受。
struct OverflowStack {
    infos: Vec<FrameInfo>,
    metas: Vec<FrameMeta>,
}
```

### 2.6 完整 ExecutionContext 变更

```rust
pub struct ExecutionContext {
    // ===== 热字段 =====
    pub registers: ElasticRegisterFile,
    pub register_write_ptr: usize,
    
    // ===== SCHF v6 帧栈 =====
    pub frame_data: FrameData,       // 连续值栈
    pub frame_ring: FrameRing,       // 16B×64 环形帧信息
    pub frame_metas: Vec<FrameMeta>, // 辅助元数据（cold path）
    pub frame_overflow: OverflowStack, // ring 溢出降级
    
    // ===== 保留字段 =====
    pub running: bool,
    pub global_scope: GlobalScope,
    // ... 其余不变
}
```

**移除**：`frames: VecDeque<CallFrame>`、`frame_pool: Vec<CallFrame>`（已删除）。

---

## 3. CIP 集成

### 3.1 已落地部分

CIP（区间图着色）已在 `allocator.rs` 中实现：
- `allocate_spill_slot(start, end)` 使用左边缘贪心着色复用槽位
- 不重叠生命周期的被 spill 变量共享同一个槽位
- `spill_slot_count()` 返回压缩后的槽数

### 3.2 v6 侧适配

v6 的 `frame_data[base..base+n_cip]` 切片同时容纳 locals 和 spill slots：
- `data[base..base+locals_count]` — 函数局部变量
- `data[base+locals_count..base+locals_count+spill_slot_count]` — spill 槽

`n_cip = locals_count + spill_slot_count`，CIP 已压缩 `spill_slot_count`。

**spill 读写适配**（当前在 `dispatch_table.rs`）：
```rust
// 当前（VecDeque<CallFrame>）：
vm.cx.frames.back().and_then(|f| f.spill_stack.get(slot)).copied()
// v6：
vm.cx.frame_data.data[base + locals_count + slot]
```

需要每个 chunk 记录 `locals_count` 以计算 spill 区起始偏移。
当前 chunk 已有 `locals_count` 字段，无需额外存储。

---

## 4. API 变更

### 4.1 push_frame — 热路径

```rust
// 当前签名（保持不变）：
pub fn push_frame(&mut self, closure: Option<Arc<HeapObject>>, argc: usize) 
    -> Result<(), NuzoError>

// v6 实现：
pub fn push_frame(&mut self, closure: Option<Arc<HeapObject>>, argc: usize) 
    -> Result<(), NuzoError> 
{
    // 1. FramePager 检查（保留）
    let frame_idx = self.frame_depth();
    if self.frame_pager.should_spill(frame_idx) {
        self.frame_pager.spill_frames(&mut self.cx);  // API 变更
    }
    
    // 2. 计算 base 和 n_cip
    let return_address = self.ip;
    let base = self.cx.frame_data.top;
    let n_cip = self.current_chunk_spill_count();  // 从当前 chunk 读取
    
    // 3. 检查栈溢出（保留）
    let needed = base + n_cip;
    if needed > self.max_stack_size {
        return Err(err_stack_overflow(needed, self.max_stack_size, false));
    }
    
    // 4. caller_chunk + call_site（保留，移入 FrameMeta）
    let caller_chunk = self.chunk.clone();
    let call_site = /* 同当前 */;
    let arena = self.cx.region.begin_frame();
    
    // 5. 写 FrameInfo（热路径核心，3 行）
    self.cx.frame_ring.push(FrameInfo { return_address, base });
    
    // 6. 写 FrameMeta（冷数据）
    self.cx.frame_metas.push(FrameMeta {
        closure, caller_chunk, caller_func_reg: 0, arena, call_site,
        kind: FrameKind::Normal,
    });
    
    // 7. bump pointer 前进 + zero-fill
    self.cx.frame_data.fill_nil(base, n_cip);  // data[base..base+n_cip] = NIL
    self.cx.frame_data.top = base + n_cip;
    
    self.current_base = base;
    self.frame_pager.record_push();
    Ok(())
}
```

### 4.2 push_frame_with_base — 热路径

同 `push_frame`，但 base 和 return_address 由调用方提供（用于 execute_normal_call / execute_closure_fast）。

### 4.3 pop_frame — 热路径

```rust
pub fn pop_frame(&mut self) -> Result<(), NuzoError> {
    self.frame_pager.record_pop();
    
    // 1. 从 ring 弹出 FrameInfo（2 行）
    let info = match self.cx.frame_ring.pop() {
        Some(i) => i,
        None => return Err(/* StackUnderflow */),
    };
    
    // 2. 从 metas 弹出 FrameMeta
    let meta = self.cx.frame_metas.pop().unwrap();
    
    // 3. 恢复 IP
    self.ip = info.return_address;
    
    // 4. 恢复 caller chunk（保留）
    if let Some(ref chunk) = meta.caller_chunk {
        self.chunk_ptr = Arc::as_ptr(chunk);
        self.chunk = Some(chunk.clone());
        self.invalidate_cigc_cache();
    }
    
    // 5. 回退 bump pointer
    self.cx.registers.truncate(info.base);
    self.cx.frame_data.top = info.base;
    
    // 6. Arena 逃逸检测（保留，参数改为 meta.arena, info.base）
    let has_arena_escape = self.check_arena_escape(meta.arena, info.base);
    if self.cx.region.end_frame(meta.arena, has_arena_escape).is_some() {
        self.promote_arena_range(meta.arena)?;
    }
    
    // 7. 更新 current_base
    self.current_base = self.cx.frame_ring.back().map(|f| f.base).unwrap_or(0);
    
    // 8. FramePager restore 检查（保留）
    if self.frame_pager.front_is_trampoline(&self.cx) {
        self.frame_pager.restore_frames(&mut self.cx);
    }
    
    Ok(())
}
```

### 4.4 帧访问 API

```rust
// 当前帧的 spill slot 读取（dispatch_table.rs 热路径）
#[inline(always)]
fn spill_get(&self, slot: u16) -> Value {
    let base = self.current_base;
    let locals = self.chunk_locals_count();  // 当前 chunk 的 locals_count
    self.cx.frame_data.data[base + locals as usize + slot as usize]
}

#[inline(always)]
fn spill_set(&mut self, slot: u16, val: Value) {
    let base = self.current_base;
    let locals = self.chunk_locals_count();
    self.cx.frame_data.data[base + locals as usize + slot as usize] = val;
}
```

---

## 5. FramePager 适配

### 5.1 当前 FramePager 接口

FramePager 操作 `&mut VecDeque<CallFrame>`：
- `should_spill(frames_len)` — 检查深度
- `spill_frames(frames)` — 从底部 drain + 插入 trampoline
- `restore_frames(frames)` — 从堆恢复 + 移除 trampoline
- `front_is_trampoline(frames)` — 检查底部是否桩帧

### 5.2 v6 适配

FramePager 改为操作 `&mut ExecutionContext`：

```rust
// should_spill — 不变，基于 frame_depth()
// spill_frames — 从 frame_metas 底部 drain，在 ring 中插入 trampoline FrameInfo
// restore_frames — 从堆恢复 frame_metas，移除 trampoline
// front_is_trampoline — 检查 frame_metas[0].kind == Trampoline
```

**Trampoline 帧**：FrameInfo { return_address: 0, base: 0 }，FrameMeta { kind: Trampoline, .. }。

### 5.3 ring 溢出与 FramePager 的交互

当 ring head 回绕（深度 > 64）时：
1. 将 ring 中所有 FrameInfo + frame_metas 中对应条目转移到 `overflow_stack`
2. ring 清空，重新开始
3. FramePager 的 `should_spill` 仍然基于总深度（ring + overflow）

这是一个极罕见的边界情况（64+ 层递归），可以先用简单实现（直接降级到 Vec），后续优化。

---

## 6. Arena 适配

### 6.1 当前 Arena 接口

- `region.begin_frame()` — 返回 arena index
- `region.end_frame(arena, has_escape)` — 结束帧，可能返回需要 promote 的对象
- `check_arena_escape(frame)` — 遍历 frame.base 以下的寄存器检查逃逸

### 6.2 v6 适配

`check_arena_escape` 签名从 `(&self, frame: &CallFrame)` 改为 `(&self, arena: usize, base: usize)`（此变更已在 SCHF v2 尝试中完成，代码中已存在）。

其余 Arena 接口不变——arena 与帧栈是独立的。

---

## 7. TCO 适配

### 7.1 当前 TCO 实现

TCO（尾调用优化）复用当前帧：
- `tco_reused: bool` — 标记帧已被 TCO 复用
- `tco_history: Vec<TcoRecord>` — 记录被替换的闭包信息

### 7.2 v6 适配

- `tco_reused` → `FrameMeta.tco_reused`
- `tco_history` → `FrameMeta.tco_history: Vec<TcoRecord>`（保留 Vec，TCO 是冷路径）

TCO 复用逻辑：
1. 不 pop（bump pointer 不回退）
2. 用新函数的 n_cip 重新 zero-fill `[base..base+new_n_cip]`
3. 更新 FrameMeta 中的 closure、caller_chunk 等
4. 如果 `new_n_cip > old_n_cip`，检查栈溢出并前进 bump pointer

---

## 8. 错误诊断适配

### 8.1 build_call_stack

当前遍历 `frames.iter()`，v6 改为遍历 `frame_metas` + `frame_ring`：

```rust
fn build_call_stack(&self, _error_ip: usize) -> Vec<StackFrameInfo> {
    self.cx.frame_metas.iter().enumerate().map(|(i, meta)| {
        // 同当前逻辑，从 meta.closure 提取函数名等
    }).collect()
}
```

### 8.2 build_call_stack_for_debug

同上，遍历 `frame_metas`。

---

## 9. 迁移步骤

### Phase 1: 数据结构搭建（不改行为）

1. 在 `vm.rs` 中定义 `FrameInfo`、`FrameRing`、`FrameData`、`FrameMeta`、`OverflowStack`
2. 在 `ExecutionContext` 中添加新字段（与 `frames: VecDeque<CallFrame>` 并存）
3. 在 `reset_and_load_chunk` 中初始化新字段
4. 在 `reset_registers_and_frames` 中重置新字段

**验证点**：编译通过，所有测试不变。

### Phase 2: push/pop 双写（影子模式）

1. 修改 `push_frame` / `push_frame_with_base`：同时写入 VecDeque 和新结构
2. 修改 `pop_frame`：同时从两处弹出
3. 添加 debug_assert 验证一致性

**验证点**：所有测试通过，debug_assert 不触发。

### Phase 3: 切换读取路径

1. 将所有 `frames.back()` / `frames.back_mut()` 读取改为从新结构读取
2. 将 `frames.len()` 改为 `frame_depth()`
3. 将 `frames.iter()` 改为 `frame_metas.iter()`
4. 将 `spill_stack` 读写改为 `frame_data` 切片访问

**验证点**：所有测试通过。

### Phase 4: 移除 VecDeque

1. 删除 `frames: VecDeque<CallFrame>` 字段
2. 删除 `CallFrame::new` / `with_closure` / `trampoline` 中的 Vec 分配逻辑
3. 清理 dead code

**验证点**：编译通过，所有测试通过，性能不退化。

### Phase 5: FramePager 适配

1. 修改 `spill_frames` / `restore_frames` / `front_is_trampoline` 操作新结构
2. 修改 ring 溢出降级逻辑

**验证点**：深度递归测试通过（`test_nud_deep_recursion_frame_paging`）。

### Phase 6: 性能验证

1. 运行 `bench_vm_branch_predict`，对比 Phase 4 前后
2. 运行 SCSB 正确性测试
3. 运行 Nuzo vs Python 对比
4. **关键验收标准**：所有场景不退化，VM-D（函数调用）改善 ≥1.3x

---

## 10. 风险与缓解

| 风险 | 概率 | 缓解 |
|------|------|------|
| ring 间接寻址引入新别名屏障 | 中 | push/pop 用 `#[inline(always)]`；FrameInfo 是 Copy 类型，无 &mut 返回 |
| FramePager drain/restore 在新结构上 O(n) 过慢 | 低 | 64+ 层递归极罕见；可后续优化为 ring 批量转移 |
| spill 读写需要额外加法（base + locals + slot） | 中 | 编译器应能 CSE 这个计算；locals_count 在 chunk 切换时缓存 |
| Arena 逃逸检测行为变化 | 低 | check_arena_escape 签名已适配，逻辑不变 |
| TCO 复用时 n_cip 变化导致 bump pointer 不一致 | 中 | TCO 路径显式检查 new_n_cip vs old_n_cip，必要时调整 top |

---

## 11. 验收标准

### 11.1 正确性

- [ ] 526+ VM 测试通过（允许 1 个预存失败）
- [ ] 8/8 SCSB 测试通过
- [ ] `test_nud_deep_recursion_frame_paging` 通过
- [ ] TCO 相关测试通过
- [ ] 错误诊断（build_call_stack）输出正确

### 11.2 性能

- [ ] VM-A（算术循环）：不退化（±5%）
- [ ] VM-B（字符串拼接）：不退化（±5%）
- [ ] **VM-D（函数调用）：改善 ≥1.3x**（median 对比基线）
- [ ] VM-E（累加）：不退化（±5%）
- [ ] VM-F（嵌套循环）：不退化（±5%）
- [ ] VM-G（条件分支）：不退化（±5%）
- [ ] VM-H（局部变量）：不退化（±5%）

### 11.3 代码质量

- [ ] push_frame / push_frame_with_base / pop_frame 主体 < 20 行
- [ ] 无 `&mut self -> &mut T` 的别名模式
- [ ] FrameRing 操作全部 `#[inline(always)]`
- [ ] 无新增 unsafe（除现有 unsafe 外）

---

## 12. 不做什么

- **不内联 locals 到 FrameInfo**：locals 在 frame_data 中，FrameInfo 只存 base + return_address
- **不改变 Arena 系统**：Arena 与帧栈独立，保持现有接口
- **不改变 FramePager 的触发逻辑**：should_spill / record_push / record_pop 语义不变
- **不改变 TCO 的语义**：TCO 仍然复用帧，只是帧的物理表示变了
- **不改变 spill_stack 的逻辑语义**：spill 仍然是按 slot 索引读写，只是物理位置从 Vec 改为 frame_data 切片
- **不做热区地址重排**：CIP 的热区优化留给后续迭代
