//! # LSRA 线性扫描寄存器分配集成（LSRA Integration）
//!
//! 提供 LSRA（Linear Scan Register Allocation）算法与编译器的集成接口。
//! 包括 def/use 信息收集、分配执行、结果查询和字节码重写。
//!
//! ## 使用流程
//!
//! ```text
//! 1. Builder(with_lsra=true) → 启用收集模式
//! 2. compile_program() → 自动收集 def/use 点
//! 3. try_lsra_allocate()? → 执行 LSRA 算法
//! 4. lsra_mapping() / lsra_phys_reg(vreg) → 查询结果
//! 5. rewrite_registers_with_lsra()? → 可选：重写字节码
//! ```

use crate::allocator::{LsraAllocator, build_intervals, enhance_intervals};
use crate::compiler::{CompileError, Compiler};
use nuzo_bytecode::{Opcode, OperandKind};
use nuzo_core::MAX_FUNCTION_LOCALS;

impl Compiler {
    // ========================================================================
    // LSRA 信息收集（LSRA Information Collection）
    // ========================================================================

    /// 获取当前字节码位置（Instruction Pointer）
    ///
    /// 返回 `chunk.code()` 的当前长度，即下一条将要发射的指令的 IP。
    /// 用于 def/use 信息收集时记录精确的位置标记。
    #[inline]
    pub(crate) fn current_ip(&self) -> usize {
        self.chunk.code().len()
    }

    /// 记录虚拟寄存器的定义点（Definition Point）
    ///
    /// 当 `use_lsra == true` 时，将当前 IP 记录为指定 vreg 的首次定义点。
    /// **只记录一次**：如果该 vreg 已有 def 点，则不覆盖。
    #[inline]
    pub(crate) fn note_vreg_def(&mut self, reg: u16) {
        if !self.use_lsra {
            return;
        }
        let idx = reg as usize;
        if idx < self.def_ips.len() && self.def_ips[idx].is_none() {
            self.def_ips[idx] = Some(self.current_ip());
            self.loop_depths[idx] = self.loop_depth;
        }
    }

    /// 记录虚拟寄存器的使用点（Use Point）
    ///
    /// 当 `use_lsra == true` 时，将当前 IP 记录为指定 vreg 的最后使用点。
    /// **每次调用都更新**：同一 vreg 可能被多次使用，始终保留最新的 IP。
    #[inline]
    pub(crate) fn note_vreg_use(&mut self, reg: u16) {
        if !self.use_lsra {
            return;
        }
        let idx = reg as usize;
        if idx < self.use_ips.len() {
            self.use_ips[idx] = Some(self.current_ip());
        }
    }

    // ========================================================================
    // LSRA 分配执行（LSRA Allocation Execution）
    // ========================================================================

    /// 运行 LSRA 分配算法
    ///
    /// 这是 LSRA 对接的主入口方法。在编译流程结束后调用。
    ///
    /// # 算法流程
    ///
    /// 1. 前置检查：确认 use_lsra 已启用且编译已产生字节码
    /// 2. 构建活区间列表：build_intervals(def_ips, use_ips)
    /// 3. （可选）NUD 增强：enhance_intervals()
    /// 4. 创建 LsraAllocator 并执行线性扫描分配
    /// 5. 提取 vreg_to_preg 映射到 lsra_result
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let mut compiler = Compiler::builder()
    ///     .source("let x = 1 + 2")
    ///     .with_lsra(true)
    ///     .build();
    /// compiler.compile_program(&program)?;
    /// compiler.try_lsra_allocate()?;
    /// ```
    pub fn try_lsra_allocate(&mut self) -> Result<(), CompileError> {
        if !self.use_lsra {
            return Err(CompileError::Error {
                message: "LSRA 未启用：请通过 CompilerBuilder::with_lsra(true) 启用".to_string(),
                line: 0,
                column: 0,
            });
        }

        let mut intervals = build_intervals(&self.def_ips, &self.use_ips).map_err(|e| {
            CompileError::Error {
                message: format!("LSRA 区间构建失败: {}", e), line: 0, column: 0
            }
        })?;

        if intervals.is_empty() {
            self.lsra_result = Some([None; MAX_FUNCTION_LOCALS as usize]);
            return Ok(());
        }

        if self.nud_config.enabled {
            enhance_intervals(&mut intervals, &self.nud_config, &self.loop_depths);
        }

        let mut lsra = if self.nud_config.enabled {
            LsraAllocator::with_nud_config(self.nud_config)
        } else {
            LsraAllocator::with_max_locals()
        };

        lsra.allocate(&mut intervals).map_err(|e| CompileError::Error {
            message: format!("LSRA 分配失败: {}", e),
            line: 0,
            column: 0,
        })?;

        let mut mapping = [None; MAX_FUNCTION_LOCALS as usize];
        for iv in &intervals {
            if (iv.vreg as usize) < mapping.len() {
                mapping[iv.vreg as usize] = iv.reg;
            }
        }

        for vreg in 0..MAX_FUNCTION_LOCALS {
            mapping[vreg as usize] = lsra.get_phys_reg(vreg);
        }

        self.lsra_result = Some(mapping);
        Ok(())
    }

    // ========================================================================
    // LSRA 结果应用（LSRA Result Application）
    // ========================================================================

    /// 使用 LSRA 分配结果重写字节码中的寄存器操作数（Post-Rewrite Pass）
    ///
    /// 将虚拟寄存器编号（vreg）替换为 LSRA 计算出的物理寄存器编号（preg）。
    ///
    /// # 前置条件
    ///
    /// - `try_lsra_allocate()` 必须已成功执行（`lsra_result` 为 `Some`）
    #[allow(dead_code)] // LSRA 重写 API，保留供 LSRA 后端集成使用
    pub(crate) fn rewrite_registers_with_lsra(&mut self) -> Result<(), CompileError> {
        let mapping = self.lsra_result.as_ref().ok_or_else(|| CompileError::Error {
            message: "LSRA 未运行：请先调用 try_lsra_allocate()".to_string(),
            line: 0,
            column: 0,
        })?;

        let code = self.chunk.code_mut();
        let mut ip = 0;
        let mut lsra_peak: u16 = 0;

        while ip < code.len() {
            let opcode_byte = *code.get(ip).ok_or_else(|| CompileError::Error {
                message: format!("LSRA 重写：字节码越界 @ ip={}", ip),
                line: 0,
                column: 0,
            })?;
            let op = Opcode::decode_opcode(opcode_byte).ok_or_else(|| CompileError::Error {
                message: format!("LSRA 重写：无效 opcode 0x{:02X} @ ip={}", opcode_byte, ip),
                line: 0,
                column: 0,
            })?;

            ip += 1;

            for &kind in op.operands() {
                match kind {
                    OperandKind::Reg => {
                        if ip + 2 > code.len() {
                            return Err(CompileError::Error {
                                message: format!("LSRA 重写：Reg 操作数截断 @ ip={}", ip),
                                line: 0,
                                column: 0,
                            });
                        }
                        let vreg = u16::from_le_bytes([code[ip], code[ip + 1]]);

                        let preg = mapping.get(vreg as usize).copied().flatten().unwrap_or(vreg);

                        let bytes = preg.to_le_bytes();
                        code[ip] = bytes[0];
                        code[ip + 1] = bytes[1];

                        lsra_peak = lsra_peak.max(preg + 1);
                        ip += 2;
                    }
                    OperandKind::Const
                    | OperandKind::Offset
                    | OperandKind::U16
                    | OperandKind::CaptureIdx => {
                        ip += 2;
                    }
                    OperandKind::U8 => {
                        ip += 1;
                    }
                    OperandKind::U32 => {
                        ip += 4;
                    }
                    OperandKind::None => {}
                }
            }
        }

        self.chunk.locals_count = self.chunk.locals_count.max(lsra_peak);
        Ok(())
    }

    // ========================================================================
    // LSRA 结果查询（LSRA Result Query）
    // ========================================================================

    /// 查询 LSRA 分配结果的完整映射
    ///
    /// 返回一个切片引用，其中 `result[vreg]` 给出该虚拟寄存器的物理寄存器分配。
    pub fn lsra_mapping(&self) -> Option<&[Option<u16>; MAX_FUNCTION_LOCALS as usize]> {
        self.lsra_result.as_ref()
    }

    /// 查询单个虚拟寄存器的 LSRA 物理寄存器分配
    #[inline]
    pub fn lsra_phys_reg(&self, vreg: u16) -> Option<u16> {
        self.lsra_result.and_then(
            |m| {
                if (vreg as usize) < m.len() { m[vreg as usize] } else { None }
            },
        )
    }

    /// 检查 LSRA 是否已启用
    #[inline]
    pub fn is_lsra_enabled(&self) -> bool {
        self.use_lsra
    }

    /// 获取已收集的 def 点信息（用于调试和测试）
    #[cfg(test)]
    pub(super) fn def_ips_snapshot(&self) -> [Option<usize>; MAX_FUNCTION_LOCALS as usize] {
        self.def_ips
    }

    /// 获取已收集的 use 点信息（用于调试和测试）
    #[cfg(test)]
    #[allow(dead_code)] // 仅测试使用，保留供 LSRA 单元测试断言
    pub(super) fn use_ips_snapshot(&self) -> [Option<usize>; MAX_FUNCTION_LOCALS as usize] {
        self.use_ips
    }
}
