//! # NuzoType Trait — 统一类型抽象层
//!
//! 将分散在 Value / HeapObject / VM dispatch 中的类型操作统一为单一接口。
//! 覆盖所有 VM 运行时需要的**类型查询与操作**。

use crate::errors::NuzoError;
use crate::heap::HeapObjectOps;
use crate::value::{Value, ValueExt};
use nuzo_core::encoding::char_len;

pub trait NuzoType {
    fn type_name(&self) -> &'static str;
    fn is_truthy(&self) -> bool;
    fn obj_len(&self) -> usize;
    /// Stable shape identifier for property-access PIC guards.
    fn shape_id(&self) -> u32;

    // ── 只读操作 (返回 Value / Option / Result) ──
    fn get_prop(&self, name: &str) -> Option<Value>;
    /// 按 slot 索引直接取属性值，绕过字符串查找（μPIC 快路径）。
    /// slot 语义由各类型自行定义：Array=属性序号, Dict=插入序号, Object=Shape 位置。
    fn get_prop_by_slot(&self, slot: usize) -> Option<Value>;
    fn get_index(&self, idx: Value) -> Result<Value, NuzoError>;

    // ── COW 写操作 (返回 Option<HeapObject>, None 表示不可变/不支持) ──
    /// 设置属性，返回修改后的新 HeapObject (COW)。None 表示不支持或只读。
    fn set_prop(
        &self,
        name: &str,
        key_index: Option<usize>,
        val: Value,
    ) -> Option<super::heap::HeapObject>;
    /// 设置索引，返回修改后的新 HeapObject (COW)。
    fn set_index(&self, idx: Value, val: Value) -> Result<super::heap::HeapObject, NuzoError>;

    #[inline]
    fn nuzo_type(&self) -> &Self
    where
        Self: Sized,
    {
        self
    }
}

// ============================================================================
// Value 实现 — 统一入口，委托到具体类型
// ============================================================================

impl NuzoType for Value {
    #[inline]
    fn type_name(&self) -> &'static str {
        if self.is_nil() {
            "nil"
        } else if self.is_bool() {
            "bool"
        } else if self.is_smi() {
            "integer"
        } else if self.is_float() && !self.is_smi() {
            "number"
        } else if self.is_string() {
            "string"
        } else if self.is_heap_object() {
            self.as_heap_object_opt().map_or("heap_object(dangling)", |h| h.type_name())
        } else {
            "unknown"
        }
    }

    #[inline]
    fn is_truthy(&self) -> bool {
        !(self.is_nil()
            || (self.is_bool() && !self.as_bool())
            || (self.is_number() && self.as_number() == 0.0))
    }

    #[inline]
    fn obj_len(&self) -> usize {
        if self.is_string() {
            self.as_string_opt().map_or(0, |s| char_len(&s))
        } else if self.is_heap_object() {
            self.as_heap_object_opt().map_or(0, |obj| obj.obj_len())
        } else {
            0
        }
    }

    #[inline]
    fn shape_id(&self) -> u32 {
        if self.is_heap_object() {
            self.as_heap_object_opt().map_or(0, |obj| obj.shape_id())
        } else {
            0
        }
    }

    #[inline]
    fn get_prop(&self, name: &str) -> Option<Value> {
        if self.is_string() {
            return if name == "length" || name == "len" {
                Some(Value::from_number(self.obj_len() as f64))
            } else {
                None
            };
        }
        if self.is_heap_object() {
            return self.as_heap_object_opt()?.get_prop(name);
        }
        None
    }

    #[inline]
    fn get_prop_by_slot(&self, slot: usize) -> Option<Value> {
        if self.is_string() {
            // String 只支持 length 属性 (slot 0)
            return if slot == 0 { Some(Value::from_number(self.obj_len() as f64)) } else { None };
        }
        if self.is_heap_object() {
            return self.as_heap_object_opt()?.get_prop_by_slot(slot);
        }
        None
    }

    #[inline]
    fn get_index(&self, idx: Value) -> Result<Value, NuzoError> {
        if let Some(obj) = self.as_heap_object_opt() {
            return obj.get_index(idx);
        }
        // BUG-D 修复：字符串索引支持，按 Unicode 字符（而非字节）取单字符子串。
        if self.is_string() {
            let i = idx.try_as_smi().map_err(|_| {
                NuzoError::type_mismatch("integer index", idx.type_name().to_string())
            })?;
            if i < 0 {
                return Err(NuzoError::index_out_of_bounds(i.to_string(), "0".to_string()));
            }
            let s = self
                .as_string_opt()
                .ok_or_else(|| NuzoError::unsupported_operation("index read", self.type_name()))?;
            // 按 Unicode scalar 而非 UTF-8 字节计数，避免多字节字符截断产生无效字符串。
            let len = nuzo_core::encoding::char_len(&s) as i64;
            if i >= len {
                return Err(NuzoError::index_out_of_bounds(i.to_string(), len.to_string()));
            }
            // 用 char_indices 找到第 i 个字符的字节范围，避免 O(n) 收集整个字符串。
            let byte_pos =
                s.char_indices().nth(i as usize).map(|(pos, _)| pos).ok_or_else(|| {
                    NuzoError::index_out_of_bounds(i.to_string(), len.to_string())
                })?;
            let ch = s[byte_pos..]
                .chars()
                .next()
                .ok_or_else(|| NuzoError::index_out_of_bounds(i.to_string(), len.to_string()))?;
            Ok(Value::from_string(&ch.to_string()))
        } else {
            Err(NuzoError::unsupported_operation("index read", self.type_name()))
        }
    }

    #[inline]
    fn set_prop(
        &self,
        name: &str,
        key_index: Option<usize>,
        val: Value,
    ) -> Option<super::heap::HeapObject> {
        self.as_heap_object_opt()?.set_prop(name, key_index, val)
    }

    #[inline]
    fn set_index(&self, idx: Value, val: Value) -> Result<super::heap::HeapObject, NuzoError> {
        self.as_heap_object_opt()
            .ok_or_else(|| {
                NuzoError::internal(
                    crate::InternalError::CompilerBug {
                        message: "heap object not found".to_string(),
                    },
                    None,
                )
            })?
            .set_index(idx, val)
    }
}

// ============================================================================
// HeapObject 实现 — 复合对象协议
// ============================================================================

impl NuzoType for super::heap::HeapObject {
    #[inline(always)]
    fn type_name(&self) -> &'static str {
        self.ops().type_name()
    }

    #[inline(always)]
    fn is_truthy(&self) -> bool {
        true
    }

    #[inline(always)]
    fn obj_len(&self) -> usize {
        self.obj_len()
    }

    #[inline(always)]
    fn shape_id(&self) -> u32 {
        self.shape_id()
    }

    #[inline(always)]
    fn get_prop(&self, name: &str) -> Option<Value> {
        self.get_prop(name)
    }

    /// μPIC 快路径：按 slot 索引直接取属性值，零字符串比较。
    ///
    /// slot 映射表:
    /// - Array:   slot 0 → length
    /// - Dict:    slot → 插入序号对应的值 (有序迭代)
    /// - 其他:    None (不支持 slot 访问)
    #[inline(always)]
    fn get_prop_by_slot(&self, slot: usize) -> Option<Value> {
        match self {
            Self::Array(arr) => {
                // Array 只有一个虚拟属性: length (slot 0)
                if slot == 0 { Some(Value::from_number(arr.len() as f64)) } else { None }
            }
            Self::Dict(d) => {
                // Dict: slot 按插入序号（键值对序号）索引
                let mut iter = d.iter();
                for _ in 0..slot {
                    iter.next()?;
                }
                iter.next().map(|(_, v)| v)
            }
            _ => None,
        }
    }

    #[inline(always)]
    fn get_index(&self, idx: Value) -> Result<Value, NuzoError> {
        self.get_index(idx)
    }

    #[inline(always)]
    fn set_prop(
        &self,
        name: &str,
        key_index: Option<usize>,
        val: Value,
    ) -> Option<super::heap::HeapObject> {
        self.set_prop(name, key_index, val)
    }

    #[inline(always)]
    fn set_index(&self, idx: Value, val: Value) -> Result<super::heap::HeapObject, NuzoError> {
        self.set_index(idx, val)
    }
}
