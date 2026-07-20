//! # VM 派发辅助方法
//!
//! 包含 VM 在派发路径中复用的辅助方法：
//! - `get_constant_string` — 从常量池读取字符串
//! - 模块加载（lazy import）：`get_module_path_from_constant` / `get_module_chunk` / `execute_module_toplevel`
//! - `try_alloc_arena` — Arena 快速路径分配
//! - `validate_jump_target` — 跳转目标校验
//! - `call_builtin_unrolled` — Builtin 函数调用展开

use crate::vm::VM;
use nuzo_bytecode::Chunk;
use nuzo_core::DIAGNOSTIC_REGISTER_WINDOW;
use nuzo_values::{HeapObject, InternalError, NIL, NuzoError, Value, ValueExt, VmDiagnosis};
use std::sync::Arc;

use super::cold_path::{err_compiler_bug, err_const_out_of_bounds, err_stack_overflow};

impl VM {
    #[inline(always)]
    pub(super) fn get_constant_string(&mut self, const_idx: usize) -> Result<String, NuzoError> {
        let chunk = self.current_chunk()?;
        let ip = self.ip;
        let frames_len = self.frame_depth();
        let reg_snapshot: Vec<(u16, String)> = {
            let start = self.cx.registers.len().saturating_sub(DIAGNOSTIC_REGISTER_WINDOW);
            self.cx
                .registers
                .as_slice()
                .iter()
                .enumerate()
                .skip(start)
                .map(|(i, v)| (i as u16, format!("{}", v)))
                .collect()
        };
        let val = chunk.constants().get(const_idx).ok_or_else(|| {
            self.error_with_source_location(err_const_out_of_bounds(
                const_idx,
                chunk.constants().len(),
                self.ip,
                None,
            ))
        })?;
        val.as_string_opt().ok_or_else(|| {
            let diag = VmDiagnosis {
                disassembly: "Constant is not a string".to_string(),
                error_ip: Some(ip),
                register_snapshot: reg_snapshot,
                call_stack_depth: frames_len,
                root_cause_analysis: format!(
                    "Internal error at IP {}: Constant is not a string",
                    ip
                ),
            };
            err_compiler_bug("Constant is not a string", Some(diag))
        })
    }

    // ========================================================================
    // Lazy Import: OP_INIT_MODULE 辅助方法
    // ========================================================================
    //
    // 设计原则：
    // - 不在 VM 内重新编译模块（避免循环依赖与编译器状态污染）
    // - 模块 Chunk 由 Engine 在编译期预编译后通过 register_module 注入
    // - 模块顶层代码通过帧切换（push_frame_with_base）执行，run_inner 主循环自然续行
    // - 模块的 OP_RETURN 触发 pop_frame，自动恢复 caller 的 chunk 与 IP

    /// 从常量池取出模块路径字符串。
    ///
    /// `module_idx` 是 `OP_INIT_MODULE` 操作数指向的常量池索引，
    /// 期望对应 `Value::String`。
    pub(in crate::vm) fn get_module_path_from_constant(
        &mut self,
        const_idx: usize,
    ) -> Result<String, NuzoError> {
        self.get_constant_string(const_idx)
    }

    /// 从 `module_cache` 取已编译的模块 Chunk。
    ///
    /// 未找到时返回 `InternalError::ModuleNotLoaded { path }`，提示
    /// Engine 未在 `VM::run` 前调用 `register_module` 注入对应模块。
    pub(in crate::vm) fn get_module_chunk(&self, path: &str) -> Result<Arc<Chunk>, NuzoError> {
        self.cx.module_cache.get(path)
            .cloned()
            .ok_or_else(|| {
                NuzoError::internal(
                    InternalError::ModuleNotLoaded { path: path.to_string() },
                    Some(VmDiagnosis {
                        disassembly: format!(
                            "OP_INIT_MODULE: module '{}' not registered in module_cache (Engine should call VM::register_module before VM::run)",
                            path
                        ),
                        error_ip: Some(self.ip),
                        register_snapshot: Vec::new(),
                        call_stack_depth: self.frame_depth(),
                        root_cause_analysis: format!(
                            "lazy import referenced module '{}' but module_cache has no entry; ensure Engine populated VM.module_cache before run",
                            path
                        ),
                    }),
                )
            })
    }

    /// 通过帧切换执行模块顶层 Chunk。
    ///
    /// 实现策略：
    /// 1. 保存当前 IP（即 `OP_INIT_MODULE` 后下一条指令位置）作为返回地址
    /// 2. 保存当前 chunk 作为 caller_chunk（pop_frame 时恢复）
    /// 3. `push_frame_with_base` 推入新帧，无闭包（模块顶层非闭包）
    /// 4. 切换 chunk 至模块 Chunk，IP 重置为 0
    /// 5. run_inner 主循环自然接管，执行模块顶层代码
    /// 6. 模块 `OP_RETURN` → pop_frame 自动恢复 caller 状态
    ///
    /// 注意：此处不复用 `run_chunk`/`run_inner` 递归调用，遵守
    /// "run_inner 不可重入" 的不变量。
    pub(in crate::vm) fn execute_module_toplevel(
        &mut self,
        chunk: Arc<Chunk>,
    ) -> Result<(), NuzoError> {
        let return_address = self.ip;
        let caller_chunk = self.chunk.clone();
        let new_base = self.cx.registers.len();

        // 模块顶层无参数（argc=0），仅预留模块自身 locals
        let needed = new_base + chunk.locals_count as usize;
        if needed > self.max_stack_size {
            return Err(err_stack_overflow(needed, self.max_stack_size, false));
        }

        // 推入新帧：return_address 为 caller 的下一条指令，caller_chunk 为恢复目标
        // closure=None（模块顶层不是闭包），caller_func_reg=0（不参与 TCO 复用）
        self.push_frame_with_base(return_address, new_base, None, 0, caller_chunk)?;

        // SCHF v6 Phase 4: spill 槽在 frame_data.data 中（VecDeque 已移除）。
        // push_frame_with_base 用 self.chunk（caller chunk）初始化 n_cip，
        // 此处按新模块 chunk 的 n_cip 修正 frame_data（locals + spill_slot）。
        let new_n_cip = chunk.locals_count as usize + chunk.spill_slot_count as usize;
        let needed = new_base + new_n_cip;
        if self.cx.frame_data.data.len() < needed {
            self.cx.frame_data.data.resize(needed, NIL);
        }
        self.cx.frame_data.fill_nil(new_base, new_n_cip);
        self.cx.frame_data.top = new_base + new_n_cip;

        // 预留模块 locals 寄存器（与 execute_closure_fast 一致的 padding 策略）
        let resize_to = new_base + chunk.locals_count as usize;
        self.cx.registers.resize(resize_to, Value::default());
        self.cx.register_write_ptr = self.cx.registers.len();

        // 切换到模块 chunk：同步 chunk (Arc) 与 chunk_ptr (raw) 必须保持一致
        self.chunk_ptr = Arc::as_ptr(&chunk);
        self.chunk = Some(chunk);
        self.invalidate_cigc_cache();
        self.ip = 0;
        Ok(())
    }

    // ========================================================================
    // Arena 快速路径辅助方法
    // ========================================================================

    /// 尝试通过 Arena 区域分配器分配堆对象。
    ///
    /// # 返回值
    /// - `Ok(Value)` — Arena 分配成功，返回用 arena offset 编码的 Value（首版策略：
    ///   对象仍通过 GC 实际存储，但 Value 标记为 arena 来源，用于验证完整数据流）
    /// - `Err(HeapObject)` — Arena 已满/禁用，调用方应将返回的 obj 降级到 gc.alloc_scratch()
    ///
    /// # 首版语义说明
    /// 真正的零拷贝 Arena 原地存储需要：序列化写入 arena.data → 通过 offset 反序列化读取 →
    /// 修改全部 get_box()/as_heap_object_opt() 支持 arena 索引。首版先验证框架正确性：
    /// bump 分配流程 / arena 编码解码 / pop_frame O(1) 释放。
    /// v2: 零拷贝路径 — 对象直接存入 region.objects，不再经过 GC alloc_scratch。
    #[inline(always)]
    pub(super) fn try_alloc_arena(&mut self, obj: HeapObject) -> Result<Value, HeapObject> {
        // SCHF v6 Phase 3：arena 索引改读 frame_metas.last()（spec 4.4 + 6.2）。
        // 旧路径 `frames.back_mut().arena` 已废弃；arena 与帧栈独立，仅需读 arena 索引。
        let arena = match self.current_meta() {
            Some(m) => m.arena,
            None => return Err(obj),
        };

        // 计算对象所需内存大小
        let size = std::mem::size_of_val(&obj) + obj.size_estimate();

        // v2: 使用 allocate_object() 零拷贝存储（不再调用 gc.alloc_scratch）
        self.cx.region.allocate_object(arena, obj, size)
    }

    #[inline(always)]
    pub(super) fn validate_jump_target(
        &mut self,
        offset: i16,
        allow_equal: bool,
    ) -> Result<usize, NuzoError> {
        let raw = self.ip as i64 + offset as i64;
        let chunk = self.current_chunk()?;
        let code_len = chunk.code().len();
        if raw < 0 {
            return Err(NuzoError::internal(
                InternalError::JumpTargetOutOfBounds { target: self.ip, code_len },
                Some(self.current_diagnosis(&format!(
                    "Negative jump target from IP {} with offset {}",
                    self.ip, offset
                ))),
            ));
        }
        let new_ip = raw as usize;
        let out_of_bounds = if allow_equal { new_ip >= code_len } else { new_ip > code_len };
        if out_of_bounds {
            return Err(NuzoError::internal(
                InternalError::JumpTargetOutOfBounds { target: new_ip, code_len },
                Some(self.current_diagnosis(&format!(
                    "Jump target {} exceeds code length {}",
                    new_ip, code_len
                ))),
            ));
        }
        Ok(new_ip)
    }

    #[inline(always)]
    pub(super) fn call_builtin_unrolled(
        &mut self,
        func: &dyn Fn(&[Value]) -> Result<Value, NuzoError>,
        func_reg: u16,
        argc: usize,
    ) -> Result<Value, NuzoError> {
        match argc {
            0 => func(&[]),
            1 => {
                let a0 = self.register(func_reg + 1)?;
                func(&[a0])
            }
            2 => {
                let a0 = self.register(func_reg + 1)?;
                let a1 = self.register(func_reg + 2)?;
                func(&[a0, a1])
            }
            3 => {
                let a0 = self.register(func_reg + 1)?;
                let a1 = self.register(func_reg + 2)?;
                let a2 = self.register(func_reg + 3)?;
                func(&[a0, a1, a2])
            }
            _ => {
                const MAX_STACK_ARGS: usize = 16;
                // Value::default() == NIL (const u64), 零成本初始化，消除 MaybeUninit::uninit().assume_init() 的 UB
                let mut args_buf: [Value; MAX_STACK_ARGS] = [Value::default(); MAX_STACK_ARGS];
                let mut heap_args;

                // 边界检查：func_reg + 1 + argc 必须落在 u16 寄存器域内，
                // 否则 `func_reg + 1 + i as u16` 在 u16 域溢出（debug panic / release wrap）。
                let last_reg = func_reg as usize + 1 + argc;
                if last_reg > u16::MAX as usize {
                    return Err(NuzoError::internal(
                        InternalError::RegisterOverflow { count: last_reg },
                        None,
                    ));
                }
                let args_slice: &[Value] = if argc <= MAX_STACK_ARGS {
                    for (i, slot) in args_buf.iter_mut().enumerate().take(argc) {
                        *slot = self.register((func_reg as usize + 1 + i) as u16)?;
                    }
                    &args_buf[..argc]
                } else {
                    heap_args = Vec::with_capacity(argc);
                    for i in 0..argc {
                        heap_args.push(self.register((func_reg as usize + 1 + i) as u16)?);
                    }
                    &heap_args
                };
                func(args_slice)
            }
        }
    }
}
