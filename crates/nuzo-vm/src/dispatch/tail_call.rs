//! # 尾调用优化 (TCO) 实现
//!
//! 包含 `execute_tail_call` 及其拆分子函数：
//! - `try_builtin_tail_call` — Builtin 函数尾调用快速路径
//! - `resolve_tco_prototype` — 解析闭包 prototype + 自递归检测 + arity 校验
//! - `apply_tco_reuse` — 参数重定位 + 寄存器调整 + 帧元数据更新 + chunk 切换 + IP 重置
//!
//! ## TCO 流程
//! 1. Builtin fast-path：直接计算结果，写入 caller_func_reg，pop_frame 返回
//! 2. 闭包路径：检测自递归（同 prototype 指针），复用当前帧
//! 3. 参数从 [func_reg+1, func_reg+1+argc) 复制到 [current_base, current_base+argc)
//! 4. 调整寄存器文件大小（new_locals_needed vs old_locals_count）
//! 5. 更新 FrameMeta：tco_history、closure（非自递归时替换）、call_site、tco_reused
//! 6. 自递归跳过 chunk 查找 + IC 失效；非自递归需要切换 chunk
//! 7. 重置 IP 为 0

use crate::vm::VM;
use nuzo_abi::NuzoErrorExt;
use nuzo_bytecode::Opcode;
use nuzo_values::*;
use std::sync::Arc;

use super::cold_path::{err_compiler_bug, err_stack_overflow};

impl VM {
    pub(super) fn execute_tail_call(&mut self, func_reg: u16, argc: u8) -> Result<(), NuzoError> {
        let current_base = self.current_frame_base();
        let argc = argc as usize;
        let func_val = self.register(func_reg)?;

        // --- builtin 尾调用 (不变) ---
        if func_val.is_builtin_fn() {
            return self.try_builtin_tail_call(func_val, func_reg, argc);
        }

        let closure = func_val.as_closure_heap_object_opt().ok_or_else(|| {
            self.error_with_source_location(NuzoErrorExt::type_mismatch(
                "function",
                func_val.type_name(),
            ))
        })?;

        // TCO 复用当前帧，调用点应更新为本次尾调用的位置
        let caller_chunk = self.chunk.clone();
        let tail_call_site_ip = self.ip.saturating_sub(Opcode::Call.instruction_size());
        let tail_call_site =
            caller_chunk.as_ref().and_then(|c| c.get_source_location(tail_call_site_ip));

        // --- 解析 prototype + 自递归检测 + arity 校验 ---
        let (prototype, is_self_recursive) = self.resolve_tco_prototype(&closure, argc)?;

        // --- 参数重定位 + 寄存器调整 + 帧更新 + chunk 切换 + IP 重置 ---
        self.apply_tco_reuse(
            current_base,
            func_reg,
            argc,
            &prototype,
            is_self_recursive,
            closure,
            tail_call_site,
            caller_chunk,
        )
    }

    /// Builtin 尾调用快速路径
    ///
    /// 成功时返回 `Ok(())`（已 pop_frame 并写入 caller_func_reg）；
    /// 调用方应在调用后立即 return。
    fn try_builtin_tail_call(
        &mut self,
        func_val: Value,
        func_reg: u16,
        argc: usize,
    ) -> Result<(), NuzoError> {
        let ip = self.ip;
        let frames_len = self.frame_depth();
        let (name, _arity, func) = func_val.as_builtin_fn_opt()
            .ok_or_else(|| err_compiler_bug("builtin function info not found", Some(VmDiagnosis {
                disassembly: "builtin function info not found".to_string(),
                error_ip: Some(ip),
                register_snapshot: vec![],
                call_stack_depth: frames_len,
                root_cause_analysis: format!("Internal error at IP {}: TCO builtin function value exists but as_builtin_fn_opt() returned None", ip),
            })))?;

        let result = self.call_builtin_unrolled(&func, func_reg, argc).map_err(|e| {
            self.error_with_source_location(NuzoErrorExt::type_mismatch(
                format!("valid arguments for builtin '{}'", name),
                format!("{}", e),
            ))
        })?;

        // SCHF v6 Phase 3：caller_func_reg 改读 frame_metas.last()（spec 4.4 + 2.4）。
        // frame_metas 与 frames 一一对应，last() 始终是当前帧。
        if let Some(meta) = self.current_meta()
            && meta.caller_func_reg < self.cx.registers.len()
        {
            self.cx.registers.set(meta.caller_func_reg, result);
        }
        self.pop_frame()?;
        Ok(())
    }

    /// 解析 TCO 闭包 prototype + 自递归检测 + arity 校验
    ///
    /// 返回 `(prototype, is_self_recursive)`：
    /// - 自递归时从帧闭包取 prototype（避免 clone 新闭包的 HeapObject）
    /// - 非自递归时从传入 closure 取 prototype，并校验 argc == arity
    fn resolve_tco_prototype(
        &mut self,
        closure: &Arc<HeapObject>,
        argc: usize,
    ) -> Result<(Arc<FunctionPrototype>, bool), NuzoError> {
        // --- 自递归 fast-path: 提前检测是否调用自身 ---
        // as_closure_heap_object_opt() 对 GC 对象返回 Arc::new(clone()),
        // Arc::ptr_eq 和 Arc::as_ptr 都不可靠。改用 prototype 指针比较:
        // 同一个函数的闭包共享同一个 Arc<FunctionPrototype>, prototype 指针唯一。
        let new_prototype_ptr = match &**closure {
            HeapObject::Closure { prototype, .. } => Arc::as_ptr(prototype) as usize,
            _ => 0,
        };
        let is_self_recursive = if new_prototype_ptr != 0 {
            // SCHF v6 Phase 3：closure 改读 frame_metas.last()（spec 4.4 + 2.4）。
            self.current_meta()
                .and_then(|m| m.closure.as_ref())
                .and_then(|c| match &**c {
                    HeapObject::Closure { prototype, .. } => Some(Arc::as_ptr(prototype) as usize),
                    _ => None,
                })
                .map(|old_ptr| old_ptr == new_prototype_ptr)
                .unwrap_or(false)
        } else {
            false
        };

        // 自递归时从帧闭包取 prototype（避免 clone 新闭包的 HeapObject）
        let prototype = if is_self_recursive {
            // SCHF v6 Phase 3：closure 改读 frame_metas.last()。
            self.current_meta()
                .and_then(|m| m.closure.as_ref())
                .and_then(|c| match &**c {
                    HeapObject::Closure { prototype, .. } => Some(prototype.clone()),
                    _ => None,
                })
                .ok_or_else(|| {
                    err_compiler_bug(
                        "self-recursive TCO: frame closure missing",
                        Some(
                            self.current_diagnosis(
                                "is_self_recursive=true but frame has no closure",
                            ),
                        ),
                    )
                })?
        } else {
            let p = match &**closure {
                HeapObject::Closure { prototype, .. } => prototype.clone(),
                _ => return Err(err_compiler_bug("non-closure in execute_tail_call", Some(self.current_diagnosis(
                    "TCO: as_closure_heap_object_opt() succeeded but heap object is not a Closure variant"
                )))),
            };
            if argc != p.arity as usize {
                return Err(self.error_with_source_location(NuzoErrorExt::arity_mismatch(
                    p.arity as usize,
                    argc,
                )));
            }
            p
        };

        if is_self_recursive && argc != prototype.arity as usize {
            return Err(self.error_with_source_location(NuzoErrorExt::arity_mismatch(
                prototype.arity as usize,
                argc,
            )));
        }

        Ok((prototype, is_self_recursive))
    }

    /// 应用 TCO 帧复用：参数重定位 + 寄存器调整 + 帧元数据更新 + chunk 切换 + IP 重置
    #[allow(clippy::too_many_arguments)]
    fn apply_tco_reuse(
        &mut self,
        current_base: usize,
        func_reg: u16,
        argc: usize,
        prototype: &Arc<FunctionPrototype>,
        is_self_recursive: bool,
        closure: Arc<HeapObject>,
        tail_call_site: Option<SourceLocation>,
        caller_chunk: Option<Arc<nuzo_bytecode::Chunk>>,
    ) -> Result<(), NuzoError> {
        // --- 参数重定位 (所有路径都需要) ---
        let args_src_start = current_base + func_reg as usize + 1;
        let args_src_end = args_src_start + argc;

        if args_src_end > self.cx.registers.len() {
            return Err(NuzoError::internal(
                InternalError::RegisterOutOfBounds {
                    reg: (args_src_end - 1) as u16,
                    available: self.cx.registers.len(),
                },
                Some(self.current_diagnosis(&format!(
                    "TCO: register range [{}, {}) exceeds available registers ({})",
                    args_src_start,
                    args_src_end,
                    self.cx.registers.len()
                ))),
            ));
        }

        self.cx.registers.copy_within(args_src_start..args_src_end, current_base);
        self.cx.register_write_ptr = current_base + argc;

        // --- 寄存器调整 (所有路径都需要) ---
        let new_locals_needed = prototype.locals_count as usize;
        let old_locals_count =
            self.cx.registers.len().saturating_sub(current_base).saturating_sub(argc);

        if new_locals_needed > old_locals_count {
            let total_needed = current_base + argc + new_locals_needed;
            if total_needed > self.max_stack_size {
                return Err(err_stack_overflow(total_needed, self.max_stack_size, true));
            }
            self.cx.registers.resize(total_needed, Value::default());
        }

        self.cx.register_write_ptr = current_base + argc + new_locals_needed;

        // --- 帧更新 ---
        let should_record_tco = self.is_diagnostic_mode() || self.is_tracer_active();

        // SCHF v6 Phase 4：TCO 复用仅写 FrameMeta（VecDeque 已移除）。
        // pop_frame 从 v6_info（return_address/base）与 v6_meta（caller_chunk/arena/closure）读取，
        // GC 遍历从 v6_meta.tco_history 读取。tco_reused/tco_history 按 spec 7.2 从本字段读写。
        // 先克隆所需外部数据，避免 current_meta_mut() 的可变借用与 self.chunk 冲突。
        let chunk_clone_for_meta = if should_record_tco { Some(caller_chunk) } else { None };
        if let Some(meta) = self.current_meta_mut() {
            if let Some(current_chunk) = chunk_clone_for_meta {
                // SCHF v6 Phase 4: tco_history 仅在 FrameMeta 中维护（VecDeque 已移除）。
                // replaced_ip/replaced_return_address 置 0（FrameMeta 不存 ip/return_address，
                // GC 仅 trace replaced_closure，诊断路径不依赖这两个字段）。
                meta.tco_history.push(super::super::TcoRecord {
                    replaced_closure: meta.closure.clone(),
                    replaced_ip: 0,             // meta 不存 ip，TCO 复用必置 0
                    replaced_return_address: 0, // meta 不存 return_address
                    replaced_chunk: current_chunk,
                });
            }
            if !is_self_recursive {
                meta.closure = Some(closure);
            }
            meta.call_site = tail_call_site;
            meta.tco_reused = true;
        }

        // --- 自递归 fast-path: 跳过 chunk 查找 + IC 失效 ---
        if !is_self_recursive {
            let chunk = self.get_or_create_chunk(prototype);
            self.chunk = Some(chunk);
            self.chunk_ptr = Arc::as_ptr(self.chunk.as_ref().expect("dispatch: chunk not loaded"));
            self.invalidate_cigc_cache();
        }
        // 自递归: chunk/cigc_cache 未变，直接重置 IP 即可

        self.ip = 0;
        Ok(())
    }
}
