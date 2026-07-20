//! # 索引访问 opcode 实现
//!
//! 包含数组/字典等可索引对象的读写指令：
//! - `op_get_index` — 索引读取（含错误处理 hook）
//! - `op_set_index` — 索引写入（COW 语义，返回新对象）
//! - `op_set_index_mut` — 索引原地写入（in-place mutation）

use crate::vm::VM;
use nuzo_bytecode::Opcode;
use nuzo_values::*;

impl VM {
    pub(in crate::vm) fn op_get_index(&mut self) -> Result<(), NuzoError> {
        let dest_reg = self.read_u16()?;
        let obj_reg = self.read_u16()?;
        let idx_reg = self.read_u16()?;
        let obj = self.register(obj_reg)?;
        let idx = self.register(idx_reg)?;

        match obj.nuzo_type().get_index(idx) {
            Ok(value) => {
                self.set_register(dest_reg, value)?;
            }
            Err(e) => {
                let e = self.error_with_source_location(e);
                if !self.handle_error_in_diagnostic_mode(
                    e.clone(),
                    Some(Opcode::GetIndex),
                    Some(self.ip.saturating_sub(1)),
                ) {
                    return Err(e);
                }
                self.set_register(dest_reg, NIL)?;
            }
        }
        Ok(())
    }

    pub(in crate::vm) fn op_set_index(&mut self) -> Result<(), NuzoError> {
        let obj_reg = self.read_u16()?;
        let idx_reg = self.read_u16()?;
        let val_reg = self.read_u16()?;
        let obj = self.register(obj_reg)?;
        let idx = self.register(idx_reg)?;
        let val = self.register(val_reg)?;

        if !obj.is_heap_object() {
            return Err(self.error_with_source_location(NuzoError::type_mismatch(
                "indexable (array, dict)".to_string(),
                obj.type_name().to_string(),
            )));
        }
        let modified_obj =
            obj.nuzo_type().set_index(idx, val).map_err(|e| self.error_with_source_location(e))?;
        let new_idx = self.gc.alloc_scratch(modified_obj);
        self.set_register(obj_reg, Value::from_scratch_index(new_idx))?;
        Ok(())
    }

    pub(in crate::vm) fn op_set_index_mut(&mut self) -> Result<(), NuzoError> {
        let obj_reg = self.read_u16()?;
        let idx_reg = self.read_u16()?;
        let val_reg = self.read_u16()?;
        let obj = self.register(obj_reg)?;
        let idx = self.register(idx_reg)?;
        let val = self.register(val_reg)?;

        if !obj.is_heap_object() {
            return Err(self.error_with_source_location(NuzoError::type_mismatch(
                "indexable (array, dict)".to_string(),
                obj.type_name().to_string(),
            )));
        }

        let result = obj.mutate_heap_object(|heap_obj| heap_obj.set_index_mut(idx, val));

        match result {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(self.error_with_source_location(e)),
            None => Err(NuzoError::internal(
                InternalError::CompilerBug {
                    message: "SetIndexMut: heap object not found in any heap area".to_string(),
                },
                Some(self.current_diagnosis(
                    "SetIndexMut: mutate_heap_object returned None for a heap object",
                )),
            )),
        }
    }
}
