//! # 属性访问 opcode 实现（PIC — Polymorphic Inline Cache）
//!
//! 包含属性 get/set 的 4 路组相联 PIC 实现：
//! - `op_get_prop` — 属性读取（热路径走 PIC，miss 走冷路径）
//! - `op_set_prop` — 属性写入
//! - `get_prop_miss_pic` — PIC miss 冷路径
//! - `get_prop_miss_pic_fallback` — 非堆对象 fallback
//! - `set_prop_slow` — set_prop 冷路径

use crate::vm::VM;
use nuzo_values::*;

use super::cache_types::{PIC_WAYS, PropICEntry};
use super::cold_path::err_const_out_of_bounds;

/// 属性访问 PIC miss 冷路径参数
struct PropMissArgs {
    ip_start: usize,
    dest_reg: u16,
    obj: Value,
    prop_idx: usize,
    prop_intern_id: u32,
    ic_idx: usize,
    shape_id: u32,
}

impl VM {
    // ══════════════════════════════════════════════════════════════════
    // 🚀 PIC (Polymorphic Inline Cache) 终极版
    // ══════════════════════════════════════════════════════════════════
    #[inline(always)]
    pub(in crate::vm) fn op_get_prop(&mut self) -> Result<(), NuzoError> {
        let ip_start = self.ip - 1;
        let dest_reg = self.read_u16()?;
        let obj_reg = self.read_u16()?;
        let prop_idx = self.read_u16()? as usize;
        let obj = self.register(obj_reg)?;

        // 非堆对象跳过 PIC，直接走冷路径（Smi/Bool/Nil 无属性）
        if !obj.is_heap_object() {
            return self.get_prop_miss_pic_fallback(ip_start, dest_reg, obj, prop_idx);
        }

        let chunk = self.current_chunk()?;
        let prop_name_val = chunk.constants().get(prop_idx).ok_or_else(|| {
            self.error_with_source_location(err_const_out_of_bounds(
                prop_idx,
                chunk.constants().len(),
                self.ip,
                None,
            ))
        })?;
        let prop_intern_id = prop_name_val.string_index().unwrap_or(prop_idx as u32);

        // 哈希到槽点
        let ic_idx = (ip_start ^ (prop_intern_id as usize)) % self.cx.prop_ic.len();
        let slot = &self.cx.prop_ic[ic_idx];

        // 顺序探测 4 路
        let nuzo_type = obj.nuzo_type();
        let shape_id = nuzo_type.shape_id();

        for way in 0..PIC_WAYS {
            let entry = &slot.ways[way];
            if entry.ip == ip_start as u32
                && entry.prop_intern_id == prop_intern_id
                && entry.shape_id == shape_id
                && let Some(val) = nuzo_type.get_prop_by_slot(entry.slot_index as usize)
            {
                // 简单 LRU 近似：将本次命中的路号置为 0（表示最近使用）
                if way != 0 {
                    let slot = &mut self.cx.prop_ic[ic_idx];
                    slot.ways[0..=way].rotate_right(1);
                }
                self.set_register(dest_reg, val)?;
                return Ok(());
            }
        }

        // 未命中，走冷路径
        self.get_prop_miss_pic(PropMissArgs {
            ip_start,
            dest_reg,
            obj,
            prop_idx,
            prop_intern_id,
            ic_idx,
            shape_id,
        })
    }

    #[cold]
    #[inline(never)]
    fn get_prop_miss_pic(&mut self, args: PropMissArgs) -> Result<(), NuzoError> {
        let PropMissArgs { ip_start, dest_reg, obj, prop_idx, prop_intern_id, ic_idx, shape_id } =
            args;
        let chunk = self.current_chunk()?;
        let prop_name_val = chunk.constants().get(prop_idx).ok_or_else(|| {
            self.error_with_source_location(err_const_out_of_bounds(
                prop_idx,
                chunk.constants().len(),
                self.ip,
                None,
            ))
        })?;
        let prop_name = prop_name_val.as_string_opt().ok_or_else(|| {
            self.error_with_source_location(NuzoError::type_mismatch(
                "string".to_string(),
                format!("constant (type={})", prop_name_val.type_name()),
            ))
        })?;

        let nuzo_type = obj.nuzo_type();
        if let Some(val) = nuzo_type.get_prop(&prop_name) {
            // 尝试在对象前 8 个槽中找到该值，确定 slot_index
            let len = nuzo_type.obj_len();
            let probe_limit = len.min(8);
            let val_bits = val.into_raw_bits();
            let mut found_slot = None;

            for i in 0..probe_limit {
                if let Some(slot_val) = nuzo_type.get_prop_by_slot(i)
                    && slot_val.into_raw_bits() == val_bits
                {
                    found_slot = Some(i as u32);
                    break;
                }
            }

            // 更新 PIC：使用近似 LRU，替换最久未使用的路（lru 指示的路）
            if let Some(slot_idx) = found_slot {
                let mut slot = self.cx.prop_ic[ic_idx];
                let new_entry = PropICEntry {
                    ip: ip_start as u32,
                    prop_intern_id,
                    shape_id,
                    slot_index: slot_idx,
                };

                // 查找是否已有相同 ip+prop_intern 的 entry（可能因 shape 变化而 miss）
                let mut replaced = false;
                for way in 0..PIC_WAYS {
                    if slot.ways[way].ip == ip_start as u32
                        && slot.ways[way].prop_intern_id == prop_intern_id
                    {
                        // 更新 shape 和 slot
                        slot.ways[way] = new_entry;
                        replaced = true;
                        break;
                    }
                }
                if !replaced {
                    // 使用 lru 指示的路进行替换
                    let lru = slot.lru as usize;
                    slot.ways[lru] = new_entry;
                    // 更新 lru 为下一个路（简单循环）
                    slot.lru = ((lru + 1) % PIC_WAYS) as u8;
                }
                self.cx.prop_ic[ic_idx] = slot;
            }

            self.set_register(dest_reg, val)?;
        } else {
            self.set_register(dest_reg, NIL)?;
        }
        Ok(())
    }

    /// 非堆对象属性访问：直接返回 NIL（Smi/Bool/Nil 无属性）
    #[inline(always)]
    fn get_prop_miss_pic_fallback(
        &mut self,
        _ip_start: usize,
        dest_reg: u16,
        _obj: Value,
        _prop_idx: usize,
    ) -> Result<(), NuzoError> {
        self.set_register(dest_reg, NIL)
    }

    #[inline(always)]
    pub(in crate::vm) fn op_set_prop(&mut self) -> Result<(), NuzoError> {
        let obj_reg = self.read_u16()?;
        let prop_idx = self.read_u16()? as usize;
        let val_reg = self.read_u16()?;
        let obj = self.register(obj_reg)?;
        let val = self.register(val_reg)?;

        if !obj.is_heap_object() {
            return Ok(());
        }
        self.set_prop_slow(obj_reg, obj, val, prop_idx)
    }

    #[cold]
    #[inline(never)]
    fn set_prop_slow(
        &mut self,
        obj_reg: u16,
        obj: Value,
        val: Value,
        prop_idx: usize,
    ) -> Result<(), NuzoError> {
        let chunk = self.current_chunk()?;
        let prop_name_val = chunk.constants().get(prop_idx).ok_or_else(|| {
            self.error_with_source_location(err_const_out_of_bounds(
                prop_idx,
                chunk.constants().len(),
                self.ip,
                None,
            ))
        })?;
        let prop_name = prop_name_val.as_string_opt().ok_or_else(|| {
            self.error_with_source_location(NuzoError::type_mismatch(
                "string".to_string(),
                format!("constant (type={})", prop_name_val.type_name()),
            ))
        })?;
        let key_index = prop_name_val.string_index().map(|id| id as usize);

        if let Some(new_obj) = obj.nuzo_type().set_prop(&prop_name, key_index, val) {
            // Arena 快速路径（首版：GC 分配 + Arena 编码）
            match self.try_alloc_arena(new_obj) {
                Ok(arena_val) => {
                    self.set_register(obj_reg, arena_val)?;
                }
                Err(fallback_obj) => {
                    // Fallthrough: 原始 Scratch/GC 路径
                    let idx = self.gc.alloc_scratch(fallback_obj);
                    self.set_register(obj_reg, Value::from_scratch_index(idx))?;
                }
            }
        }
        Ok(())
    }
}
