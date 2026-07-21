//! # 全局变量 opcode 实现（GMVC + ISS）
//!
//! 包含：
//! - `op_get_global` — 全局变量读取（带 GMVC 缓存 + ISS patch 特化）
//! - `op_set_global` — 全局变量写入（带版本号递增）
//! - `op_get_global_cached` — ISS 特化的热路径读取（版本内嵌指令操作数）
//!
//! ## GMVC (Global Multi-Versioned Cache)
//! 每个全局变量独立版本号，写时递增。缓存条目记录 (version, index)，
//! 读时校验版本，失配则回退到 resolve_global 路径并 patch 指令。
//!
//! ## ISS (Instruction Self-Specialization)
//! OP_GET_GLOBAL 首次 resolve 成功后，patch 字节码为 OP_GET_GLOBAL_CACHED，
//! 将 gidx + ver 内嵌进指令操作数，后续读取零表查找。

use crate::vm::VM;
use nuzo_abi::NuzoErrorExt;
use nuzo_bytecode::Opcode;
use nuzo_values::*;

use super::cache_types::GlobalCacheEntry;

impl VM {
    // ========================================================================
    // 🌍 GMVC (Global Variable Multi-Versioned Cache)
    // ========================================================================

    pub(in crate::vm) fn op_get_global(&mut self) -> Result<(), NuzoError> {
        let instr_start = self.ip - 1; // 指令起始 IP（opcode 已被 fetch_opcode 读取）
        let dest = self.read_u16()?;
        let name_idx = self.read_u16()? as usize;
        let _iss_pad = self.read_u16()?; // ISS padding: 跳过（将被 patch 为 version:u16)

        if name_idx < self.cx.global_cache.len() {
            let entry = self.cx.global_cache[name_idx];
            // GMVC: 检查每个全局变量的独立版本号
            if (entry.index as usize) < self.cx.global_versions.len()
                && self.cx.global_versions[entry.index as usize] == entry.version
                && let Some(value) = self.get_global(entry.index as usize)
            {
                self.set_register(dest, value)?;
                return Ok(());
            }
        }

        let name = self.get_constant_string(name_idx)?;
        match self.resolve_global(&name) {
            Some(idx) => {
                // 确保版本号数组足够
                if idx >= self.cx.global_versions.len() {
                    self.cx.global_versions.resize(idx + 1, 0);
                }
                if name_idx >= self.cx.global_cache.len() {
                    self.cx.global_cache.resize(
                        name_idx + 1,
                        GlobalCacheEntry { version: u32::MAX, index: u32::MAX },
                    );
                }
                let ver = self.cx.global_versions[idx];
                self.cx.global_cache[name_idx] =
                    GlobalCacheEntry { version: ver, index: idx as u32 };
                self.cx.hot_trace_table.invalidate_fused_cache_for_cigc(name_idx);
                match self.get_global(idx) {
                    Some(value) => {
                        self.set_register(dest, value)?;

                        // ★ ISS: 将 GetGlobal patch 为 GetGlobalCached
                        // 字节布局（7 字节）：
                        //   [0]    opcode
                        //   [1-2]  dest:u16
                        //   [3-4]  gidx:u16  (覆盖原 name_idx:u16)
                        //   [5-6]  ver:u16   (覆盖原 _iss_pad:u16)
                        //
                        // 边界检查：idx (usize) 与 ver (u32) 必须落在 u16 操作数域内，
                        // 否则 `as u16` 截断会写入错误值，导致后续 GetGlobalCached
                        // 读取到错误的 gidx/ver，引发幽灵全局变量访问。
                        if idx > u16::MAX as usize || ver > u16::MAX as u32 {
                            return Err(NuzoError::internal(
                                InternalError::GlobalIndexOverflow { idx, ver },
                                None,
                            ));
                        }
                        let cached_opcode = Opcode::GetGlobalCached as u8;
                        self.patch_code(instr_start, &[cached_opcode])?;
                        self.patch_code(instr_start + 3, &(idx as u16).to_le_bytes())?;
                        self.patch_code(instr_start + 5, &(ver as u16).to_le_bytes())?;
                    }
                    None => {
                        return Err(self.error_with_source_location(
                            NuzoErrorExt::index_out_of_bounds(
                                idx.to_string(),
                                self.global_count().to_string(),
                            ),
                        ));
                    }
                }
            }
            None => {
                return Err(self.error_with_source_location(NuzoErrorExt::undefined_variable(name)));
            }
        }
        Ok(())
    }

    pub(in crate::vm) fn op_set_global(&mut self) -> Result<(), NuzoError> {
        let src = self.read_u16()?;
        let name_idx = self.read_u16()? as usize;
        let value = self.register(src)?;

        // 快速路径：缓存命中且版本匹配
        if name_idx < self.cx.global_cache.len() {
            let entry = self.cx.global_cache[name_idx];
            if (entry.index as usize) < self.cx.global_versions.len()
                && self.cx.global_versions[entry.index as usize] == entry.version
            {
                // 更新全局值并递增该变量版本号
                let _ = self.set_global(entry.index as usize, value);
                self.cx.global_versions[entry.index as usize] =
                    self.cx.global_versions[entry.index as usize].wrapping_add(1);
                // 更新缓存 entry 的版本以保持同步
                self.cx.global_cache[name_idx].version =
                    self.cx.global_versions[entry.index as usize];
                return Ok(());
            }
        }

        let name = self.get_constant_string(name_idx)?;
        let idx = self.resolve_global(&name).unwrap_or_else(|| {
            self.set_global_by_name(&name, value);
            usize::MAX
        });

        // 确保缓存数组足够大（使用无效哨兵值作为默认填充，避免与有效条目混淆）
        if name_idx >= self.cx.global_cache.len() {
            self.cx
                .global_cache
                .resize(name_idx + 1, GlobalCacheEntry { version: u32::MAX, index: u32::MAX });
        }

        if idx != usize::MAX {
            // 已有变量：更新值 + 版本 + 缓存
            if idx >= self.cx.global_versions.len() {
                self.cx.global_versions.resize(idx + 1, 0);
            }
            let _ = self.set_global(idx, value);
            self.cx.global_versions[idx] = self.cx.global_versions[idx].wrapping_add(1);
            self.cx.global_cache[name_idx] =
                GlobalCacheEntry { version: self.cx.global_versions[idx], index: idx as u32 };
            self.cx.hot_trace_table.invalidate_fused_cache_for_cigc(name_idx);
        } else {
            // 首次定义变量：解析实际索引并填充缓存（修复：旧代码跳过此分支导致缓存残留默认值）
            let actual_idx = self.resolve_global(&name).ok_or_else(|| {
                NuzoError::internal(InternalError::GlobalRegistrationFailed, None)
            })?;
            if actual_idx >= self.cx.global_versions.len() {
                self.cx.global_versions.resize(actual_idx + 1, 0);
            }
            self.cx.global_versions[actual_idx] =
                self.cx.global_versions[actual_idx].wrapping_add(1);
            self.cx.global_cache[name_idx] = GlobalCacheEntry {
                version: self.cx.global_versions[actual_idx],
                index: actual_idx as u32,
            };
        }
        Ok(())
    }

    // ========================================================================
    // 🚀 ISS: Instruction Self-Specialization — 特化全局变量读取
    // ========================================================================

    /// ISS 特化全局变量读取（零表查找热路径）
    ///
    /// 由 `OP_GET_GLOBAL` 在首次 resolve 成功后 patch 而来。
    /// 所有缓存数据直接嵌入指令操作数，无需访问 `global_cache`。
    ///
    /// 热路径（版本匹配）：~5 条指令，零表查找
    /// 冷路径（版本过期）：重新读值 + 更新指令中的版本号
    pub(in crate::vm) fn op_get_global_cached(&mut self) -> Result<(), NuzoError> {
        let dest = self.read_u16()?;
        let gidx = self.read_u16()? as usize;
        let expected_ver = self.read_u16()? as u32;
        // 热路径：版本匹配 → 直接取值，零开销
        if gidx < self.cx.global_versions.len()
            && self.cx.global_versions[gidx] == expected_ver
            && let Some(value) = self.get_global(gidx)
        {
            return self.set_register(dest, value);
        }

        // 冷路径：版本过期 → 重新读值 + 更新指令中的版本号
        let value = self.get_global(gidx).ok_or_else(|| {
            self.error_with_source_location(NuzoErrorExt::index_out_of_bounds(
                gidx.to_string(),
                self.global_count().to_string(),
            ))
        })?;

        let new_ver = self.cx.global_versions.get(gidx).copied().unwrap_or(0);
        // 边界检查：new_ver (u32) 必须落在 u16 操作数域内，否则 `as u16` 截断
        // 会写入错误版本号，导致 GetGlobalCached 永远版本不匹配（死循环冷路径）。
        if new_ver > u16::MAX as u32 {
            return Err(NuzoError::internal(
                InternalError::GlobalIndexOverflow { idx: gidx, ver: new_ver },
                None,
            ));
        }
        // patch 版本号字段 (offset: 1(opcode)+2(dest)+2(gidx) = 5)
        let patch_ip = self.ip - 2; // 指向 version 字段起始
        self.patch_code(patch_ip, &(new_ver as u16).to_le_bytes())?;

        self.set_register(dest, value)
    }
}
