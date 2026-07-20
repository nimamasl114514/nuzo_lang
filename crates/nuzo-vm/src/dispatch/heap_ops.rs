//! # 堆对象构造与查询 opcode 实现
//!
//! 包含：
//! - `op_array_new` — 数组构造（支持 inline payload 与 SetIndex 填充两种 IR 模式）
//! - `op_range_new` — Range 对象构造
//! - `op_len` — 集合长度查询（string/array/dict/range）
//! - `op_slicechain_new` / `op_slicechain_append` / `op_slicechain_finish` — SCSB 字符串构建器

use crate::vm::VM;
use nuzo_values::*;

impl VM {
    pub(in crate::vm) fn op_array_new(&mut self) -> Result<(), NuzoError> {
        let dest = self.read_u16()?;
        let count = self.read_u16()? as usize;
        // 边界检查：dest + 1 + count 必须落在 u16 寄存器域内，否则 u16 加法溢出
        // （debug 模式 panic，release 模式 wrap 为错误索引）。
        let last_reg = dest as usize + 1 + count;
        if last_reg > u16::MAX as usize {
            return Err(NuzoError::internal(
                InternalError::RegisterOverflow { count: last_reg },
                None,
            ));
        }
        let available_regs = self.cx.registers.len().saturating_sub(self.current_base);
        let has_inline_payload = dest as usize + count < available_regs;
        let mut elements = Vec::with_capacity(count);
        if has_inline_payload {
            for i in 0..count {
                elements.push(self.register((dest as usize + 1 + i) as u16)?);
            }
        } else {
            // IR codegen emits ArrayNew as "allocate count slots, then SetIndex fill".
            // Older bytecode inlines elements in dest+1..dest+count. Support both.
            elements.resize(count, NIL);
        }
        let heap_obj = HeapObject::Array(elements);

        // Arena 快速路径（首版：GC 分配 + Arena 编码）
        match self.try_alloc_arena(heap_obj) {
            Ok(arena_val) => {
                self.set_register(dest, arena_val)?;
                return Ok(());
            }
            Err(fallback_obj) => {
                // Fallthrough: 原始 Scratch/GC 路径
                let idx = self.gc.alloc_scratch(fallback_obj);
                self.set_register(dest, Value::from_scratch_index(idx))?;
            }
        }
        Ok(())
    }

    pub(in crate::vm) fn op_range_new(&mut self) -> Result<(), NuzoError> {
        let dest = self.read_u16()?;
        let start_reg = self.read_u16()?;
        let end_reg = self.read_u16()?;
        let inclusive_byte = self.read_byte()?;

        let start_val = self.register(start_reg)?;
        let end_val = self.register(end_reg)?;
        if !nuzo_core::tag::is_number(start_val.into_raw_bits())
            || !nuzo_core::tag::is_number(end_val.into_raw_bits())
        {
            return Err(self.error_with_source_location(NuzoError::type_mismatch(
                "numbers for range".to_string(),
                format!("{}, {}", start_val.type_name(), end_val.type_name()),
            )));
        }
        let heap_obj = HeapObject::Range {
            start: start_val.as_number(),
            end: end_val.as_number(),
            range_end: if inclusive_byte != 0 { RangeEnd::Inclusive } else { RangeEnd::Exclusive },
        };

        // Arena 快速路径（首版：GC 分配 + Arena 编码）
        match self.try_alloc_arena(heap_obj) {
            Ok(arena_val) => {
                self.set_register(dest, arena_val)?;
                return Ok(());
            }
            Err(fallback_obj) => {
                // Fallthrough: 原始 Scratch/GC 路径
                let idx = self.gc.alloc_scratch(fallback_obj);
                self.set_register(dest, Value::from_scratch_index(idx))?;
            }
        }
        Ok(())
    }

    pub(in crate::vm) fn op_len(&mut self) -> Result<(), NuzoError> {
        let dest = self.read_u16()?;
        let obj_reg = self.read_u16()?;
        let obj = self.register(obj_reg)?;
        let is_collection = obj.is_string()
            || obj.as_heap_object_opt().is_some_and(|h| {
                matches!(*h, HeapObject::Array(_) | HeapObject::Dict(_) | HeapObject::Range { .. })
            });
        if !is_collection {
            return Err(self.error_with_source_location(NuzoError::type_mismatch(
                "collection (array, dict, string, or range)".to_string(),
                obj.type_name().to_string(),
            )));
        }
        let len = obj.nuzo_type().obj_len();
        self.set_register(dest, Value::from_number(len as f64))?;
        Ok(())
    }

    // ========================================================================
    // SCSB — SliceChain 字符串构建器操作码
    // ========================================================================

    /// SliceChainNew: 创建空切片链
    pub(in crate::vm) fn op_slicechain_new(&mut self) -> Result<(), NuzoError> {
        let dest = self.read_u16()?;
        let chain = nuzo_values::heap::SliceChain::new();
        let obj = HeapObject::StrBuilder(chain);
        let idx = self.gc.alloc(obj);
        let val = nuzo_core::Value::from_gc_index(idx);
        self.set_register(dest, val)?;
        Ok(())
    }

    /// SliceChainAppend: 追加到切片链
    pub(in crate::vm) fn op_slicechain_append(&mut self) -> Result<(), NuzoError> {
        let chain_reg = self.read_u16()?;
        let src_reg = self.read_u16()?;
        let chain_val = self.register(chain_reg)?;
        let src_val = self.register(src_reg)?;
        let idx = chain_val.heap_index().ok_or_else(|| {
            NuzoError::internal(
                InternalError::CompilerBug {
                    message: "SliceChainAppend: chain is not a heap object".into(),
                },
                None,
            )
        })?;
        let src_str = src_val.concat_repr();
        let heap_obj = self.gc.get_mut(idx)?;
        match heap_obj {
            HeapObject::StrBuilder(sc) => {
                sc.append(&src_str);
            }
            _ => {
                return Err(NuzoError::internal(
                    InternalError::CompilerBug {
                        message: "SliceChainAppend: not a StrBuilder".into(),
                    },
                    None,
                ));
            }
        }
        Ok(())
    }

    /// SliceChainFinish: 完成切片链，返回拼接结果字符串
    pub(in crate::vm) fn op_slicechain_finish(&mut self) -> Result<(), NuzoError> {
        let dest = self.read_u16()?;
        let chain_reg = self.read_u16()?;
        let chain_val = self.register(chain_reg)?;
        let idx = chain_val.heap_index().ok_or_else(|| {
            NuzoError::internal(
                InternalError::CompilerBug {
                    message: "SliceChainFinish: chain is not a heap object".into(),
                },
                None,
            )
        })?;
        let heap_obj = self.gc.get_mut(idx)?;
        let result = match heap_obj {
            HeapObject::StrBuilder(sc) => sc.finish(),
            _ => {
                return Err(NuzoError::internal(
                    InternalError::CompilerBug {
                        message: "SliceChainFinish: not a StrBuilder".into(),
                    },
                    None,
                ));
            }
        };
        let val = Value::from_string(&result);
        self.set_register(dest, val)?;
        Ok(())
    }
}
