//! # 变量操作 — 全局/局部/upvalue 变量存取
//!
//! 负责编译期声明的变量在运行时的读写操作，
//! 包括全局变量表、局部寄存器和 upvalue 闭包捕获三种路径。

use crate::gc::{Gc, Trace, is_scratch};
use crate::vm_lic::CallTargetType;
use nuzo_core::DIAGNOSTIC_REGISTER_WINDOW;
use nuzo_core::Value;
use nuzo_core::encoding::{char_at, char_len};
use nuzo_core::tag;
use nuzo_error::ErrorCollector;
use nuzo_error::ExecutionContext as NuzoErrorContext;
use nuzo_error::classifier::ErrorClassifier;
use nuzo_values::ValueExt;
use nuzo_values::{HeapObject, InternalError, NuzoError, VmDiagnosis};

use super::{BUILTIN_ARGS_BUF_SIZE, FrameKind, VM, extract_type_tag};

impl VM {
    // ---- Running State & GC Accessors ----
    #[inline]
    pub fn is_running(&self) -> bool {
        self.cx.running
    }
    #[inline]
    pub fn gc(&self) -> &Gc {
        &self.gc
    }
    #[inline]
    pub fn gc_mut(&mut self) -> &mut Gc {
        &mut self.gc
    }
    #[inline]
    pub fn hot_trace_events(&self) -> &[crate::vm_hot_trace::HotTraceEvent] {
        &self.cx.hot_trace_table.events
    }

    // ---- GC Root Collection & Safe Point ----
    pub(super) fn collect_gc_roots(&self, gc: &mut Gc) {
        let trace_end = self.cx.register_write_ptr.min(self.cx.registers.len());
        for value in &self.cx.registers.as_slice()[..trace_end] {
            value.trace(gc);
        }
        for i in 0..self.cx.global_scope.len() {
            if let Some(value) = self.cx.global_scope.get(i) {
                value.trace(gc);
            }
        }
        // Root constants of the currently executing chunk and any cached chunks.
        if !self.chunk_ptr.is_null() {
            let chunk = unsafe { &*self.chunk_ptr };
            for value in chunk.constants() {
                value.trace(gc);
            }
        }
        for cached in self.cx.chunk_cache.values() {
            for value in cached.constants() {
                value.trace(gc);
            }
        }
        // Root constants of all registered lazy-import modules (chunks held by module_cache).
        // These chunks may contain heap-allocated constants (strings, arrays, etc.)
        // that the GC must trace to avoid premature collection while the module
        // is pending execution or actively executing via OP_INIT_MODULE frame switch.
        for module_chunk in self.cx.module_cache.values() {
            for value in module_chunk.constants() {
                value.trace(gc);
            }
        }
        // SCHF v6 Phase 4: spill 值存储在 frame_data.data 中（非旧 CallFrame.spill_stack）。
        // frame_data.data[0..top] 包含所有帧的 locals（NIL 占位）+ spill 槽（实际值）。
        // locals 在 frame_data.data 中始终为 NIL（实际值在 registers 中，已上方 trace），
        // 故此处仅 spill 槽会触发实际 trace，不会重复 trace locals。
        for value in &self.cx.frame_data.data[..self.cx.frame_data.top] {
            value.trace(gc);
        }
        // SCHF v6 Phase 4: for_each_frame 遍历 FrameMeta（含 spilled_blocks 中的换出帧）。
        // FrameMeta 包含 closure 与 tco_history，是 GC 根的唯一来源（VecDeque 已移除）。
        self.frame_pager.for_each_frame(&self.cx, |meta| {
            if let Some(ref closure) = meta.closure {
                Trace::trace(&**closure, gc);
            }
            for record in &meta.tco_history {
                if let Some(ref replaced) = record.replaced_closure {
                    Trace::trace(&**replaced, gc);
                }
            }
        });
        if let Some(ref exc) = self.pending_exception {
            exc.trace(gc);
        }
    }

    pub(super) fn gc_safe_point(&mut self) -> Result<(), NuzoError> {
        let remap = self.gc.safe_point(|| {
            let mut scratch_indices = Vec::new();
            let trace_end = self.cx.register_write_ptr.min(self.cx.registers.len());
            for value in &self.cx.registers.as_slice()[..trace_end] {
                if tag::is_heap_object(value.into_raw_bits())
                    && tag::is_gc_managed(value.into_raw_bits())
                    && let Some(idx) = value.heap_index()
                    && is_scratch(idx)
                {
                    scratch_indices.push(idx);
                }
            }
            for i in 0..self.cx.global_scope.len() {
                if let Some(value) = self.cx.global_scope.get(i)
                    && tag::is_heap_object(value.into_raw_bits())
                    && tag::is_gc_managed(value.into_raw_bits())
                    && let Some(idx) = value.heap_index()
                    && is_scratch(idx)
                {
                    scratch_indices.push(idx);
                }
            }
            // SCHF v6 Phase 4: spill 值在 frame_data.data[0..top] 中，扫描 scratch 索引。
            for value in &self.cx.frame_data.data[..self.cx.frame_data.top] {
                if tag::is_heap_object(value.into_raw_bits())
                    && tag::is_gc_managed(value.into_raw_bits())
                    && let Some(idx) = value.heap_index()
                    && is_scratch(idx)
                {
                    scratch_indices.push(idx);
                }
            }
            // SCHF v6 Phase 4: for_each_frame 遍历 FrameMeta（含 spilled_blocks）。
            self.frame_pager.for_each_frame(&self.cx, |meta| {
                if let Some(ref closure) = meta.closure {
                    closure.trace_gc(&mut |idx| {
                        if is_scratch(idx) {
                            scratch_indices.push(idx);
                        }
                    });
                }
            });
            scratch_indices
        });
        crate::gc::update_gc_chunks_ptr(&self.gc);
        if !remap.is_empty() {
            let trace_end = self.cx.register_write_ptr.min(self.cx.registers.len());
            let slice = self.cx.registers.as_mut_slice();
            for value in slice[..trace_end].iter_mut() {
                value.try_remap(&remap);
            }
            for i in 0..self.cx.global_scope.len() {
                if let Some(mut value) = self.cx.global_scope.get(i)
                    && value.try_remap(&remap)
                {
                    self.cx.global_scope.set(i, value);
                }
            }
            // S2 修复：重写 frame_data（spill 值）中的 scratch 索引。
            // 原代码只扫描 frame_data 作为根（用于提升），但提升后未重写这些
            // Value → spill 值持有悬垂的 scratch 索引 → UAF。
            let frame_top = self.cx.frame_data.top;
            for value in self.cx.frame_data.data[..frame_top].iter_mut() {
                value.try_remap(&remap);
            }
            // 注：safe_point 内部已对被提升对象调用 remap_scratch_indices（传递性
            // 重写对象内部引用），此处无需重复。闭包捕获（Arc<HeapObject>）的
            // scratch 引用重写受限于 Arc::get_mut 的单引用约束，当前实现通过
            // trace_gc 把闭包捕获的 scratch 索引作为根提升，确保对象不被误回收；
            // 但闭包内部对旧 scratch 索引的引用在 Arc 多引用时无法原地重写，
            // 这是一个已知限制（需要 nuzo_values 提供 make_mut 或重构闭包存储）。
        }
        Ok(())
    }

    // ---- Diagnostic Mode ----
    pub fn enable_diagnostic_mode(&mut self) {
        self.error_collector.enable();
    }
    pub fn disable_diagnostic_mode(&mut self) {
        self.error_collector.disable();
    }
    #[inline]
    pub fn is_diagnostic_mode(&self) -> bool {
        self.error_collector.is_enabled()
    }
    #[inline]
    pub(super) fn is_tracer_active(&self) -> bool {
        self.tracer.is_some()
    }
    pub fn with_max_diagnostic_errors(&mut self, max: usize) {
        self.error_collector.max_errors(max);
    }
    pub fn with_stop_on_fatal(&mut self, stop: bool) {
        self.error_collector.with_stop_on_fatal(stop);
    }
    #[inline]
    pub fn error_collector(&self) -> &ErrorCollector {
        &self.error_collector
    }
    #[inline]
    pub fn error_collector_mut(&mut self) -> &mut ErrorCollector {
        &mut self.error_collector
    }
    pub fn print_diagnostic_report(&self) {
        self.error_collector.print_full_report();
    }
    #[inline]
    pub fn diagnostic_error_count(&self) -> usize {
        self.error_collector.error_count()
    }
    #[inline]
    pub fn has_diagnostic_errors(&self) -> bool {
        self.error_collector.has_errors()
    }
    pub fn clear_diagnostics(&mut self) {
        self.error_collector.clear();
    }
    #[inline]
    pub fn last_call_stack(&self) -> &[nuzo_error::StackFrameInfo] {
        &self.cx.last_call_stack
    }

    #[cold]
    #[inline(never)]
    pub(super) fn with_current_source_location(&self, error: NuzoError, ip: usize) -> NuzoError {
        if error.source_location.is_some() {
            return error;
        }
        if self.chunk_ptr.is_null() {
            return error;
        }
        let chunk = unsafe { &*self.chunk_ptr };
        match chunk.get_source_location(ip) {
            Some(loc) => error.with_source_location(loc),
            None => error,
        }
    }

    pub(super) fn build_call_stack(&self, _error_ip: usize) -> Vec<nuzo_error::StackFrameInfo> {
        use nuzo_error::StackFrameInfo;

        // SCHF v6 Phase 4：遍历 frame_metas（VecDeque 已移除）。
        // base 不在 FrameMeta 中（spec 2.4），从 frame_ring + frame_overflow 重建。
        // frame_metas 与 (ring + overflow) 在 push/pop/spill/restore 全程同步，
        // 故按 meta 索引对应读取 info.base。
        let metas = &self.cx.frame_metas;
        if metas.is_empty() {
            return Vec::new();
        }

        let has_trampoline = metas[0].kind == FrameKind::Trampoline;
        let ring_depth = metas.len().saturating_sub(self.cx.frame_overflow.infos.len());

        // 收集 ring 中的 base（cold path，O(N) 可接受）
        let ring_bases: Vec<usize> = self.cx.frame_ring.iter().map(|info| info.base).collect();

        let mut result = Vec::with_capacity(metas.len());

        for (i, meta) in metas.iter().enumerate() {
            // 跳过桩帧（spill 时插入的占位帧，不对应真实调用）
            if meta.kind == FrameKind::Trampoline {
                continue;
            }

            let base = if has_trampoline || ring_depth == 0 {
                // Post-spill 或 ring 已清空：所有 info 在 overflow。
                // trampoline（若存在）在 overflow[0] 与 metas[0] 对齐，故索引 i 一致。
                self.cx.frame_overflow.infos.get(i).map(|info| info.base).unwrap_or(0)
            } else if i < ring_depth {
                // ring 内：info 在 ring.slots[i]
                ring_bases.get(i).copied().unwrap_or(0)
            } else {
                // ring 外：info 在 overflow[i - ring_depth]
                self.cx.frame_overflow.infos.get(i - ring_depth).map(|info| info.base).unwrap_or(0)
            };

            // 从该帧对应的函数原型 / 当前 chunk 提取定义信息
            let (fn_name, source_file, definition_line) = match &meta.closure {
                Some(obj) => match obj.as_ref() {
                    HeapObject::Closure { prototype, .. } => {
                        let debug = &prototype.debug_info;
                        let name =
                            debug.function_name.clone().unwrap_or_else(|| prototype.name.clone());
                        let file = debug.source_file.clone();
                        let line = debug.ip_to_line.values().min().copied();
                        (name, Some(file), line)
                    }
                    _ => (format!("frame_{}", i), None, None),
                },
                None => {
                    // 顶层脚本帧使用当前正在执行的 chunk 调试信息
                    let mut file = None;
                    let mut line = None;
                    if !self.chunk_ptr.is_null() {
                        let chunk = unsafe { &*self.chunk_ptr };
                        let debug = &chunk.debug_info;
                        file = Some(debug.source_file.clone());
                        line = debug.ip_to_line.values().min().copied();
                    }
                    ("<script>".to_string(), file, line)
                }
            };

            let mut info = StackFrameInfo::new(fn_name, base);
            if let Some(file) = source_file
                && let Some(line) = definition_line
            {
                info.source(file, line);
            }
            if let Some(site) = meta.call_site.clone() {
                info.call_site(site);
            }
            result.push(info);
        }

        result
    }

    #[cold]
    #[allow(dead_code)] // 内部错误诊断 API，保留供调试模式使用
    pub(super) fn diagnose_internal_error(&self, error: &InternalError) -> VmDiagnosis {
        let disassembly = if !self.chunk_ptr.is_null() {
            unsafe { &*self.chunk_ptr }.disassemble()
        } else {
            "<no chunk loaded>".to_string()
        };
        let error_ip =
            if !self.chunk_ptr.is_null() { Some(self.ip.saturating_sub(1)) } else { None };
        let default_val = Value::default();
        let register_snapshot: Vec<(u16, String)> = self
            .cx
            .registers
            .iter()
            .enumerate()
            .filter(|(_, v)| **v != default_val)
            .map(|(i, v)| (i as u16, format!("{} ({})", v, v.type_name())))
            .collect();
        VmDiagnosis {
            disassembly,
            error_ip,
            register_snapshot,
            call_stack_depth: self.frame_depth(),
            root_cause_analysis: ErrorClassifier::root_cause(error),
        }
    }

    #[cold]
    pub(super) fn handle_error_in_diagnostic_mode(
        &mut self,
        error: NuzoError,
        opcode: Option<nuzo_bytecode::Opcode>,
        instr_ip: Option<usize>,
    ) -> bool {
        if !self.error_collector.is_enabled() {
            return false;
        }
        let error_ip = instr_ip.unwrap_or_else(|| self.ip.saturating_sub(1));
        let source_location = if !self.chunk_ptr.is_null() {
            unsafe { &*self.chunk_ptr }.get_source_location(error_ip)
        } else {
            None
        };
        let mut context = match source_location {
            Some(loc) => NuzoErrorContext::with_source(error_ip, opcode, self.call_depth(), loc),
            None => NuzoErrorContext::new(error_ip, opcode, self.call_depth()),
        };
        let start = self.cx.registers.len().saturating_sub(DIAGNOSTIC_REGISTER_WINDOW);
        for (i, &val) in self.cx.registers.iter().enumerate().skip(start) {
            context.add_register(i, val);
        }
        let call_stack = self.build_call_stack(error_ip);
        self.error_collector.collect_error(error, context, call_stack)
    }

    // ---- Stack & Register Operations ----
    #[allow(dead_code)] // 弹性栈预留扩展点，当前为 no-op，保留供后续栈溢出检测
    #[inline(always)]
    pub(super) fn check_and_expand_stack(&mut self, _needed: usize) -> Result<(), NuzoError> {
        Ok(())
    }

    #[inline(always)]
    pub(super) fn register(&self, reg: u16) -> Result<Value, NuzoError> {
        let idx = self.current_base + reg as usize;
        self.cx.registers.get(idx).ok_or_else(|| {
            NuzoError::internal(
                InternalError::RegisterOutOfBounds {
                    reg,
                    available: self.cx.registers.len().saturating_sub(self.current_base),
                },
                None,
            )
        })
    }

    #[inline(always)]
    pub(super) fn set_register(&mut self, reg: u16, value: Value) -> Result<(), NuzoError> {
        // H1 修复: 防御性边界检查。ElasticRegisterFile::set 会自动扩容，
        // 但若 reg 超过 MAX_FUNCTION_LOCALS 表示编译器约束被违反（应早在
        // codegen 阶段返回 TooManyLocals）。此处返回错误以防内部不变量违反
        // 导致后续崩溃或内存无限增长。
        if reg as u32 > nuzo_core::MAX_FUNCTION_LOCALS as u32 {
            return Err(NuzoError::internal(
                InternalError::RegisterOutOfBounds {
                    reg,
                    available: nuzo_core::MAX_FUNCTION_LOCALS as usize,
                },
                None,
            ));
        }
        self.cx.registers.set(self.current_base + reg as usize, value);
        Ok(())
    }

    #[inline(always)]
    pub(super) fn register_tagged(&self, reg: u16) -> (u64, crate::trf::RegTag) {
        let idx = self.current_base + reg as usize;
        match self.cx.registers.get(idx) {
            Some(val) => {
                (val.into_raw_bits(), crate::trf::TypedRegFile::infer_tag(val.into_raw_bits()))
            }
            None => (nuzo_core::tag::NIL_VALUE, crate::trf::RegTag::Nil),
        }
    }

    #[inline(always)]
    pub(super) fn set_register_tagged(
        &mut self,
        reg: u16,
        raw: u64,
        _tag: crate::trf::RegTag,
    ) -> Result<(), NuzoError> {
        // H1 修复: 同 set_register 的边界检查，防止编译器约束违反导致内存无限扩容
        if reg as u32 > nuzo_core::MAX_FUNCTION_LOCALS as u32 {
            return Err(NuzoError::internal(
                InternalError::RegisterOutOfBounds {
                    reg,
                    available: nuzo_core::MAX_FUNCTION_LOCALS as usize,
                },
                None,
            ));
        }
        self.cx
            .registers
            .set(self.current_base + reg as usize, unsafe { Value::from_raw_bits(raw) });
        Ok(())
    }

    pub fn push(&mut self, value: Value) -> Result<u16, NuzoError> {
        let idx = self.cx.registers.len();
        // 边界检查：寄存器索引必须落在 u16 域内，否则 `as u16` 截断返回错误句柄，
        // 后续 pop/peek 会错位。超限时拒绝 push 以保持状态一致（根源修复）。
        if idx > u16::MAX as usize {
            return Err(NuzoError::internal(InternalError::RegisterOverflow { count: idx }, None));
        }
        self.cx.registers.push(value);
        Ok(idx as u16)
    }
    pub fn pop(&mut self) -> Result<Value, NuzoError> {
        self.cx.registers.pop().ok_or_else(|| {
            NuzoError::internal(InternalError::StackUnderflow { operation: "pop".into() }, None)
        })
    }
    pub fn peek(&self, offset: usize) -> Result<Value, NuzoError> {
        let idx =
            self.cx.registers.len().checked_sub(1).and_then(|t| t.checked_sub(offset)).ok_or_else(
                || {
                    NuzoError::internal(
                        InternalError::StackUnderflow {
                            operation: format!("peek at offset {}", offset),
                        },
                        None,
                    )
                },
            )?;
        Ok(self.cx.registers[idx])
    }
    #[inline]
    pub fn stack_size(&self) -> usize {
        self.cx.registers.len()
    }
    pub fn clear_stack(&mut self) {
        self.cx.registers.clear();
    }

    // ---- Exception Handling API ----
    #[inline(always)]
    pub(super) fn set_ip(&mut self, ip: usize) {
        self.ip = ip;
    }
    #[inline]
    #[allow(dead_code)] // 异常处理查询 API，保留供异常调试使用
    pub(super) fn pending_exception(&self) -> Option<Value> {
        self.pending_exception
    }
    #[inline]
    pub fn set_pending_exception(&mut self, ex: Option<Value>) {
        self.pending_exception = ex;
    }
    #[inline]
    pub fn clear_pending_exception(&mut self) {
        self.pending_exception = None;
    }

    // ---- Index Access Helper ----
    #[allow(dead_code)] // 索引访问辅助，保留供后续运算符重载/索引表达式使用
    pub(super) fn get_index(&self, obj: Value, idx: Value) -> Result<Value, NuzoError> {
        if let Some(s) = obj.as_string_opt()
            && let Some(num) = idx.try_number()
        {
            if num < 0.0 {
                return Err(NuzoError::index_out_of_bounds(num.to_string(), s.len().to_string()));
            }
            let i = num as usize;
            let len = char_len(&s);
            if i >= len {
                return Err(NuzoError::index_out_of_bounds(idx.to_string(), len.to_string()));
            }
            return char_at(&s, i)
                .map(|ch| Value::from_string(&ch.to_string()))
                .ok_or_else(|| NuzoError::index_out_of_bounds(idx.to_string(), len.to_string()));
        }
        if obj.is_ptr() {
            return Err(NuzoError::type_mismatch(
                "array or string with implemented index access",
                format!("{} (heap object)", obj.type_name()),
            ));
        }
        Err(NuzoError::type_mismatch("array or string", obj.type_name()))
    }

    // ---- Tail Position Detection ----
    pub(super) fn is_tail_position(&self, ip: usize) -> bool {
        if !self.chunk_ptr.is_null() {
            let chunk = unsafe { &*self.chunk_ptr };
            if ip < chunk.code().len()
                && let Some(next_op) = nuzo_bytecode::Chunk::decode_opcode(chunk.code()[ip])
            {
                return next_op == nuzo_bytecode::Opcode::Return;
            }
        }
        false
    }
    #[inline(always)]
    pub(super) fn current_frame_base(&self) -> usize {
        self.current_base
    }

    // ---- Current Frame Function Name ----
    #[inline]
    pub(super) fn current_frame_function_name(&self) -> Option<String> {
        // SCHF v6 Phase 3：closure 改读 frame_metas.last()（spec 4.4 + 2.4）。
        // frame_metas.last() 始终对应当前帧（含 spill 后）。
        self.current_meta().map(|meta| match &meta.closure {
            Some(obj) => match obj.as_ref() {
                HeapObject::Closure { prototype, .. } => prototype.name.clone(),
                _ => format!("frame_{}", self.frame_depth()),
            },
            None => "<script>".to_string(),
        })
    }

    // ---- Global Variable Access ----
    #[inline]
    pub fn get_global(&self, idx: usize) -> Option<Value> {
        self.cx.global_scope.get(idx)
    }
    #[inline]
    pub fn set_global(&mut self, idx: usize, value: Value) -> Result<(), NuzoError> {
        self.cx.global_scope.set(idx, value);
        Ok(())
    }
    pub fn add_global(&mut self, value: Value) -> usize {
        let idx = self.cx.global_scope.len();
        self.cx.global_scope.define(&format!("__global_{}", idx), value)
    }
    #[inline]
    pub fn global_count(&self) -> usize {
        self.cx.global_scope.len()
    }
    #[inline]
    pub fn resolve_global(&self, name: &str) -> Option<usize> {
        self.cx.global_scope.resolve(name)
    }
    #[inline]
    pub fn define_global(&mut self, name: &str, value: Value) -> usize {
        self.cx.global_scope.define(name, value)
    }
    pub fn get_global_by_name(&self, name: &str) -> Option<Value> {
        self.cx.global_scope.resolve(name).and_then(|idx| self.cx.global_scope.get(idx))
    }

    #[inline]
    pub(super) fn invalidate_cigc_cache(&mut self) {
        for entry in &mut self.cx.global_cache {
            entry.index = u32::MAX;
            entry.version = u32::MAX;
        }
    }

    pub fn set_global_by_name(&mut self, name: &str, value: Value) {
        if let Some(idx) = self.cx.global_scope.resolve(name) {
            self.cx.global_scope.set(idx, value);
        } else {
            self.cx.global_scope.define(name, value);
        }
    }

    // ---- Runtime Variable Inspection APIs ----
    #[inline]
    pub fn lookup_global(&self, name: &str) -> Option<Value> {
        self.cx.global_scope.resolve(name).and_then(|idx| self.cx.global_scope.get(idx))
    }
    #[inline]
    pub fn global_names(&self) -> Vec<String> {
        self.cx.global_scope.names()
    }

    pub fn local_info(&self) -> Vec<(usize, Value)> {
        let end = self.cx.register_write_ptr.min(self.cx.registers.len());
        if end <= self.current_base {
            return Vec::new();
        }
        let default_val = Value::default();
        (self.current_base..end)
            .filter_map(|idx| {
                let val = self.cx.registers[idx];
                if val != default_val { Some((idx, val)) } else { None }
            })
            .collect()
    }

    // ---- MLIC Inline Cache Dispatch ----
    #[inline(always)]
    pub(super) fn execute_builtin_fast(
        &mut self,
        fn_ptr: nuzo_values::heap::BuiltinFnPtr,
        func_reg: u16,
        argc: usize,
    ) -> Result<Value, NuzoError> {
        if argc <= BUILTIN_ARGS_BUF_SIZE {
            for i in 0..argc {
                self.builtin_args_buf[i] = self.register(func_reg + 1 + i as u16)?;
            }
            let result = (fn_ptr)(&self.builtin_args_buf[..argc]).map_err(|e| {
                NuzoError::type_mismatch("valid arguments for cached builtin", format!("{}", e))
            })?;
            self.set_register(func_reg, result)?;
            Ok(result)
        } else {
            let mut args = Vec::with_capacity(argc);
            for i in 0..argc {
                args.push(self.register(func_reg + 1 + i as u16)?);
            }
            let result = (fn_ptr)(&args).map_err(|e| {
                NuzoError::type_mismatch("valid arguments for cached builtin", format!("{}", e))
            })?;
            self.set_register(func_reg, result)?;
            Ok(result)
        }
    }

    pub(super) fn update_call_site_cache(&mut self, call_ip: usize, func_val: Value) {
        let raw_value_bits = func_val.into_raw_bits();
        let (target_type, prototype, builtin_name, builtin_fn, builtin_arity) =
            if func_val.is_builtin_fn() {
                if let Some((name, arity, fn_ptr)) = func_val.as_builtin_fn_opt() {
                    (CallTargetType::Builtin, None, Some(name), Some(fn_ptr), Some(arity as u8))
                } else {
                    (CallTargetType::Builtin, None, None, None, None)
                }
            } else if func_val.is_closure() {
                (CallTargetType::Closure, func_val.as_closure_opt(), None, None, None)
            } else {
                (CallTargetType::Unknown, None, None, None, None)
            };
        self.cx.call_sites.ensure(call_ip).update_cache(
            raw_value_bits,
            target_type,
            prototype,
            builtin_name,
            builtin_fn,
            builtin_arity,
            raw_value_bits,
        );
        if let Some(entry) = self.cx.inline_cache.get_mut(call_ip) {
            entry.record(extract_type_tag(func_val));
        }
    }
}
