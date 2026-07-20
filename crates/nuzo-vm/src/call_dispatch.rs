//! # 调用派发 — 函数调用链路
//!
//! 负责所有函数调用的统一派发：Nuzo 闭包、native 函数、builtin 函数。
//! 处理参数传递、返回值、帧创建等调用约定细节。

use std::sync::Arc;

use nuzo_bytecode::{Chunk, Opcode};
use nuzo_core::Value;
use nuzo_values::{HeapObject, InternalError, NIL, NuzoError};

use super::{FrameKind, InlineCacheEntry, VM};

impl VM {
    // ========================================================================
    // Frame Management (Zero-Copy Argument Reuse)
    // ========================================================================

    // ----------------------------------------------------------------
    // SCHF v6 Phase 3：帧访问辅助方法（spec 4.4）
    // ----------------------------------------------------------------

    /// 当前帧栈深度（spec 4.4 `frame_depth()`）。
    ///
    /// 等价于旧 `frames.len()`。`frame_metas` 与 `frames` 在 push/pop/spill/restore
    /// 全程同步（spill 时 drain + insert trampoline，restore 时移除 trampoline），
    /// 故 `frame_metas.len() == frames.len()` 始终成立。
    #[inline(always)]
    pub(super) fn frame_depth(&self) -> usize {
        self.cx.frame_metas.len()
    }

    /// 返回当前栈顶 `FrameInfo` 的不可变引用（spec 4.4）。
    ///
    /// 查询顺序：优先 `frame_overflow`（>64 层递归降级路径），再 `frame_ring`。
    /// spill 后 ring/overflow 均被清空（Phase 2 设计），返回 `None`；
    /// 调用方应在此时触发 `restore_frames` 恢复 spilled block 后再读取。
    #[inline]
    pub(super) fn current_v6_info(&self) -> Option<&super::frame_v6::FrameInfo> {
        if !self.cx.frame_overflow.infos.is_empty() {
            self.cx.frame_overflow.infos.last()
        } else {
            self.cx.frame_ring.back()
        }
    }

    /// 返回当前栈顶 `FrameMeta` 的不可变引用（spec 4.4 + 2.4）。
    ///
    /// `frame_metas` 在 spill 后仍保留剩余帧的 meta + 1 个 trampoline meta，
    /// 故 `frame_metas.last()` 始终对应当前栈顶帧。
    #[inline(always)]
    pub(super) fn current_meta(&self) -> Option<&super::frame_v6::FrameMeta> {
        self.cx.frame_metas.last()
    }

    /// 返回当前栈顶 `FrameMeta` 的可变引用（spec 7.2 TCO 复用写入）。
    #[inline(always)]
    pub(super) fn current_meta_mut(&mut self) -> Option<&mut super::frame_v6::FrameMeta> {
        self.cx.frame_metas.last_mut()
    }

    /// 当前 chunk 的 `locals_count`（spec 4.4 spill 区起点计算）。
    ///
    /// 直接从 `self.chunk` 读取，与 push_frame 影子写入时使用的字段一致。
    /// chunk 为 None 时返回 0（VM 未加载 chunk 的边界场景）。
    #[inline(always)]
    fn chunk_locals_count(&self) -> usize {
        self.chunk.as_ref().map(|c| c.locals_count as usize).unwrap_or(0)
    }

    /// SpillLoad 热路径（spec 4.4）：从 `frame_data` 切片读取 spill 槽。
    ///
    /// 等价于旧 `frames.back().spill_stack.get(slot).copied().unwrap_or(NIL)`。
    /// 越界返回 `NIL`（与旧 SpillLoad 行为一致），不自动扩容（spill 槽由 CIP 预分配）。
    #[inline(always)]
    pub(super) fn spill_get(&self, slot: u16) -> Value {
        let base = self.current_base;
        let locals = self.chunk_locals_count();
        let idx = base + locals + slot as usize;
        self.cx.frame_data.data.get(idx).copied().unwrap_or(NIL)
    }

    /// SpillStore 热路径（spec 4.4）：写入 `frame_data` 切片 spill 槽。
    ///
    /// 等价于旧 `frames.back_mut().spill_stack[slot] = val`（含自动扩容）。
    /// 本实现移除自动扩容：spill 槽数量由 CIP 编译期计算，运行期不会越界；
    /// 若越界（编译器 bug）以 debug_assert 触发，release 模式静默丢弃写入。
    #[inline(always)]
    pub(super) fn spill_set(&mut self, slot: u16, val: Value) {
        let base = self.current_base;
        let locals = self.chunk_locals_count();
        let idx = base + locals + slot as usize;
        if let Some(slot_ref) = self.cx.frame_data.data.get_mut(idx) {
            *slot_ref = val;
        } else {
            debug_assert!(
                false,
                "spill_set: idx {} out of bounds (data.len={})",
                idx,
                self.cx.frame_data.data.len()
            );
        }
    }

    pub fn push_frame(
        &mut self,
        closure: Option<Arc<HeapObject>>,
        argc: usize,
    ) -> Result<(), NuzoError> {
        // SCHF v6 Phase 4：FramePager 直接操作 ExecutionContext（spec 5.2）
        if self.frame_pager.should_spill(self.frame_depth()) {
            self.frame_pager.spill_frames(&mut self.cx);
        }
        let return_address = self.ip;
        let base = self.cx.registers.len().saturating_sub(argc);
        let caller_chunk = self.chunk.clone();
        let call_site = caller_chunk.as_ref().and_then(|chunk| {
            let call_instr_size = Opcode::Call.instruction_size();
            chunk.get_source_location(return_address.saturating_sub(call_instr_size))
        });
        let arena = self.cx.region.begin_frame();

        // === SCHF v6 Phase 4：仅写入 v6 结构（VecDeque 已移除） ===
        let v6_info = super::frame_v6::FrameInfo { return_address, base };
        let v6_meta = super::frame_v6::FrameMeta {
            closure,
            caller_chunk,
            caller_func_reg: 0,
            arena,
            call_site,
            kind: FrameKind::Normal,
            tco_reused: false,
            tco_history: Vec::new(),
        };

        self.current_base = base;
        self.frame_pager.record_push();

        // 写入 FrameInfo（ring 或 overflow 降级）
        // 保守条件：ring_depth >= 63 或 overflow 非空时降级
        // （FrameRing head==0 表示空，深度达 64 会让 head 回绕到 0 误判为空）
        let ring_depth =
            self.cx.frame_metas.len().saturating_sub(self.cx.frame_overflow.infos.len());
        if ring_depth >= 63 || !self.cx.frame_overflow.infos.is_empty() {
            self.cx.frame_overflow.infos.push(v6_info);
        } else {
            self.cx.frame_ring.push(v6_info);
        }
        // 写入 FrameMeta（统一 Vec，包含 ring + overflow 的 meta）
        self.cx.frame_metas.push(v6_meta);
        // bump pointer 前进 + zero-fill（n_cip = locals_count + spill_slot_count）
        let n_cip = self
            .chunk
            .as_ref()
            .map(|c| c.locals_count as usize + c.spill_slot_count as usize)
            .unwrap_or(0);
        let needed = base.saturating_add(n_cip);
        if self.cx.frame_data.data.len() < needed {
            self.cx.frame_data.data.resize(needed, NIL);
        }
        self.cx.frame_data.fill_nil(base, n_cip);
        self.cx.frame_data.top = base + n_cip;

        Ok(())
    }

    pub fn push_frame_with_base(
        &mut self,
        return_address: usize,
        base: usize,
        closure: Option<Arc<HeapObject>>,
        caller_func_reg: usize,
        caller_chunk: Option<Arc<Chunk>>,
    ) -> Result<(), NuzoError> {
        // SCHF v6 Phase 4：FramePager 直接操作 ExecutionContext（spec 5.2）
        if self.frame_pager.should_spill(self.frame_depth()) {
            self.frame_pager.spill_frames(&mut self.cx);
        }
        let call_site = caller_chunk.as_ref().and_then(|chunk| {
            let call_instr_size = Opcode::Call.instruction_size();
            chunk.get_source_location(return_address.saturating_sub(call_instr_size))
        });
        let arena = self.cx.region.begin_frame();

        // === SCHF v6 Phase 4：仅写入 v6 结构（VecDeque 已移除） ===
        let v6_info = super::frame_v6::FrameInfo { return_address, base };
        let v6_meta = super::frame_v6::FrameMeta {
            closure,
            caller_chunk,
            caller_func_reg,
            arena,
            call_site,
            kind: FrameKind::Normal,
            tco_reused: false,
            tco_history: Vec::new(),
        };

        self.current_base = base;
        self.frame_pager.record_push();

        // 写入 FrameInfo（ring 或 overflow 降级）
        let ring_depth =
            self.cx.frame_metas.len().saturating_sub(self.cx.frame_overflow.infos.len());
        if ring_depth >= 63 || !self.cx.frame_overflow.infos.is_empty() {
            self.cx.frame_overflow.infos.push(v6_info);
        } else {
            self.cx.frame_ring.push(v6_info);
        }
        // 写入 FrameMeta
        self.cx.frame_metas.push(v6_meta);
        // bump pointer 前进 + zero-fill（n_cip = locals_count + spill_slot_count）
        let n_cip = self
            .chunk
            .as_ref()
            .map(|c| c.locals_count as usize + c.spill_slot_count as usize)
            .unwrap_or(0);
        let needed = base.saturating_add(n_cip);
        if self.cx.frame_data.data.len() < needed {
            self.cx.frame_data.data.resize(needed, NIL);
        }
        self.cx.frame_data.fill_nil(base, n_cip);
        self.cx.frame_data.top = base + n_cip;

        Ok(())
    }

    pub fn pop_frame(&mut self) -> Result<(), NuzoError> {
        self.frame_pager.record_pop();

        // === SCHF v6 Phase 4：从 v6 结构弹出（VecDeque 已移除） ===
        // Pop v6_meta（栈顶，含 caller_chunk / arena / closure 等冷路径字段）
        let meta = self.cx.frame_metas.pop().ok_or_else(|| {
            NuzoError::internal(
                InternalError::StackUnderflow { operation: "pop_frame".into() },
                None,
            )
        })?;
        // Pop v6_info（overflow 优先，再 ring；含 return_address / base 热路径字段）
        let info = if !self.cx.frame_overflow.infos.is_empty() {
            self.cx.frame_overflow.infos.pop()
        } else {
            self.cx.frame_ring.pop()
        }
        .ok_or_else(|| {
            NuzoError::internal(
                InternalError::StackUnderflow { operation: "pop_frame (v6_info)".into() },
                None,
            )
        })?;

        // 桩帧不应被 pop（front_is_trampoline 检查应在 pop 前触发 restore）
        debug_assert_ne!(
            meta.kind,
            FrameKind::Trampoline,
            "pop_frame: trampoline meta should have been restored before popping"
        );

        // 恢复 IP 与 caller_chunk
        self.ip = info.return_address;
        if let Some(ref chunk) = meta.caller_chunk {
            self.chunk_ptr = Arc::as_ptr(chunk);
            self.chunk = Some(chunk.clone());
            self.invalidate_cigc_cache();
        }
        // 截断寄存器与 frame_data 到被弹帧的 base
        self.cx.registers.truncate(info.base);
        self.cx.frame_data.top = info.base;
        // Arena 逃逸检测与提升
        // S4 修复：promote_arena_range 现在需要 caller_base（= info.base），
        // 以便重写 caller 寄存器 [0..base) 中持有的旧 arena 索引。
        //
        // 关键：必须在 end_frame 之前调用 promote_arena_range。
        // 原顺序（end_frame → promote）有 bug：end_frame 会 truncate frame_stack，
        // 销毁 ArenaFrameState（obj_start/obj_count），导致 promote_arena_range 内
        // frame_objects()/frame_state() 返回 None → 提前返回 → 提升从未发生 →
        // caller 寄存器仍持旧 arena 索引 → 悬垂指针 → UAF。
        // 正确顺序：check_arena_escape（读 frame_state）→ promote（读 frame_state，
        // take_arena_object 取走对象）→ end_frame（truncate frame_stack，不再触碰
        // objects Vec，因为已被 take_arena_object 取走）。
        let has_arena_escape = self.check_arena_escape(meta.arena, info.base);
        if has_arena_escape {
            self.promote_arena_range(meta.arena, info.base)?;
        }
        self.cx.region.end_frame(meta.arena, has_arena_escape);
        self.cx.register_write_ptr = self.cx.registers.len();
        // SCHF v6 Phase 4：current_base 改读 v6_info（无 fallback，v6 是唯一数据源）。
        // spill 后 ring/overflow 被清空，current_v6_info() 返回 None → unwrap_or(0)。
        // 此时 front_is_trampoline 会触发 restore，恢复后 current_base 在下次 pop 时读取。
        self.current_base = self.current_v6_info().map(|i| i.base).unwrap_or(0);
        // 检查是否到达桩帧（spill 时插入在 frame_metas[0]）。restore_frames 会自动
        // 移除 trampoline meta 与 info，并恢复 spilled block 到 frame_metas/overflow 头部。
        if crate::frame_paging::FramePager::front_is_trampoline(&self.cx) {
            self.frame_pager.restore_frames(&mut self.cx);
        }
        Ok(())
    }

    #[inline]
    pub fn call_depth(&self) -> usize {
        self.frame_pager.depth()
    }
    pub fn frame_pager_stats(&self) -> &crate::frame_paging::FramePagerStats {
        self.frame_pager.stats()
    }

    // ========================================================================
    // Arena 逃逸检测与提升
    // ========================================================================

    /// 检测 Arena 对象是否逃逸到外层帧（spec 6.2）。
    ///
    /// SCHF v6 Phase 4：签名从 `&CallFrame` 改为 `(arena, base)`，
    /// 因为 CallFrame 已移除，arena 与 base 分别来自 FrameMeta.arena 与 FrameInfo.base。
    fn check_arena_escape(&self, arena: usize, base: usize) -> bool {
        let arena_state = match self.cx.region.frame_state(arena) {
            Some(s) => s,
            None => return false,
        };
        let arena_start = arena_state.start;
        if arena_start == 0 && arena_state.top == 0 {
            return false;
        }
        let window_end = self.cx.register_write_ptr;
        for idx in 0..window_end {
            if idx >= base {
                continue;
            }
            // SAFETY: idx < window_end <= registers.len(); the loop bound
            // ensures idx never reaches registers.len().
            let val = unsafe { self.cx.registers.get_unchecked(idx) };
            if let Some(offset) = val.try_arena_offset()
                && offset >= arena_start as u32
                && (arena_start + arena_state.top) as u32 > offset
            {
                return true;
            }
        }
        false
    }

    /// 提升指定帧的 Arena 对象到持久堆，并重写所有引用了这些 Arena 对象的 Value。
    ///
    /// # S4 修复：dangling pointer 重写
    ///
    /// 原实现调用 `promote_from_region` 后丢弃返回的 `new_heap_idx`，
    /// 导致 caller 寄存器 / 全局变量 / spill 值中仍持有旧的 arena 索引。
    /// Arena 帧结束后其内存可能被后续帧复用 → 旧 arena 索引变成悬垂指针 → UAF。
    ///
    /// 修复：收集 `(arena_obj_idx, new_heap_idx)` remap，提升后遍历所有根位置
    /// （caller 寄存器 `[0..caller_base)`、global_scope、frame_data），
    /// 把指向已提升 arena 对象的 Value 从 `from_arena_index(off)` 重写为
    /// `from_gc_index(new_idx)`。
    ///
    /// 注意：`Value::try_remap`（在 nuzo_core 中）只处理 scratch 索引
    /// （`>= SCRATCH_BASE`），不处理 arena 索引。故此处手动用
    /// `try_arena_offset()` + `from_gc_index()` 重写，无法复用 try_remap。
    pub(crate) fn promote_arena_range(
        &mut self,
        frame_idx: usize,
        caller_base: usize,
    ) -> Result<(), NuzoError> {
        let objects = match self.cx.region.frame_objects(frame_idx) {
            Some(slice) => slice,
            None => return Ok(()),
        };
        if objects.is_empty() {
            return Ok(());
        }
        let obj_start = match self.cx.region.frame_state(frame_idx) {
            Some(s) => s.obj_start,
            None => return Ok(()),
        };
        let obj_count = objects.len();
        // NLL: objects 的借用在 obj_count = objects.len() 后结束，
        // 后续 take_arena_object 需要 &mut self.cx.region。
        let mut remap: Vec<(u32, u32)> = Vec::with_capacity(obj_count);
        for i in 0..obj_count {
            let reverse_i = obj_count - 1 - i;
            let arena_obj_idx = (obj_start + reverse_i) as u32;
            if let Some(obj) = self.cx.region.take_arena_object(arena_obj_idx) {
                let size_est = obj.size_estimate();
                let new_heap_idx = self.gc.promote_from_region(obj, size_est);
                remap.push((arena_obj_idx, new_heap_idx));
            }
        }
        // 无提升对象时提前返回，避免不必要的根扫描。
        if remap.is_empty() {
            return Ok(());
        }
        // remap 按 arena_obj_idx 排序（binary_search 要求）。
        remap.sort_by_key(|(old, _)| *old);

        // 二分查找辅助闭包：给定 arena offset，返回对应的 new_heap_idx。
        let lookup = |arena_off: u32| -> Option<u32> {
            remap.binary_search_by_key(&arena_off, |(old, _)| *old).ok().map(|i| remap[i].1)
        };

        // 1) 重写 caller 寄存器 [0..caller_base)。
        //    registers 已在 pop_frame 中 truncate(info.base)，
        //    故 [0..caller_base) 是 caller 的寄存器窗口。
        let reg_end = caller_base.min(self.cx.registers.len());
        let slice = self.cx.registers.as_mut_slice();
        for value in slice[..reg_end].iter_mut() {
            if let Some(arena_off) = value.try_arena_offset()
                && let Some(new_idx) = lookup(arena_off)
            {
                *value = Value::from_gc_index(new_idx);
            }
        }

        // 2) 重写 global_scope（全局变量可能持有逃逸的 arena 值）。
        for i in 0..self.cx.global_scope.len() {
            if let Some(mut value) = self.cx.global_scope.get(i)
                && let Some(arena_off) = value.try_arena_offset()
                && let Some(new_idx) = lookup(arena_off)
            {
                value = Value::from_gc_index(new_idx);
                self.cx.global_scope.set(i, value);
            }
        }

        // 3) 重写 frame_data（spill 值可能持有逃逸的 arena 值）。
        let frame_top = self.cx.frame_data.top;
        let frame_data_len = self.cx.frame_data.data.len();
        let frame_top = frame_top.min(frame_data_len);
        for value in self.cx.frame_data.data[..frame_top].iter_mut() {
            if let Some(arena_off) = value.try_arena_offset()
                && let Some(new_idx) = lookup(arena_off)
            {
                *value = Value::from_gc_index(new_idx);
            }
        }

        Ok(())
    }

    // ========================================================================
    // Instruction Tracing (Cold Path)
    // ========================================================================

    #[cold]
    pub(crate) fn record_trace(
        &mut self,
        opcode: Opcode,
        operands: Vec<u16>,
        ip_before: usize,
        duration_ns: u128,
        registers_before: Option<Vec<Value>>,
    ) {
        let ip_after = self.ip;
        let frame_depth = self.call_depth();
        // SAFETY: chunk_ptr is null-checked in the if condition below; when non-null,
        // it points to a valid Arc<Chunk> whose lifetime covers this method.
        let source_location = if !self.chunk_ptr.is_null() {
            unsafe { &*self.chunk_ptr }.get_source_location(ip_before)
        } else {
            None
        };
        let function_name = self.current_frame_function_name();
        if let Some(ref mut tracer) = self.tracer {
            if !tracer.should_record(&opcode) {
                return;
            }
            let registers_after = if tracer.should_capture_registers() {
                Some(self.cx.registers.as_slice().to_vec())
            } else {
                None
            };
            tracer.record(
                opcode,
                operands,
                ip_before,
                ip_after,
                frame_depth,
                registers_before,
                registers_after,
                duration_ns,
                source_location,
                function_name,
            );
        }
    }

    #[inline]
    pub(crate) fn capture_registers_for_trace(&self) -> Option<Vec<Value>> {
        self.tracer.as_ref().and_then(|t| {
            if t.should_capture_registers() {
                Some(self.cx.registers.as_slice().to_vec())
            } else {
                None
            }
        })
    }

    #[inline]
    pub(crate) fn tracer_should_record(&self, opcode: &Opcode) -> bool {
        match &self.tracer {
            Some(t) => t.should_record(opcode),
            None => false,
        }
    }

    pub fn take_tracer_result(&mut self) -> Option<crate::tracer_state::TraceResult> {
        self.tracer.take().map(|t| {
            let c = t.instruction_counter();
            t.into_result(c)
        })
    }

    #[inline]
    pub fn current_ip(&self) -> usize {
        self.ip
    }
    #[inline]
    pub fn instruction_count(&self) -> usize {
        self.tracer.as_ref().map_or(0, |t| t.instruction_counter())
    }

    pub fn build_call_stack_for_debug(&self) -> Option<Vec<String>> {
        if self.chunk_ptr.is_null() {
            return None;
        }
        let call_stack = self.build_call_stack(self.ip.saturating_sub(1));
        if call_stack.is_empty() {
            return None;
        }
        Some(
            call_stack
                .iter()
                .rev()
                .map(|frame| {
                    let loc = frame.call_site.as_ref().map_or_else(
                        || {
                            format!(
                                "{}:{}",
                                frame.source_file.as_deref().unwrap_or("<unknown>"),
                                frame.definition_line.unwrap_or(0)
                            )
                        },
                        |site| format!("{}:{}", site.file, site.line),
                    );
                    format!("{} @ {}", frame.function_name, loc)
                })
                .collect(),
        )
    }

    // ========================================================================
    // Hot Trace JIT Helpers
    // ========================================================================

    #[inline(always)]
    fn peek_i16_from_current_ip(&self, operand_offset: usize) -> Option<i16> {
        if self.chunk_ptr.is_null() {
            return None;
        }
        // SAFETY: chunk_ptr is null-checked above; it points to a valid
        // Arc<Chunk> set during reset_and_load_chunk/pop_frame.
        let chunk = unsafe { &*self.chunk_ptr };
        let current_ip = self.ip;
        let offset_pos = current_ip.checked_add(operand_offset)?;
        if offset_pos + 1 < chunk.code().len() {
            let offset_bytes: [u8; 2] = chunk.code()[offset_pos..offset_pos + 2]
                .try_into()
                .expect("vm: bytecode offset conversion failed");
            Some(i16::from_le_bytes(offset_bytes))
        } else {
            None
        }
    }

    pub(super) fn is_backward_jump(&self, opcode: Opcode) -> bool {
        let operand_offset = match opcode {
            Opcode::Jmp => 0,
            Opcode::Test => 2,
            _ => return false,
        };
        self.peek_i16_from_current_ip(operand_offset).is_some_and(|offset| offset < 0)
    }

    pub(super) fn try_register_hot_trace(&mut self) {
        if !self.chunk_ptr.is_null() {
            // SAFETY: chunk_ptr is null-checked above; it points to a valid
            // Arc<Chunk> set during reset_and_load_chunk/pop_frame.
            let chunk = unsafe { &*self.chunk_ptr };
            self.cx.hot_trace_table.try_register_at_ip(chunk.code(), self.ip);
        }
    }

    // ========================================================================
    // Instruction Fetching & Decoding (Bounds-Elided)
    // ========================================================================

    #[inline(always)]
    pub(super) fn fetch_opcode(&mut self) -> Result<Opcode, NuzoError> {
        let byte = self.read_byte()?;
        Chunk::decode_opcode(byte)
            .ok_or_else(|| NuzoError::internal(InternalError::InvalidOpcode { opcode: byte }, None))
    }

    #[inline(always)]
    pub(super) fn read_byte(&mut self) -> Result<u8, NuzoError> {
        if self.chunk_ptr.is_null() {
            return Err(NuzoError::internal(InternalError::NoChunkLoaded, None));
        }
        // SAFETY: chunk_ptr is null-checked above; it points to a valid
        // Arc<Chunk> set during reset_and_load_chunk/pop_frame.
        let chunk = unsafe { &*self.chunk_ptr };
        let ip = self.ip;
        if ip >= chunk.code().len() {
            return Err(NuzoError::internal(
                InternalError::BytecodeOutOfBounds { ip, code_len: chunk.code().len() },
                None,
            ));
        }
        self.ip = ip + 1;
        // SAFETY: ip < chunk.code().len() verified by the bounds check above.
        Ok(unsafe { *chunk.code().get_unchecked(ip) })
    }

    #[inline(always)]
    pub(super) fn read_u16(&mut self) -> Result<u16, NuzoError> {
        if self.chunk_ptr.is_null() {
            return Err(NuzoError::internal(InternalError::NoChunkLoaded, None));
        }
        // SAFETY: chunk_ptr is null-checked above; it points to a valid
        // Arc<Chunk> set during reset_and_load_chunk/pop_frame.
        let chunk = unsafe { &*self.chunk_ptr };
        let ip = self.ip;
        if ip + 1 >= chunk.code().len() {
            return Err(NuzoError::internal(
                InternalError::BytecodeOutOfBounds { ip, code_len: chunk.code().len() },
                None,
            ));
        }
        self.ip = ip + 2;
        // SAFETY: ip + 1 < chunk.code().len() verified by the bounds check above,
        // so both ip and ip+1 are valid indices into chunk.code().
        Ok((unsafe { *chunk.code().get_unchecked(ip) } as u16)
            | ((unsafe { *chunk.code().get_unchecked(ip + 1) } as u16) << 8))
    }

    #[inline]
    pub(super) fn read_i16(&mut self) -> Result<i16, NuzoError> {
        Ok(self.read_u16()? as i16)
    }

    #[inline]
    pub(super) fn current_chunk(&self) -> Result<&Chunk, NuzoError> {
        if self.chunk_ptr.is_null() {
            Err(NuzoError::internal(InternalError::NoChunkLoaded, None))
        } else {
            // SAFETY: chunk_ptr is null-checked above; it points to a valid
            // Arc<Chunk> set during reset_and_load_chunk/pop_frame.
            Ok(unsafe { &*self.chunk_ptr })
        }
    }

    #[inline]
    pub(super) fn current_closure(&self) -> Option<Arc<HeapObject>> {
        // SCHF v6 Phase 3：closure 改读 frame_metas.last()（spec 4.4 + 2.4）。
        // frame_metas 与 frames 一一对应（含 spill 后），last() 始终是当前帧。
        self.current_meta().and_then(|m| m.closure.clone())
    }

    // ========================================================================
    // ISS: Instruction Self-Specialization — 字节码修补 API
    // ========================================================================

    #[inline]
    pub(super) fn patch_code(&mut self, ip: usize, new_bytes: &[u8]) -> Result<(), NuzoError> {
        /// 尝试把 `new_bytes` 写入 `chunk` 的 `code` 区间 [ip, ip+new_bytes.len())。
        /// 任何边界错误都返回 `PatchOverflow`，由调用方决定是否降级。
        fn try_patch(chunk: &mut Chunk, ip: usize, new_bytes: &[u8]) -> Result<(), NuzoError> {
            let code = chunk.code_mut();
            let end = ip
                .checked_add(new_bytes.len())
                .ok_or_else(|| NuzoError::internal(InternalError::PatchOverflow, None))?;
            if end > code.len() {
                return Err(NuzoError::internal(InternalError::PatchOverflow, None));
            }
            code[ip..end].copy_from_slice(new_bytes);
            Ok(())
        }

        let chunk_arc = self
            .chunk
            .as_mut()
            .ok_or_else(|| NuzoError::internal(InternalError::NoChunkLoaded, None))?;

        if let Some(chunk) = Arc::get_mut(chunk_arc) {
            // 独占持有：直接原地修补。
            try_patch(chunk, ip, new_bytes)?;
        } else {
            // Chunk 被共享（chunk_cache、caller_chunk、外部克隆等）。
            // ISS 只是优化，必须避免污染其他持有者，因此克隆一份再修补。
            let mut cloned = (**chunk_arc).clone();
            if try_patch(&mut cloned, ip, new_bytes).is_err() {
                // 克隆后仍无法安全修补（如边界异常），跳过本次 ISS 优化。
                return Ok(());
            }
            let new_arc = Arc::new(cloned);
            self.chunk_ptr = Arc::as_ptr(&new_arc);
            self.chunk = Some(new_arc);
            if let Some(ref c) = self.chunk {
                self.cx.inline_cache.resize(c.code().len(), InlineCacheEntry::default());
            }
        }

        Ok(())
    }

    // ========================================================================
    // Execution Environment Setup
    // ========================================================================

    pub(super) fn reset_and_load_chunk(&mut self, chunk: Chunk) {
        self.ip = 0;
        self.cx.snapshot_for_chunk_switch();
        self.cx.reset_registers_and_frames(chunk.locals_count);
        self.frame_pager.reset();
        self.current_base = 0;
        let vm_ptr = self as *mut VM;
        crate::gc::heap::set_gc_heap_gc_ptr(&mut *self.gc);
        self.gc.register_roots_fn(
            Some(crate::vm::gc_roots_trampoline),
            vm_ptr as *mut std::ffi::c_void,
        );
        crate::gc::install_scratch_aware_accessors(self.gc.scratch_data_ptr(), &mut self.cx.region);
        crate::gc::update_gc_chunks_ptr(&self.gc);
        let arc = Arc::new(chunk);
        self.chunk_ptr = Arc::as_ptr(&arc);
        self.chunk = Some(arc);
        self.invalidate_cigc_cache();
        if let Some(ref c) = self.chunk {
            self.cx.inline_cache.resize(c.code().len(), InlineCacheEntry::default());
        }
        if self.error_collector.is_enabled() {
            self.error_collector.clear();
        }
    }

    pub(super) fn get_or_create_chunk(
        &mut self,
        prototype: &Arc<nuzo_values::FunctionPrototype>,
    ) -> Arc<Chunk> {
        let key = Arc::as_ptr(prototype) as usize;
        if let Some(cached) = self.cx.chunk_cache.get(&key) {
            return Arc::clone(cached);
        }
        let chunk = Arc::new(Chunk::from_arcs(
            Arc::clone(&prototype.chunk),
            Arc::clone(&prototype.constants),
            Arc::clone(&prototype.lines),
            Arc::clone(&prototype.debug_info),
            prototype.locals_count,
            prototype.spill_slot_count,
        ));
        self.cx.chunk_cache.insert(key, Arc::clone(&chunk));
        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_bytecode::Opcode;

    #[test]
    fn test_is_backward_jump_for_negative_jmp() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Jmp);
        chunk.write_i16(-3);
        vm.reset_and_load_chunk(chunk);
        let opcode = vm.fetch_opcode().expect("fetch jmp");
        assert!(vm.is_backward_jump(opcode));
    }

    #[test]
    fn test_is_backward_jump_for_positive_test_is_false() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Test);
        chunk.write_u16(0);
        chunk.write_i16(4);
        vm.reset_and_load_chunk(chunk);
        let opcode = vm.fetch_opcode().expect("fetch test");
        assert!(!vm.is_backward_jump(opcode));
    }

    // ========================================================================
    // ISS patch_code Arc 独占性测试
    // ========================================================================

    /// 构造一个 7 字节的 GetGlobal 指令占位，与 ISS 实际布局一致。
    fn build_getglobal_chunk() -> Chunk {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::GetGlobal);
        chunk.write_u16(0); // dest
        chunk.write_u16(0); // name_idx
        chunk.write_u16(0); // ISS padding
        chunk
    }

    #[test]
    fn test_patch_code_unique_arc_patches_in_place() {
        let mut vm = VM::new();
        vm.reset_and_load_chunk(build_getglobal_chunk());

        // 不提前 clone，确保 strong_count == 1，测试真正的独占路径。
        assert_eq!(
            Arc::strong_count(vm.chunk.as_ref().unwrap()),
            1,
            "freshly loaded chunk must be uniquely owned"
        );
        let original_ptr = vm.chunk_ptr;

        vm.patch_code(0, &[Opcode::GetGlobalCached as u8])
            .expect("patch on unique Arc should succeed");

        // 独占持有：应该原地修补，指针不变。
        assert_eq!(vm.chunk_ptr, original_ptr);
        assert_eq!(vm.current_chunk().unwrap().code()[0], Opcode::GetGlobalCached as u8);
    }

    #[test]
    fn test_patch_code_shared_arc_clones_without_polluting_others() {
        let mut vm = VM::new();
        vm.reset_and_load_chunk(build_getglobal_chunk());

        // 模拟外部共享：chunk_cache、caller_frame 或外部克隆都会让 strong_count > 1。
        let external_clone = vm.chunk.clone().expect("chunk loaded");
        assert_eq!(Arc::strong_count(&external_clone), 2, "chunk must be shared before patch");

        vm.patch_code(0, &[Opcode::GetGlobalCached as u8])
            .expect("patch on shared Arc should succeed by cloning");

        // 外部持有者必须保持原字节码不变。
        assert_eq!(external_clone.code()[0], Opcode::GetGlobal as u8);
        // VM 必须指向新的、已修补的克隆。
        assert_eq!(vm.current_chunk().unwrap().code()[0], Opcode::GetGlobalCached as u8);
        assert!(!Arc::ptr_eq(vm.chunk.as_ref().unwrap(), &external_clone));
        assert_eq!(
            Arc::strong_count(&external_clone),
            1,
            "external clone should remain with its single owner"
        );
    }

    #[test]
    fn test_patch_code_shared_arc_overflow_skips_safely() {
        let mut vm = VM::new();
        vm.reset_and_load_chunk(build_getglobal_chunk());

        let external_clone = vm.chunk.clone().expect("chunk loaded");

        // 共享状态下越界：ISS 是优化，应跳过而非报错。
        vm.patch_code(100, &[0xFF]).expect("shared overflow should be silently skipped");

        assert_eq!(external_clone.code()[0], Opcode::GetGlobal as u8);
        assert_eq!(vm.current_chunk().unwrap().code()[0], Opcode::GetGlobal as u8);
        assert!(Arc::ptr_eq(vm.chunk.as_ref().unwrap(), &external_clone));
    }

    #[test]
    fn test_patch_code_unique_arc_overflow_reports_error() {
        let mut vm = VM::new();
        vm.reset_and_load_chunk(build_getglobal_chunk());

        // 独占状态下越界：返回错误，便于上层定位 ISS 逻辑缺陷。
        let result = vm.patch_code(100, &[0xFF]);
        assert!(result.is_err(), "unique Arc overflow should report PatchOverflow");
    }
}
