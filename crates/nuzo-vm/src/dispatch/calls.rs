//! # 函数调用派发
//!
//! 包含函数调用的核心派发逻辑：
//! - `execute_normal_call` — 普通调用（builtin / closure）
//! - `execute_closure_fast` — CSTS 快速路径调用闭包
//! - `populate_csts_snapshot` — 填充 CSTS 快照 + 创建 CDD invoker
//! - `op_call` — Call 指令入口（派发到 fast path 或 slow path）
//! - `op_return` — Return 指令
//! - `op_halt` — Halt 指令
//!
//! ## `op_call` 子函数拆分
//! - `try_builtin_l1_fast_path` — Builtin L1 单态缓存命中时的快速调用
//! - `try_cdd_fast_path` — CDD (Closure Direct Dispatch) 闭包直接调用
//! - `update_call_site_after_call` — 调用成功后更新调用点缓存

use crate::vm::VM;
use crate::vm_lic::{CallSiteState, CallTargetType};
use nuzo_abi::NuzoErrorExt;
use nuzo_bytecode::Chunk;
use nuzo_values::*;
use std::sync::Arc;

use super::cache_types::{ClosureInvoker, ClosureSnapshot};
use super::cold_path::{err_compiler_bug, err_stack_overflow};

impl VM {
    // ========================================================================
    // Function Call Execution
    // ========================================================================

    /// 冷路径：普通调用（builtin / closure），仅在 fast path 全部 miss 时进入。
    #[cold]
    #[inline(never)]
    fn execute_normal_call(&mut self, func_reg: u16, argc: usize) -> Result<(), NuzoError> {
        let func_val = self.register(func_reg)?;

        if func_val.is_builtin_fn() {
            self.execute_normal_call_builtin(func_val, func_reg, argc)
        } else if func_val.is_closure() {
            self.execute_normal_call_closure(func_val, func_reg, argc)
        } else {
            Err(self.error_with_source_location(NuzoErrorExt::type_mismatch(
                "function",
                func_val.type_name(),
            )))
        }
    }

    #[cold]
    #[inline(never)]
    fn execute_normal_call_builtin(
        &mut self,
        func_val: Value,
        func_reg: u16,
        argc: usize,
    ) -> Result<(), NuzoError> {
        let (name, _arity, func) = func_val.as_builtin_fn_opt().ok_or_else(|| {
            err_compiler_bug(
                "builtin function info not found",
                Some(VmDiagnosis {
                    disassembly: "builtin function info not found".to_string(),
                    error_ip: Some(self.ip),
                    register_snapshot: vec![],
                    call_stack_depth: self.frame_depth(),
                    root_cause_analysis: format!(
                        "Internal error at IP {}: builtin function value exists but as_builtin_fn_opt() returned None",
                        self.ip
                    ),
                }),
            )
        })?;

        let result = self.call_builtin_unrolled(&func, func_reg, argc).map_err(|e| {
            self.error_with_source_location(NuzoErrorExt::type_mismatch(
                format!("valid arguments for builtin '{}'", name),
                format!("{}", e),
            ))
        })?;

        self.set_register(func_reg, result)
    }

    #[cold]
    #[inline(never)]
    fn execute_normal_call_closure(
        &mut self,
        func_val: Value,
        func_reg: u16,
        argc: usize,
    ) -> Result<(), NuzoError> {
        let closure_heap_obj = func_val.as_closure_heap_object_opt().ok_or_else(|| {
            err_compiler_bug(
                "closure heap object not found",
                Some(VmDiagnosis {
                    disassembly: "closure heap object not found".to_string(),
                    error_ip: Some(self.ip),
                    register_snapshot: vec![],
                    call_stack_depth: self.frame_depth(),
                    root_cause_analysis: format!(
                        "Internal error at IP {}: is_closure() is true but as_closure_heap_object_opt() returned None",
                        self.ip
                    ),
                }),
            )
        })?;

        let (prototype, arity) = match &*closure_heap_obj {
            HeapObject::Closure { prototype, .. } => {
                (Arc::clone(prototype), prototype.arity as usize)
            }
            _ => {
                return Err(err_compiler_bug(
                    "non-closure heap object",
                    Some(self.current_diagnosis(
                        "is_closure() returned true but heap object is not a Closure variant",
                    )),
                ));
            }
        };

        if argc != arity {
            return Err(self.error_with_source_location(NuzoErrorExt::arity_mismatch(arity, argc)));
        }

        let closure_for_frame = closure_heap_obj.clone();
        self.setup_closure_frame(&prototype, &closure_for_frame, func_reg, argc)?;

        let chunk = self.get_or_create_chunk(&prototype);
        self.activate_chunk_frame(&chunk, func_reg, argc, prototype.locals_count as usize);
        Ok(())
    }

    /// 共享的闭包帧建立逻辑（参数拷贝 + push_frame），减少 icache 占用。
    #[inline(always)]
    fn setup_closure_frame(
        &mut self,
        prototype: &Arc<FunctionPrototype>,
        closure_for_frame: &Arc<HeapObject>,
        func_reg: u16,
        argc: usize,
    ) -> Result<(), NuzoError> {
        let current_base = self.current_frame_base();
        let caller_func_reg_abs = current_base + func_reg as usize;
        let new_base = self.cx.registers.len();
        let needed = new_base + argc + prototype.locals_count as usize;
        if needed > self.max_stack_size {
            return Err(err_stack_overflow(needed, self.max_stack_size, false));
        }

        // 批量拷贝参数寄存器
        // 修复 BUG: src_start 必须加上 current_base，否则在嵌套调用（current_base > 0）时
        // 会从错误的寄存器位置读取参数（读到调用者帧之外的旧值，如闭包本身）。
        // 对应 tail_call.rs L195 的正确写法 `current_base + func_reg + 1`。
        let src_start = caller_func_reg_abs + 1;
        // SAFETY: 我们只读 src_start..src_start+argc，然后 push 到末尾，不重叠
        // Value: Copy，故每轮读取后立即结束不可变借用，push 可变借用不冲突
        for i in 0..argc {
            let val = self.cx.registers[src_start + i];
            self.cx.registers.push(val);
        }

        let return_address = self.ip;
        let caller_chunk = self.chunk.clone();
        self.push_frame_with_base(
            return_address,
            new_base,
            Some(Arc::clone(closure_for_frame)),
            caller_func_reg_abs,
            caller_chunk,
        )
    }

    /// 共享的 chunk 激活逻辑（frame_data 初始化 + 寄存器 resize + chunk 切换）。
    #[inline(always)]
    fn activate_chunk_frame(
        &mut self,
        chunk: &Arc<Chunk>,
        _func_reg: u16,
        argc: usize,
        locals_count: usize,
    ) {
        let new_base = {
            // new_base = registers.len() - argc - locals_count 之前的值
            // 但此时 registers 已经 push 了 argc 个参数，所以：
            self.cx.registers.len() - argc
        };

        // SCHF v6 Phase 4: spill 槽在 frame_data.data 中
        let new_n_cip = chunk.locals_count as usize + chunk.spill_slot_count as usize;
        let needed = new_base + new_n_cip;
        if self.cx.frame_data.data.len() < needed {
            self.cx.frame_data.data.resize(needed, NIL);
        }
        self.cx.frame_data.fill_nil(new_base, new_n_cip);
        self.cx.frame_data.top = new_base + new_n_cip;

        let resize_to = new_base + argc + locals_count;
        self.cx.registers.resize(resize_to, Value::default());
        self.cx.register_write_ptr = self.cx.registers.len();

        self.chunk = Some(Arc::clone(chunk));
        self.chunk_ptr = Arc::as_ptr(self.chunk.as_ref().unwrap());
        self.invalidate_cigc_cache();
        self.ip = 0;
    }

    #[inline(always)]
    pub(crate) fn execute_closure_fast(
        &mut self,
        snap: &ClosureSnapshot,
        func_reg: u16,
        argc: usize,
    ) -> Result<(), NuzoError> {
        let current_base = self.current_frame_base();
        let caller_func_reg_abs = current_base + func_reg as usize;
        let new_base = self.cx.registers.len();
        let needed = new_base + argc + snap.locals_count as usize;
        if needed > self.max_stack_size {
            return Err(err_stack_overflow(needed, self.max_stack_size, false));
        }

        // 批量拷贝参数
        // 修复 BUG: src_start 必须加上 current_base（同 setup_closure_frame L155 的修复），
        // 否则在嵌套调用（current_base > 0）时会从错误的寄存器位置读取参数。
        let src_start = caller_func_reg_abs + 1;
        for i in 0..argc {
            let val = self.cx.registers[src_start + i];
            self.cx.registers.push(val);
        }

        let return_address = self.ip;
        let caller_chunk = self.chunk.clone();

        let func_val = self.cx.registers[self.current_frame_base() + func_reg as usize];
        let closure_for_frame = func_val
            .as_closure_heap_object_opt()
            .ok_or_else(|| err_compiler_bug("CSTS fast path: closure heap object missing", None))?;

        self.push_frame_with_base(
            return_address,
            new_base,
            Some(closure_for_frame),
            caller_func_reg_abs,
            caller_chunk,
        )?;

        // SCHF v6 Phase 4: spill 槽
        let new_n_cip = snap.chunk.locals_count as usize + snap.chunk.spill_slot_count as usize;
        let needed = new_base + new_n_cip;
        if self.cx.frame_data.data.len() < needed {
            self.cx.frame_data.data.resize(needed, NIL);
        }
        self.cx.frame_data.fill_nil(new_base, new_n_cip);
        self.cx.frame_data.top = new_base + new_n_cip;

        let resize_to = new_base + argc + snap.locals_count as usize;
        self.cx.registers.resize(resize_to, Value::default());
        self.cx.register_write_ptr = self.cx.registers.len();

        self.chunk = Some(Arc::clone(&snap.chunk));
        self.chunk_ptr = Arc::as_ptr(self.chunk.as_ref().unwrap());
        self.invalidate_cigc_cache();
        self.ip = 0;

        Ok(())
    }

    #[inline]
    fn populate_csts_snapshot(&mut self, call_ip: usize) {
        // 合并所有前置检查为单次读取，减少分支
        let proto = {
            let site = self.cx.call_sites.get(call_ip);
            // 快速拒绝：非 Monomorphic / 非 Closure / 已有 snapshot
            if site.state != CallSiteState::Monomorphic
                || site.mono.target_type != CallTargetType::Closure
                || site.closure_snapshot.is_some()
            {
                return;
            }
            match &site.mono.cached_prototype {
                Some(p) => Arc::clone(p),
                None => return,
            }
        };

        let chunk = self.get_or_create_chunk(&proto);

        let snap = ClosureSnapshot::new(Arc::clone(&chunk), proto.arity, proto.locals_count);
        self.cx.call_sites.get_mut(call_ip).closure_snapshot = Some(snap.clone());

        // 🧬 CDD：为这个调用点创建专门的快速调用闭包
        self.closure_invokers.entry(call_ip).or_insert_with(|| {
            let invoker: ClosureInvoker =
                Box::new(move |vm: &mut VM, func_reg: u16, argc: usize| {
                    vm.execute_closure_fast(&snap, func_reg, argc)
                });
            invoker
        });

        self.cx.hot_trace_table.invalidate_fused_cache_for_csts(call_ip);
    }

    // ========================================================================
    // op_call: 子函数拆分
    // ========================================================================

    /// Builtin L1 单态缓存快速路径
    ///
    /// 返回 `Some(result)` 表示快速路径已处理（caller 应立即 return）；
    /// 返回 `None` 表示未命中，caller 应继续 fallback。
    #[inline(always)]
    fn try_builtin_l1_fast_path(
        &mut self,
        call_site_ip: usize,
        func_reg: u16,
        argc: usize,
        func_val: Value,
    ) -> Option<Result<(), NuzoError>> {
        if call_site_ip >= self.cx.call_sites.len() {
            return None;
        }
        let site = self.cx.call_sites.get(call_site_ip);
        // 单次比较合并：state + target_type
        if site.state != CallSiteState::Monomorphic
            || site.mono.target_type != CallTargetType::Builtin
        {
            return None;
        }
        let raw_bits = site.mono.raw_value_bits;
        // 快速 identity 检查（最可能的 miss 原因）
        if raw_bits == 0 || raw_bits != func_val.into_raw_bits() {
            return None;
        }

        let fn_ptr = site.mono.cached_builtin_fn;
        let arity = site.mono.cached_builtin_arity;

        // 统计更新（合并为单次 get_mut）
        {
            let site_mut = self.cx.call_sites.get_mut(call_site_ip);
            site_mut.stats.l1_hits += 1;
            site_mut.stats.total_calls += 1;
        }

        if let (Some(fn_ptr), Some(arity)) = (fn_ptr, arity)
            && arity as usize == argc
        {
            return match self.execute_builtin_fast(fn_ptr, func_reg, argc) {
                Ok(_) => Some(Ok(())),
                Err(e) => Some(Err(self.error_with_source_location(e))),
            };
        }
        None
    }

    /// CDD (Closure Direct Dispatch) 闭包直接调用快速路径
    ///
    /// 返回 `Some(result)` 表示快速路径已处理（caller 应立即 return）；
    /// 返回 `None` 表示未命中，caller 应继续 fallback。
    #[inline(always)]
    fn try_cdd_fast_path(
        &mut self,
        call_site_ip: usize,
        func_reg: u16,
        argc: usize,
        is_tail: bool,
        func_val: Value,
    ) -> Option<Result<(), NuzoError>> {
        if is_tail {
            return None;
        }
        if call_site_ip >= self.cx.call_sites.len() {
            return None;
        }

        // 先做 identity 检查，避免不必要的 HashMap 操作
        let (_raw_bits, snap_arity) = {
            let site = self.cx.call_sites.get(call_site_ip);
            if site.state != CallSiteState::Monomorphic
                || site.mono.target_type != CallTargetType::Closure
            {
                return None;
            }
            let rb = site.mono.raw_value_bits;
            if rb == 0 || rb != func_val.into_raw_bits() {
                return None;
            }
            let sa = site.closure_snapshot.as_ref()?.arity as usize;
            (rb, sa)
        };

        if snap_arity != argc {
            return None;
        }

        // 通过 raw pointer 避免 HashMap remove/insert 的双重哈希开销
        // SAFETY: invoker 在调用期间不会被移除（单线程 VM 执行）
        let invoker_ptr = self.closure_invokers.get(&call_site_ip)? as *const ClosureInvoker;

        // 统计更新
        {
            let site_mut = self.cx.call_sites.get_mut(call_site_ip);
            site_mut.stats.l1_hits += 1;
            site_mut.stats.total_calls += 1;
        }

        // SAFETY: VM 是单线程执行，invoker 生命周期覆盖整个调用
        let invoker = unsafe { &*invoker_ptr };
        let result = invoker(self, func_reg, argc);
        Some(result)
    }

    /// 调用成功后更新调用点缓存（monomorphic state、CSTS snapshot 等）
    #[inline]
    fn update_call_site_after_call(
        &mut self,
        call_site_ip: usize,
        func_reg: u16,
        is_tail: bool,
        result: &Result<(), NuzoError>,
    ) {
        // 快速拒绝：失败或尾调用不需要更新
        if result.is_err() || is_tail {
            return;
        }

        if call_site_ip >= self.cx.call_sites.len() {
            return;
        }

        // 单次读取判断是否需要跳过
        let site = self.cx.call_sites.get(call_site_ip);
        if site.state == CallSiteState::Monomorphic
            && site.mono.target_type == CallTargetType::Closure
        {
            return; // 已经是 monomorphic closure，无需更新
        }

        if let Ok(func_val) = self.register(func_reg) {
            self.update_call_site_cache(call_site_ip, func_val);
        }

        self.populate_csts_snapshot(call_site_ip);
    }

    pub(in crate::vm) fn op_call(&mut self) -> Result<(), NuzoError> {
        let call_site_ip = self.ip;
        let func_reg = self.read_u16()?;
        let argc = self.read_byte()? as usize;
        let next_instr_ip = self.ip;

        // 一次性读取 func_val，后续所有路径复用
        let func_val = self.register(func_reg)?;

        // is_tail 计算（消除重复分支）
        let is_tail = if call_site_ip < self.cx.call_sites.len() {
            let site = self.cx.call_sites.get(call_site_ip);
            match site.cached_is_tail {
                Some(v) => v,
                None => {
                    let result = self.is_tail_position(next_instr_ip);
                    self.cx.call_sites.ensure(call_site_ip).cached_is_tail = Some(result);
                    result
                }
            }
        } else {
            let result = self.is_tail_position(next_instr_ip);
            self.cx.call_sites.ensure(call_site_ip).cached_is_tail = Some(result);
            result
        };

        // Builtin L1 Fast Path（传入已读取的 func_val，避免重复 register 访问）
        if let Some(result) = self.try_builtin_l1_fast_path(call_site_ip, func_reg, argc, func_val)
        {
            return result;
        }

        // 🧬 CDD 极速路径：闭包直接调用
        if let Some(result) =
            self.try_cdd_fast_path(call_site_ip, func_reg, argc, is_tail, func_val)
        {
            return result;
        }

        // Slow path: 普通调用或尾调用
        let result = if is_tail {
            self.execute_tail_call(func_reg, argc as u8)
        } else {
            self.execute_normal_call(func_reg, argc)
        };

        self.update_call_site_after_call(call_site_ip, func_reg, is_tail, &result);
        result
    }

    #[inline(always)]
    pub(in crate::vm) fn op_return(&mut self) -> Result<(), NuzoError> {
        let val_reg = self.read_u16()?;
        let return_value = self.register(val_reg)?;

        // SCHF v6 Phase 3：caller_func_reg 改读 frame_metas.last()
        if let Some(meta) = self.current_meta() {
            let caller_reg = meta.caller_func_reg;
            let regs_len = self.cx.registers.len();
            if caller_reg < regs_len {
                // 直接写入，避免 set_register 的额外检查
                self.cx.registers.set(caller_reg, return_value);
            } else {
                return Err(NuzoError::internal(
                    InternalError::RegisterOutOfBounds {
                        reg: caller_reg as u16,
                        available: regs_len,
                    },
                    Some(self.current_diagnosis(&format!(
                        "Return: caller_func_reg {} exceeds register file size {}",
                        caller_reg, regs_len
                    ))),
                ));
            }
        }
        self.pop_frame()
    }

    #[inline(always)]
    pub(in crate::vm) fn op_halt(&mut self) -> Result<(), NuzoError> {
        self.cx.running = false;
        Ok(())
    }
}
