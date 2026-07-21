//! # 闭包变量捕获 opcode 实现
//!
//! 包含：
//! - `op_capture` — 将变量捕获到闭包（outer / inner 两种模式）+ 应用到闭包
//! - `op_get_captured` — 读取闭包捕获的变量
//! - `op_set_captured` — 写入闭包捕获的可变变量（ByBox 模式）
//!
//! ## `op_capture` 子函数拆分
//! - `capture_outer_var` — 从 parent_env 链解析外层捕获变量
//! - `capture_inner_var` — 从当前帧寄存器读取内层捕获变量（按 CaptureMode 决定 ByValue/ByBox）

use crate::vm::VM;
use nuzo_abi::NuzoErrorExt;
use nuzo_core::{CAPTURE_OUTER_FLAG, CAPTURE_OUTER_INDEX_MASK};
use nuzo_values::heap::{CaptureMode, CapturedVar};
use nuzo_values::value::{allocate_box, get_box, set_box};
use nuzo_values::*;

impl VM {
    pub(in crate::vm) fn op_capture(&mut self) -> Result<(), NuzoError> {
        let closure_reg = self.read_u16()?;
        let capture_index = self.read_u16()?;
        let source = self.read_u16()?;

        let captured_var = if source & CAPTURE_OUTER_FLAG != 0 {
            let outer_index = (source & CAPTURE_OUTER_INDEX_MASK) as usize;
            self.capture_outer_var(closure_reg, outer_index)?
        } else {
            self.capture_inner_var(closure_reg, capture_index, source)?
        };

        // 应用 captured_var 到闭包的 captured 向量
        let closure_val = self.register(closure_reg)?;
        closure_val.mutate_heap_object(|heap_obj| {
            match heap_obj {
                HeapObject::Closure { prototype: _, captured, parent_env } => {
                    while captured.len() <= capture_index as usize {
                        captured.push(CapturedVar::Value(NIL));
                    }
                    captured[capture_index as usize] = captured_var;
                    *parent_env = self.current_closure();
                }
                _ => {
                    // mutate_heap_object 无法返回错误，设置一个空闭包作为标记
                    // 实际上这个分支不应到达（上面的校验已保证是 Closure）
                }
            }
        });
        Ok(())
    }

    /// 解析外层捕获变量：沿 parent_env 链查找 `outer_index` 处的 CapturedVar
    ///
    /// 流程：
    /// 1. 取 closure_reg 对应的闭包 HeapObject
    /// 2. 从闭包的 parent_env 开始遍历（若为空则 fallback 到 current_closure）
    /// 3. 在每一层 Closure 中检查 `captured[outer_index]` 是否存在
    /// 4. 找到则返回 clone；链耗尽则报错；深度超 256 则报循环引用错
    fn capture_outer_var(
        &mut self,
        closure_reg: u16,
        outer_index: usize,
    ) -> Result<CapturedVar, NuzoError> {
        let closure_val = self.register(closure_reg)?;
        let heap_obj = closure_val.as_heap_object_opt().ok_or(NuzoError::internal(
            InternalError::CompilerBug {
                message: "Capture target is not a heap object".to_string(),
            },
            Some(self.current_diagnosis(
                "Capture: outer flag set but closure register is not a heap object",
            )),
        ))?;

        let mut current_env = match &*heap_obj {
            HeapObject::Closure { parent_env, .. } => parent_env.clone(),
            _ => {
                return Err(NuzoError::internal(
                    InternalError::CompilerBug {
                        message: "Capture target is not a closure".to_string(),
                    },
                    Some(self.current_diagnosis(
                        "Capture: outer flag set but heap object is not a Closure variant",
                    )),
                ));
            }
        };
        let mut depth: u32 = 0;
        if current_env.is_none() {
            current_env = self.current_closure();
        }

        let mut found_capture: Option<CapturedVar> = None;
        loop {
            let env = current_env.clone().ok_or(NuzoError::internal(
                InternalError::CompilerBug {
                    message: format!(
                        "Capture outer index {} not found in parent_env chain (chain exhausted)",
                        outer_index
                    ),
                },
                Some(self.current_diagnosis(&format!(
                    "Capture: outer index {} not found, parent_env chain exhausted",
                    outer_index
                ))),
            ))?;

            const MAX_CAPTURE_DEPTH: u32 = 256;
            if depth >= MAX_CAPTURE_DEPTH {
                return Err(NuzoError::internal(
                    InternalError::CompilerBug {
                        message: format!(
                            "Capture parent_env chain depth exceeded limit ({} >= {})",
                            depth, MAX_CAPTURE_DEPTH
                        ),
                    },
                    Some(self.current_diagnosis(&format!(
                        "Capture: parent_env chain too deep ({}), possible circular reference",
                        depth
                    ))),
                ));
            }
            depth += 1;

            match &*env {
                HeapObject::Closure { captured, parent_env: next_env, .. } => {
                    if outer_index < captured.len() {
                        found_capture = Some(captured[outer_index].clone());
                        break;
                    }
                    current_env = next_env.clone();
                }
                _ => {
                    return Err(NuzoError::internal(
                        InternalError::CompilerBug {
                            message: "Parent env is not a closure (chain corrupted)".to_string(),
                        },
                        Some(self.current_diagnosis(
                            "Capture: parent_env chain contains non-Closure heap object",
                        )),
                    ));
                }
            }
            if current_env.is_none() {
                break;
            }
        }
        found_capture.ok_or(NuzoError::internal(
            InternalError::CompilerBug { message: "Capture outer index not found".to_string() },
            None,
        ))
    }

    /// 解析内层捕获变量：从当前帧寄存器读取，并按 CaptureMode 决定 ByValue/ByBox
    fn capture_inner_var(
        &mut self,
        closure_reg: u16,
        capture_index: u16,
        source: u16,
    ) -> Result<CapturedVar, NuzoError> {
        let value = self.register(source)?;
        let closure_val = self.register(closure_reg)?;
        let heap_obj = closure_val.as_heap_object_opt().ok_or(NuzoError::internal(
            InternalError::CompilerBug {
                message: "Capture target is not a heap object".to_string(),
            },
            Some(self.current_diagnosis(
                "Capture: inner capture but closure register is not a heap object",
            )),
        ))?;
        match &*heap_obj {
            HeapObject::Closure { prototype, .. } => {
                let mode = prototype
                    .captured_vars
                    .get(capture_index as usize)
                    .map(|info| info.mode)
                    .unwrap_or(CaptureMode::ByValue);
                match mode {
                    CaptureMode::ByValue => Ok(CapturedVar::Value(value)),
                    CaptureMode::ByBox => {
                        let box_idx = allocate_box(value)?;
                        Ok(CapturedVar::Box(box_idx))
                    }
                }
            }
            _ => Err(NuzoError::internal(
                InternalError::CompilerBug {
                    message: "Capture target is not a closure".to_string(),
                },
                Some(self.current_diagnosis(
                    "Capture: inner capture but heap object is not a Closure variant",
                )),
            )),
        }
    }

    pub(in crate::vm) fn op_get_captured(&mut self) -> Result<(), NuzoError> {
        let dest = self.read_u16()?;
        let capture_index = self.read_u16()?;
        let closure_ref = self.current_closure().ok_or(NuzoError::internal(
            InternalError::CompilerBug {
                message: "GetCaptured executed outside of closure context".to_string(),
            },
            Some(self.current_diagnosis("GetCaptured: no current closure found in call frame")),
        ))?;

        match &*closure_ref {
            HeapObject::Closure { captured, .. } => {
                if capture_index as usize >= captured.len() {
                    return Err(self.error_with_source_location(
                        NuzoErrorExt::index_out_of_bounds(
                            capture_index.to_string(),
                            captured.len().to_string(),
                        ),
                    ));
                }
                let value = match &captured[capture_index as usize] {
                    CapturedVar::Value(v) => *v,
                    CapturedVar::Box(box_idx) => get_box(*box_idx).ok_or_else(|| {
                        self.error_with_source_location(NuzoErrorExt::index_out_of_bounds(
                            box_idx.to_string(),
                            "gc_heap".to_string(),
                        ))
                    })?,
                };
                self.set_register(dest, value)?;
            }
            _ => {
                return Err(NuzoError::internal(
                    InternalError::CompilerBug {
                        message: "Current closure is not a Closure heap object".to_string(),
                    },
                    Some(self.current_diagnosis(
                        "GetCaptured: current_closure() returned non-Closure heap object",
                    )),
                ));
            }
        }
        Ok(())
    }

    pub(in crate::vm) fn op_set_captured(&mut self) -> Result<(), NuzoError> {
        let capture_index = self.read_u16()?;
        let src = self.read_u16()?;
        let value = self.register(src)?;
        let closure_ref = self.current_closure().ok_or(NuzoError::internal(
            InternalError::CompilerBug {
                message: "SetCaptured executed outside of closure context".to_string(),
            },
            Some(self.current_diagnosis("SetCaptured: no current closure found in call frame")),
        ))?;

        match &*closure_ref {
            HeapObject::Closure { captured, .. } => {
                if capture_index as usize >= captured.len() {
                    return Err(self.error_with_source_location(
                        NuzoErrorExt::index_out_of_bounds(
                            capture_index.to_string(),
                            captured.len().to_string(),
                        ),
                    ));
                }
                match &captured[capture_index as usize] {
                    CapturedVar::Value(_) => {
                        return Err(self.error_with_source_location(NuzoErrorExt::assert_failed(
                            "Cannot assign to immutable captured variable (ByValue).",
                        )));
                    }
                    CapturedVar::Box(box_idx) => {
                        set_box(*box_idx, value)?;
                        #[cfg(debug_assertions)]
                        {
                            if let Some(v) = get_box(*box_idx) {
                                debug_assert_eq!(v, value);
                            }
                        }
                    }
                }
            }
            _ => {
                return Err(NuzoError::internal(
                    InternalError::CompilerBug {
                        message: "Current closure is not a Closure heap object".to_string(),
                    },
                    Some(self.current_diagnosis(
                        "SetCaptured: current_closure() returned non-Closure heap object",
                    )),
                ));
            }
        }
        Ok(())
    }
}
