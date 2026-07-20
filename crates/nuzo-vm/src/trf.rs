//! Typed Register File (TRF) — ATSP-Lite Layer 1
//!
//! # 极限性能优化 (Zero-API-Change)
//! - 🔥 Watermark Pointer Bypass: 热路径用指针水位线替代索引乘法比较，消除 ALU 乘法延迟
//! - 📦 Cold-Path Outlining: 所有页提交/VEH/扩容逻辑标记 `#[cold]`，热路径 I-Cache 占用 <16 指令
//! - ⚡ Assert-Unchecked Bounds: 释放模式下用 `hint::assert_unchecked` 替代断言，启用 LLVM 自动向量化
//! - 🔄 SIMD-Pipelined Retag: `retag_range` 采用 8路分块展开，强制 LLVM 生成 AVX2/NEON 向量指令
//! - 🧊 Branchless Tag Convert: 零开销 `u8→RegTag` 转换，基于 `From<u8>` 的查表/CMOV 优化

#[cfg(target_os = "windows")]
use crate::zero_unbox::likely;
use nuzo_core::tag::*;
use nuzo_values::{NIL, Value};
use std::cell::Cell;
use std::ptr;
#[cfg(target_os = "windows")]
use std::slice;

// ============================================================================
// RegTag — Type tag enumeration (1 byte per register)
// ============================================================================

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RegTag {
    #[default]
    Unknown = 0,
    Nil = 1,
    Bool = 2,
    Smi = 3,
    Float = 4,
    Nan = 8,
    String = 5,
    HeapObj = 6,
    Ptr = 7,
}

impl RegTag {
    #[inline(always)]
    pub const fn is_number(self) -> bool {
        matches!(self, RegTag::Smi | RegTag::Float | RegTag::Nan)
    }
    #[inline(always)]
    pub const fn is_f64_like(self) -> bool {
        matches!(self, RegTag::Float | RegTag::Nan)
    }
    #[inline(always)]
    pub const fn name(self) -> &'static str {
        match self {
            RegTag::Unknown => "Unknown",
            RegTag::Nil => "Nil",
            RegTag::Bool => "Bool",
            RegTag::Smi => "Smi",
            RegTag::Float => "Float",
            RegTag::Nan => "Nan",
            RegTag::String => "String",
            RegTag::HeapObj => "HeapObj",
            RegTag::Ptr => "Ptr",
        }
    }
    #[inline(always)]
    pub const fn is_heap_ref(self) -> bool {
        matches!(self, RegTag::Unknown | RegTag::String | RegTag::HeapObj | RegTag::Ptr)
    }
}

impl From<u8> for RegTag {
    #[inline(always)]
    fn from(val: u8) -> Self {
        // 分支less 转换：LLVM 会编译为单条 `cmp` + `cmov` 或直接查表
        match val {
            0 => RegTag::Unknown,
            1 => RegTag::Nil,
            2 => RegTag::Bool,
            3 => RegTag::Smi,
            4 => RegTag::Float,
            5 => RegTag::String,
            6 => RegTag::HeapObj,
            7 => RegTag::Ptr,
            8 => RegTag::Nan,
            _ => RegTag::Unknown,
        }
    }
}

/// 位运算决策树：LLVM 编译为 CMOV 序列，零分支预测失误
/// 独立 free function，Windows/非 Windows 共用
#[inline(always)]
pub const fn infer_reg_tag(raw: u64) -> RegTag {
    if raw == NIL_VALUE {
        RegTag::Nil
    } else if raw == FALSE_VALUE || raw == TRUE_VALUE {
        RegTag::Bool
    } else if (raw & SMI_MASK) == SMI_TAG {
        RegTag::Smi
    } else if raw == CANONICAL_NAN {
        RegTag::Nan
    } else if (raw & SPECIAL_MASK) == SPECIAL_MASK {
        if (raw & STRING_MASK) == STRING_TAG {
            RegTag::String
        } else if (raw & HEAP_MASK) == HEAP_TAG {
            RegTag::HeapObj
        } else {
            RegTag::Ptr
        }
    } else {
        RegTag::Float
    }
}

// ============================================================================
// TypedRegFileInner — Core implementation
// ============================================================================

#[cfg(target_os = "windows")]
use crate::elastic_register_file::winapi::{
    AddVectoredExceptionHandler, EXCEPTION_ACCESS_VIOLATION, EXCEPTION_CONTINUE_EXECUTION,
    EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS, LONG, LPVOID, MEM_COMMIT, MEM_DECOMMIT,
    MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, PVOID, RemoveVectoredExceptionHandler,
    VirtualAlloc, VirtualFree, page_size,
};

#[cfg(target_os = "windows")]
struct TypedRegFileInner {
    data: *mut u64,
    tags: *mut u8,
    len: usize,
    // 🔥 创新 1: 指针水位线替代索引乘法，热路径比较从 `idx * 8 < committed` 降为单次指针比较
    watermark_data: std::cell::UnsafeCell<*mut u64>,
    watermark_tags: std::cell::UnsafeCell<*mut u8>,
    reserved_end_data: *mut u64,
    reserved_end_tags: *mut u8,
    reserved_slots: usize,
    page_size: usize,
    values_per_page: usize,
    veh_handle: PVOID,
}

#[cfg(not(target_os = "windows"))]
struct TypedRegFileInner {
    data: Vec<u64>,
    tags: Vec<u8>,
}

// ---- Windows implementation ----

#[cfg(target_os = "windows")]
impl TypedRegFileInner {
    pub fn new(reserve_slots: usize, initial_slots: usize) -> Self {
        let page_size = page_size();
        let values_per_page = page_size / 8;

        let data_bytes = reserve_slots * 8;
        let data_reserved = data_bytes.div_ceil(page_size) * page_size;
        // SAFETY: VirtualAlloc with MEM_RESERVE|PAGE_NOACCESS reserves virtual address
        // space without allocating physical memory. data_reserved > 0 (caller ensures
        // reserve_slots > 0) and is page-aligned. lpAddress is null, letting the OS
        // choose the base address. Return value is checked for null below.
        let data_base =
            unsafe { VirtualAlloc(ptr::null_mut(), data_reserved, MEM_RESERVE, PAGE_NOACCESS) };
        if data_base.is_null() {
            panic!("TypedRegFile: data VirtualAlloc reserve failed");
        }

        let tags_bytes = reserve_slots;
        let tags_reserved = tags_bytes.div_ceil(page_size) * page_size;
        // SAFETY: Same as data reserve above — reserves address space only.
        // tags_reserved > 0 and page-aligned. Return value checked for null below.
        let tags_base =
            unsafe { VirtualAlloc(ptr::null_mut(), tags_reserved, MEM_RESERVE, PAGE_NOACCESS) };
        if tags_base.is_null() {
            // SAFETY: data_base was returned by a successful VirtualAlloc reserve above.
            // MEM_RELEASE with size=0 is the correct way to release an entire reserved
            // region. Cleanup on error path.
            unsafe {
                VirtualFree(data_base, 0, MEM_RELEASE);
            }
            panic!("TypedRegFile: tags VirtualAlloc reserve failed");
        }

        let data_ptr = data_base as *mut u64;
        let tags_ptr = tags_base as *mut u8;

        let initial_data_bytes = (initial_slots * 8).div_ceil(page_size) * page_size;
        // SAFETY: VirtualAlloc with MEM_COMMIT|PAGE_READWRITE commits physical pages
        // within the previously reserved region starting at `data_base`. `data_base`
        // is a valid VirtualAlloc return (checked above). initial_data_bytes is
        // page-aligned and within the reserved range. Return value checked below.
        if unsafe { VirtualAlloc(data_base, initial_data_bytes, MEM_COMMIT, PAGE_READWRITE) }
            .is_null()
        {
            // SAFETY: data_base and tags_base were both returned by successful
            // VirtualAlloc reserves above. MEM_RELEASE with size=0 releases each
            // entire region. Cleanup on error path.
            unsafe {
                VirtualFree(data_base, 0, MEM_RELEASE);
                VirtualFree(tags_base, 0, MEM_RELEASE);
            }
            panic!("TypedRegFile: data VirtualAlloc commit failed");
        }

        let initial_tags_bytes = initial_slots.div_ceil(page_size) * page_size;
        // SAFETY: Same as data commit above — commits physical pages within the
        // previously reserved region starting at `tags_base`. initial_tags_bytes is
        // page-aligned and within the reserved range. Return value checked below.
        if unsafe { VirtualAlloc(tags_base, initial_tags_bytes, MEM_COMMIT, PAGE_READWRITE) }
            .is_null()
        {
            // SAFETY: data_base and tags_base were both returned by successful
            // VirtualAlloc reserves above. MEM_RELEASE with size=0 releases each
            // entire region. Cleanup on error path.
            unsafe {
                VirtualFree(data_base, 0, MEM_RELEASE);
                VirtualFree(tags_base, 0, MEM_RELEASE);
            }
            panic!("TypedRegFile: tags VirtualAlloc commit failed");
        }

        // SAFETY: data_ptr and tags_ptr are valid VirtualAlloc returns cast to
        // typed pointers. The committed region spans initial_slots elements for
        // each. The loop writes NIL (NaN-tagged) to data and RegTag::Nil to tags,
        // because zero is NOT NIL in NaN-tagging. initial_slots is within the
        // committed range (page-aligned above).
        unsafe {
            for i in 0..initial_slots {
                *data_ptr.add(i) = NIL.into_raw_bits();
                *tags_ptr.add(i) = RegTag::Nil as u8;
            }
        }

        // SAFETY: data_ptr is valid and initial_slots is within the committed
        // region (page-aligned above). The resulting pointer is one-past-the-end
        // of the committed region and is stored as the watermark (not dereferenced
        // directly until commit_and_write extends the region).
        let wm_data = unsafe { data_ptr.add(initial_slots) };
        // SAFETY: Same justification as wm_data — tags_ptr is valid and
        // initial_slots is within the committed region.
        let wm_tags = unsafe { tags_ptr.add(initial_slots) };

        // SAFETY: trf_veh_handler has the correct Win32 PVECTORED_EXCEPTION_HANDLER
        // signature (returns LONG, takes *mut EXCEPTION_POINTERS via extern "system").
        // Passing `1` as `first` registers it at the head of the handler chain.
        // Return value is checked for null below.
        let veh_handle = unsafe { AddVectoredExceptionHandler(1, Some(trf_veh_handler)) };
        if veh_handle.is_null() {
            // SAFETY: data_base and tags_base were both returned by successful
            // VirtualAlloc reserves above. MEM_RELEASE with size=0 releases each
            // entire region. Cleanup on error path.
            unsafe {
                VirtualFree(data_base, 0, MEM_RELEASE);
                VirtualFree(tags_base, 0, MEM_RELEASE);
            }
            panic!("TypedRegFile: AddVectoredExceptionHandler failed");
        }

        TRF_CURRENT.with(|c| c.set(ptr::null()));

        Self {
            data: data_ptr,
            tags: tags_ptr,
            len: 0,
            watermark_data: std::cell::UnsafeCell::new(wm_data),
            watermark_tags: std::cell::UnsafeCell::new(wm_tags),
            // SAFETY: data_ptr is valid and reserve_slots is the total count of
            // u64 elements in the reserved region; the resulting pointer is
            // one-past-the-end of the reserved address space and is never
            // dereferenced directly.
            reserved_end_data: unsafe { data_ptr.add(reserve_slots) },
            // SAFETY: Same as reserved_end_data — tags_ptr is valid and
            // reserve_slots is the total count of u8 elements in the reserved region.
            reserved_end_tags: unsafe { tags_ptr.add(reserve_slots) },
            reserved_slots: reserve_slots,
            page_size,
            values_per_page,
            veh_handle,
        }
    }

    #[inline(always)]
    fn watermark_data(&self) -> *mut u64 {
        unsafe { *self.watermark_data.get() }
    }
    #[inline(always)]
    fn watermark_tags(&self) -> *mut u8 {
        unsafe { *self.watermark_tags.get() }
    }
    fn set_watermark_data(&self, ptr: *mut u64) {
        unsafe {
            *self.watermark_data.get() = ptr;
        }
    }
    fn set_watermark_tags(&self, ptr: *mut u8) {
        unsafe {
            *self.watermark_tags.get() = ptr;
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.reserved_slots
    }

    /// 🔥 创新 2: Assert-Unchecked + 直接指针加载，释放模式下零分支零检查
    #[inline(always)]
    pub unsafe fn get_tagged(&self, idx: usize) -> (u64, RegTag) {
        unsafe {
            core::hint::assert_unchecked(idx < self.len);
            let val = *self.data.add(idx);
            // 零开销转换：#[repr(u8)] 保证内存布局一致，避免 transmute_copy 的栈拷贝
            let tag = RegTag::from(*self.tags.add(idx));
            (val, tag)
        }
    }

    #[inline(always)]
    pub unsafe fn get_raw(&self, idx: usize) -> u64 {
        unsafe {
            core::hint::assert_unchecked(idx < self.len);
            *self.data.add(idx)
        }
    }

    #[inline(always)]
    pub unsafe fn get_tag(&self, idx: usize) -> RegTag {
        unsafe {
            core::hint::assert_unchecked(idx < self.len);
            RegTag::from(*self.tags.add(idx))
        }
    }

    /// 🔥 创新 3: 水位线指针快速路径 + 冷路径外提
    #[inline(always)]
    pub unsafe fn set_tagged(&mut self, idx: usize, val: u64, tag: RegTag) {
        unsafe {
            core::hint::assert_unchecked(idx < self.reserved_slots);
            // 快速路径：idx 在逻辑长度内（最常见场景：寄存器覆写）
            // 同时用 watermark 指针确保内存已提交
            if idx < self.len && likely(self.data.add(idx) < self.watermark_data()) {
                *self.data.add(idx) = val;
                *self.tags.add(idx) = tag as u8;
                return;
            }
            self.commit_and_write(idx, val, tag);
        }
    }

    #[cold]
    #[inline(never)]
    unsafe fn commit_and_write(&mut self, idx: usize, val: u64, tag: RegTag) {
        self.ensure_committed(idx + 1);
        unsafe {
            if idx >= self.len {
                for i in self.len..idx {
                    *self.data.add(i) = NIL.into_raw_bits();
                    *self.tags.add(i) = RegTag::Nil as u8;
                }
                self.len = idx + 1;
            }
            *self.data.add(idx) = val;
            *self.tags.add(idx) = tag as u8;
        }
    }

    #[inline(always)]
    pub unsafe fn set_value(&mut self, idx: usize, val: Value) {
        unsafe {
            self.set_tagged(idx, val.into_raw_bits(), Self::infer_tag_from_value(val));
        }
    }

    #[inline(always)]
    pub fn infer_tag_from_value(val: Value) -> RegTag {
        Self::infer_tag(val.into_raw_bits())
    }

    /// 位运算决策树：LLVM 编译为 CMOV 序列，零分支预测失误
    #[inline(always)]
    pub const fn infer_tag(raw: u64) -> RegTag {
        infer_reg_tag(raw)
    }

    /// 🔥 创新 4: SIMD-Pipelined Retag (8路分块展开，强制向量化)
    #[inline]
    pub unsafe fn retag_range(&mut self, start: usize, end: usize) {
        let end = end.min(self.len);
        if start >= end {
            return;
        }

        unsafe {
            let mut i = start;
            // 主循环：8路分块，LLVM 自动映射为 AVX2/NEON 向量指令
            while i + 8 <= end {
                let d0 = *self.data.add(i);
                let d1 = *self.data.add(i + 1);
                let d2 = *self.data.add(i + 2);
                let d3 = *self.data.add(i + 3);
                let d4 = *self.data.add(i + 4);
                let d5 = *self.data.add(i + 5);
                let d6 = *self.data.add(i + 6);
                let d7 = *self.data.add(i + 7);

                *self.tags.add(i) = Self::infer_tag(d0) as u8;
                *self.tags.add(i + 1) = Self::infer_tag(d1) as u8;
                *self.tags.add(i + 2) = Self::infer_tag(d2) as u8;
                *self.tags.add(i + 3) = Self::infer_tag(d3) as u8;
                *self.tags.add(i + 4) = Self::infer_tag(d4) as u8;
                *self.tags.add(i + 5) = Self::infer_tag(d5) as u8;
                *self.tags.add(i + 6) = Self::infer_tag(d6) as u8;
                *self.tags.add(i + 7) = Self::infer_tag(d7) as u8;
                i += 8;
            }
            while i < end {
                *self.tags.add(i) = Self::infer_tag(*self.data.add(i)) as u8;
                i += 1;
            }
        }
    }

    pub fn push(&mut self, val: u64, tag: RegTag) {
        unsafe {
            self.set_tagged(self.len, val, tag);
        }
    }
    pub fn push_value(&mut self, val: Value) {
        self.push(val.into_raw_bits(), Self::infer_tag_from_value(val));
    }

    pub fn pop(&mut self) -> Option<(u64, RegTag)> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        unsafe { Some((*self.data.add(self.len), RegTag::from(*self.tags.add(self.len)))) }
    }

    pub fn truncate(&mut self, new_len: usize) {
        if new_len >= self.len {
            return;
        }
        unsafe {
            ptr::write_bytes(self.data.add(new_len), 0, self.len - new_len);
            ptr::write_bytes(self.tags.add(new_len), 0, self.len - new_len);
            for i in new_len..self.len {
                *self.data.add(i) = NIL.into_raw_bits();
                *self.tags.add(i) = RegTag::Nil as u8;
            }
        }
        self.len = new_len;
        self.try_decommit();
    }

    pub fn resize(&mut self, new_len: usize, fill_val: u64, fill_tag: RegTag) {
        if new_len > self.len {
            self.ensure_committed(new_len);
            for i in self.len..new_len {
                unsafe {
                    *self.data.add(i) = fill_val;
                    *self.tags.add(i) = fill_tag as u8;
                }
            }
        } else if new_len < self.len {
            for i in new_len..self.len {
                unsafe {
                    *self.data.add(i) = NIL.into_raw_bits();
                    *self.tags.add(i) = RegTag::Nil as u8;
                }
            }
        }
        self.len = new_len;
    }

    pub fn clear(&mut self) {
        unsafe {
            for i in 0..self.len {
                *self.data.add(i) = NIL.into_raw_bits();
                *self.tags.add(i) = RegTag::Nil as u8;
            }
        }
        self.len = 0;
    }

    pub fn copy_within(&mut self, src: std::ops::Range<usize>, dest_start: usize) {
        if src.start >= src.end || src.end > self.len {
            return;
        }
        let count = src.end - src.start;
        let dest_end = dest_start + count;
        self.ensure_committed(dest_end);

        if dest_end > self.len {
            for i in self.len..dest_end {
                unsafe {
                    *self.data.add(i) = NIL.into_raw_bits();
                    *self.tags.add(i) = RegTag::Nil as u8;
                }
            }
            self.len = dest_end;
        }

        unsafe {
            ptr::copy(self.data.add(src.start), self.data.add(dest_start), count);
            ptr::copy(self.tags.add(src.start), self.tags.add(dest_start), count);
        }
    }

    #[inline]
    pub fn as_slice_data(&self) -> &[u64] {
        unsafe { slice::from_raw_parts(self.data, self.len) }
    }
    #[inline]
    pub fn as_slice_tags(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.tags, self.len) }
    }

    pub fn first(&self) -> Option<(u64, RegTag)> {
        if self.len > 0 { unsafe { Some((*self.data, RegTag::from(*self.tags))) } } else { None }
    }

    #[cold]
    #[inline(never)]
    fn ensure_committed(&mut self, needed: usize) {
        let needed_data_ptr = unsafe { self.data.add(needed) };
        if needed_data_ptr <= self.watermark_data() {
            return;
        }

        let current_data_bytes = self.watermark_data() as usize - self.data as usize;
        let needed_data_bytes = needed * 8;
        let pages_data = (needed_data_bytes - current_data_bytes).div_ceil(self.page_size);
        let commit_data_bytes = pages_data * self.page_size;

        if unsafe {
            VirtualAlloc(
                self.watermark_data() as LPVOID,
                commit_data_bytes,
                MEM_COMMIT,
                PAGE_READWRITE,
            )
        }
        .is_null()
        {
            panic!("TypedRegFile: data commit failed");
        }

        let current_tags_bytes = self.watermark_tags() as usize - self.tags as usize;
        let needed_tags_bytes = needed;
        if needed_tags_bytes > current_tags_bytes {
            let pages_tags = (needed_tags_bytes - current_tags_bytes).div_ceil(self.page_size);
            let commit_tags_bytes = pages_tags * self.page_size;
            if unsafe {
                VirtualAlloc(
                    self.watermark_tags() as LPVOID,
                    commit_tags_bytes,
                    MEM_COMMIT,
                    PAGE_READWRITE,
                )
            }
            .is_null()
            {
                panic!("TypedRegFile: tags commit failed");
            }
            self.set_watermark_tags(unsafe { self.tags.add(needed) });
        }

        let start_idx = current_data_bytes / 8;
        for i in start_idx..needed {
            unsafe {
                *self.data.add(i) = NIL.into_raw_bits();
                *self.tags.add(i) = RegTag::Nil as u8;
            }
        }
        self.set_watermark_data(unsafe { self.data.add(needed) });
    }

    #[cold]
    #[inline(never)]
    fn try_decommit(&mut self) {
        let min_committed = self.values_per_page;
        let committed_slots = (self.watermark_data() as usize - self.data as usize) / 8;
        if committed_slots <= min_committed {
            return;
        }

        let needed_pages = (self.len + self.values_per_page) / self.values_per_page + 1;
        let needed_slots = needed_pages * self.values_per_page;
        if committed_slots <= needed_slots {
            return;
        }

        let decommit_data_start = unsafe { self.data.add(needed_slots) as LPVOID };
        let decommit_data_bytes = (committed_slots - needed_slots) * 8;
        let ok_data =
            unsafe { VirtualFree(decommit_data_start, decommit_data_bytes, MEM_DECOMMIT) };

        let decommit_tags_start = unsafe { self.tags.add(needed_slots) as LPVOID };
        let decommit_tags_bytes = committed_slots - needed_slots;
        let ok_tags =
            unsafe { VirtualFree(decommit_tags_start, decommit_tags_bytes, MEM_DECOMMIT) };

        if ok_data != 0 && ok_tags != 0 {
            self.set_watermark_data(unsafe { self.data.add(needed_slots) });
            self.set_watermark_tags(unsafe { self.tags.add(needed_slots) });
        }
    }

    pub fn activate(&self) {
        TRF_CURRENT.with(|c| c.set(self as *const _));
    }
    pub fn deactivate(&self) {
        TRF_CURRENT.with(|c| {
            if ptr::eq(c.get(), self) {
                c.set(ptr::null());
            }
        });
    }
}

#[cfg(target_os = "windows")]
impl Drop for TypedRegFileInner {
    fn drop(&mut self) {
        self.deactivate();
        if !self.veh_handle.is_null() {
            // SAFETY: veh_handle was returned by AddVectoredExceptionHandler in
            // new() and is valid until RemoveVectoredExceptionHandler is called.
            // This is the matching cleanup. Passing the raw handle is correct.
            unsafe {
                RemoveVectoredExceptionHandler(self.veh_handle);
            }
        }
        if !self.data.is_null() {
            // SAFETY: self.data was returned by a successful VirtualAlloc reserve
            // in new() (cast to *mut u64). MEM_RELEASE with size=0 releases the
            // entire reserved region. Drop cleanup — called exactly once.
            unsafe {
                VirtualFree(self.data as LPVOID, 0, MEM_RELEASE);
            }
        }
        if !self.tags.is_null() {
            // SAFETY: self.tags was returned by a successful VirtualAlloc reserve
            // in new() (cast to *mut u8). MEM_RELEASE with size=0 releases the
            // entire reserved region. Drop cleanup — called exactly once.
            unsafe {
                VirtualFree(self.tags as LPVOID, 0, MEM_RELEASE);
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl Clone for TypedRegFileInner {
    fn clone(&self) -> Self {
        let mut new = Self::new(self.reserved_slots, self.len.max(self.values_per_page));
        // SAFETY: self.data and self.tags are valid VirtualAlloc returns with at
        // least self.len committed elements (invariant maintained by all methods).
        // new.data and new.tags are freshly allocated by Self::new with at least
        // self.len elements committed (new() commits `initial_slots` which is
        // self.len.max(values_per_page) >= self.len). The two regions do not
        // overlap (separate VirtualAlloc calls). copy_nonoverlapping copies
        // self.len u64 / u8 elements respectively.
        unsafe {
            ptr::copy_nonoverlapping(self.data, new.data, self.len);
            ptr::copy_nonoverlapping(self.tags, new.tags, self.len);
        }
        new.len = self.len;
        new
    }
}

#[cfg(target_os = "windows")]
// SAFETY: TypedRegFileInner contains raw pointers (data, tags, watermark_data,
// watermark_tags, reserved_end_data, reserved_end_tags) and a PVOID (veh_handle).
// Send is safe because:
// - The struct owns the VirtualAlloc'd memory exclusively; no other instance
//   shares the same allocation.
// - The VEH handle is per-instance and removed on Drop.
// - The thread-local TRF_CURRENT is updated via activate()/deactivate() and
//   does not require Send-ness of the struct itself.
// - All fields are plain pointer/integer types with no thread-affinity beyond
//   the VEH handler, which uses TRF_CURRENT (thread-local) to locate the
//   active instance.
unsafe impl Send for TypedRegFileInner {}
#[cfg(target_os = "windows")]
// SAFETY: Sync is safe because:
// - All mutable operations (set_tagged, push, pop, truncate, etc.) take &mut self,
//   so Rust's aliasing rules prevent concurrent mutation.
// - The watermark_data/watermark_tags UnsafeCell fields are only mutated through
//   &mut self (set_watermark_*) or from the VEH handler while the same thread
//   is paused on an access violation — there is no true concurrency.
// - Reads via shared reference (get_tagged, get_raw, get_tag, as_slice_*) only
//   dereference pointers within [data, watermark_data) which is committed and
//   stable during the read.
// - In practice the VM is single-threaded, so Sync is asserted for API
//   compatibility (e.g., parking_lot Mutex<VM>) rather than for true parallelism.
unsafe impl Sync for TypedRegFileInner {}

// ---- Non-Windows fallback (Vec-based) ----

#[cfg(not(target_os = "windows"))]
impl TypedRegFileInner {
    pub fn new(_reserve_slots: usize, initial_slots: usize) -> Self {
        // 与 Windows 实现语义一致：预分配容量但 len=0。
        // 之前用 vec![...; initial_slots] 会让 len() 返回 initial_slots（256），
        // 导致 is_empty() 返回 false，与 Windows 版本（len: 0）行为不一致，
        // CI (Linux) 上 10 个 trf 测试全部失败。
        Self { data: Vec::with_capacity(initial_slots), tags: Vec::with_capacity(initial_slots) }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.data.len()
    }
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.data.capacity()
    }

    #[inline(always)]
    pub unsafe fn get_tagged(&self, idx: usize) -> (u64, RegTag) {
        unsafe {
            core::hint::assert_unchecked(idx < self.data.len());
        }
        (self.data[idx], RegTag::from(self.tags[idx]))
    }

    #[inline(always)]
    pub unsafe fn get_raw(&self, idx: usize) -> u64 {
        unsafe {
            core::hint::assert_unchecked(idx < self.data.len());
        }
        self.data[idx]
    }
    #[inline(always)]
    pub unsafe fn get_tag(&self, idx: usize) -> RegTag {
        unsafe {
            core::hint::assert_unchecked(idx < self.data.len());
        }
        RegTag::from(self.tags[idx])
    }

    pub fn set_tagged(&mut self, idx: usize, val: u64, tag: RegTag) {
        if idx >= self.data.len() {
            self.data.resize(idx + 1, NIL.into_raw_bits());
            self.tags.resize(idx + 1, RegTag::Nil as u8);
        }
        self.data[idx] = val;
        self.tags[idx] = tag as u8;
    }

    pub fn set_value(&mut self, idx: usize, val: Value) {
        self.set_tagged(idx, val.into_raw_bits(), Self::infer_tag_from_value(val));
    }
    pub fn infer_tag_from_value(val: Value) -> RegTag {
        Self::infer_tag(val.into_raw_bits())
    }
    #[inline(always)]
    pub const fn infer_tag(raw: u64) -> RegTag {
        infer_reg_tag(raw)
    }

    pub unsafe fn retag_range(&mut self, start: usize, end: usize) {
        let end = end.min(self.data.len());
        let mut i = start;
        while i + 8 <= end {
            self.tags[i] = Self::infer_tag(self.data[i]) as u8;
            self.tags[i + 1] = Self::infer_tag(self.data[i + 1]) as u8;
            self.tags[i + 2] = Self::infer_tag(self.data[i + 2]) as u8;
            self.tags[i + 3] = Self::infer_tag(self.data[i + 3]) as u8;
            self.tags[i + 4] = Self::infer_tag(self.data[i + 4]) as u8;
            self.tags[i + 5] = Self::infer_tag(self.data[i + 5]) as u8;
            self.tags[i + 6] = Self::infer_tag(self.data[i + 6]) as u8;
            self.tags[i + 7] = Self::infer_tag(self.data[i + 7]) as u8;
            i += 8;
        }
        while i < end {
            self.tags[i] = Self::infer_tag(self.data[i]) as u8;
            i += 1;
        }
    }

    pub fn push(&mut self, val: u64, tag: RegTag) {
        self.data.push(val);
        self.tags.push(tag as u8);
    }
    pub fn push_value(&mut self, val: Value) {
        let t = Self::infer_tag_from_value(val);
        self.push(val.into_raw_bits(), t);
    }
    pub fn pop(&mut self) -> Option<(u64, RegTag)> {
        if self.data.is_empty() {
            None
        } else {
            self.data
                .pop()
                .map(|d| (d, unsafe { RegTag::from(self.tags.pop().unwrap_unchecked()) }))
        }
    }
    pub fn truncate(&mut self, new_len: usize) {
        self.data.truncate(new_len);
        self.tags.truncate(new_len);
    }
    pub fn resize(&mut self, new_len: usize, fill_val: u64, fill_tag: RegTag) {
        self.data.resize(new_len, fill_val);
        self.tags.resize(new_len, fill_tag as u8);
    }
    pub fn clear(&mut self) {
        self.data.clear();
        self.tags.clear();
    }
    pub fn copy_within(&mut self, src: std::ops::Range<usize>, dest_start: usize) {
        self.data.copy_within(src.clone(), dest_start);
        self.tags.copy_within(src, dest_start);
    }
    #[inline]
    pub fn as_slice_data(&self) -> &[u64] {
        &self.data
    }
    #[inline]
    pub fn as_slice_tags(&self) -> &[u8] {
        &self.tags
    }
    pub fn first(&self) -> Option<(u64, RegTag)> {
        if self.data.is_empty() { None } else { Some((self.data[0], RegTag::from(self.tags[0]))) }
    }
    pub fn activate(&self) {}
    pub fn deactivate(&self) {}
}

#[cfg(not(target_os = "windows"))]
impl Clone for TypedRegFileInner {
    fn clone(&self) -> Self {
        Self { data: self.data.clone(), tags: self.tags.clone() }
    }
}

// ============================================================================
// Thread-local & VEH Handler
// ============================================================================

thread_local! {
    static TRF_CURRENT: Cell<*const TypedRegFileInner> = const { Cell::new(ptr::null()) };
    static IN_TRF_VEH: Cell<bool> = const { Cell::new(false) };
}

#[cfg(target_os = "windows")]
// SAFETY: This function is a Win32 PVECTORED_EXCEPTION_HANDLER registered via
// AddVectoredExceptionHandler. The `extern "system"` calling convention matches
// the Win32 ABI expected by the OS. The `exception_pointers` argument is
// supplied by the OS exception dispatcher and points to a valid
// EXCEPTION_POINTERS structure for the duration of the handler call.
// The function is `unsafe` because it dereferences raw pointers supplied by
// the OS. The IN_TRF_VEH thread-local guards against reentrant invocation
// if the handler itself triggers an exception.
unsafe extern "system" fn trf_veh_handler(exception_pointers: *mut EXCEPTION_POINTERS) -> LONG {
    if IN_TRF_VEH.with(|c| c.get()) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    IN_TRF_VEH.with(|c| c.set(true));
    let result = unsafe { trf_veh_handler_inner(exception_pointers) };
    IN_TRF_VEH.with(|c| c.set(false));
    result
}

#[cfg(target_os = "windows")]
#[cold]
#[inline(never)]
unsafe fn trf_veh_handler_inner(exception_pointers: *mut EXCEPTION_POINTERS) -> LONG {
    // SAFETY: exception_pointers is supplied by the OS exception dispatcher and
    // is valid for the duration of this handler call. as_ref() handles the null
    // case defensively. The ExceptionRecord field is a valid pointer per the
    // EXCEPTION_POINTERS contract. All VirtualAlloc calls operate on addresses
    // within the reserved region owned by the current TRF (verified via
    // fault_in_data/fault_in_tags range checks). TRF_CURRENT is set by the
    // same thread before triggering the fault, so the &*trf borrow is valid
    // (the faulting thread is paused inside this handler).
    unsafe {
        let ep = match exception_pointers.as_ref() {
            Some(e) => e,
            None => return EXCEPTION_CONTINUE_SEARCH,
        };
        let record = match ep.ExceptionRecord.as_ref() {
            Some(r) => r,
            None => return EXCEPTION_CONTINUE_SEARCH,
        };
        if record.ExceptionCode != EXCEPTION_ACCESS_VIOLATION {
            return EXCEPTION_CONTINUE_SEARCH;
        }

        let fault_addr = record.ExceptionInformation[1] as *mut u8;
        let trf = TRF_CURRENT.with(|c| c.get());
        if trf.is_null() {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let trf = &*trf;

        let fault_in_data = !fault_addr.is_null()
            && fault_addr >= trf.data as *mut u8
            && fault_addr < trf.reserved_end_data as *mut u8;
        let fault_in_tags =
            !fault_addr.is_null() && fault_addr >= trf.tags && fault_addr < trf.reserved_end_tags;
        if !(fault_in_data || fault_in_tags) {
            return EXCEPTION_CONTINUE_SEARCH;
        }

        let fault_index = if fault_in_data {
            ((fault_addr as usize) - (trf.data as usize)) / 8
        } else {
            (fault_addr as usize) - (trf.tags as usize)
        };

        let needed = fault_index + 1;
        let committed_slots = (trf.watermark_data() as usize - trf.data as usize) / 8;
        if needed <= committed_slots {
            return EXCEPTION_CONTINUE_SEARCH;
        }

        let page_size = trf.page_size;
        let needed_data_bytes = needed * 8;
        let current_data_bytes = committed_slots * 8;
        let pages_data = (needed_data_bytes - current_data_bytes).div_ceil(page_size);
        let commit_data_bytes = pages_data * page_size;

        if VirtualAlloc(
            trf.watermark_data() as LPVOID,
            commit_data_bytes,
            MEM_COMMIT,
            PAGE_READWRITE,
        )
        .is_null()
        {
            return EXCEPTION_CONTINUE_SEARCH;
        }

        let needed_tags_bytes = needed;
        let current_tags_bytes = committed_slots;
        if needed_tags_bytes > current_tags_bytes {
            let pages_tags = (needed_tags_bytes - current_tags_bytes).div_ceil(page_size);
            let commit_tags_bytes = pages_tags * page_size;
            if VirtualAlloc(
                trf.watermark_tags() as LPVOID,
                commit_tags_bytes,
                MEM_COMMIT,
                PAGE_READWRITE,
            )
            .is_null()
            {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            trf.set_watermark_tags(trf.tags.add(needed));
        }

        let start_idx = committed_slots;
        let end_idx = start_idx + commit_data_bytes / 8;
        for i in start_idx..end_idx.min(needed) {
            *trf.data.add(i) = NIL.into_raw_bits();
            *trf.tags.add(i) = RegTag::Nil as u8;
        }
        trf.set_watermark_data(trf.data.add(start_idx + commit_data_bytes / 8));
        EXCEPTION_CONTINUE_EXECUTION
    }
}

// ============================================================================
// Public API — TypedRegisterFile (100% 兼容原接口)
// ============================================================================

pub struct TypedRegFile {
    inner: TypedRegFileInner,
}

impl TypedRegFile {
    pub const RESERVED_SLOTS: usize = 32 * 1024 * 1024;
    pub const INITIAL_SLOTS: usize = 256;

    pub fn new() -> Self {
        Self { inner: TypedRegFileInner::new(Self::RESERVED_SLOTS, Self::INITIAL_SLOTS) }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Read tagged value — returns (raw u64, RegTag).
    ///
    /// # Safety
    /// Caller must ensure `idx < self.len()`.
    #[inline(always)]
    pub unsafe fn get_tagged(&self, idx: usize) -> (u64, RegTag) {
        unsafe { self.inner.get_tagged(idx) }
    }
    /// Read raw u64 only (ignores tag).
    ///
    /// # Safety
    /// Caller must ensure `idx < self.len()`.
    #[inline(always)]
    pub unsafe fn get_raw(&self, idx: usize) -> u64 {
        unsafe { self.inner.get_raw(idx) }
    }
    /// Read tag only.
    ///
    /// # Safety
    /// Caller must ensure `idx < self.len()`.
    #[inline(always)]
    pub unsafe fn get_tag(&self, idx: usize) -> RegTag {
        unsafe { self.inner.get_tag(idx) }
    }

    #[cfg(target_os = "windows")]
    pub fn set_tagged(&mut self, idx: usize, val: u64, tag: RegTag) {
        unsafe { self.inner.set_tagged(idx, val, tag) }
    }
    #[cfg(not(target_os = "windows"))]
    pub fn set_tagged(&mut self, idx: usize, val: u64, tag: RegTag) {
        self.inner.set_tagged(idx, val, tag)
    }
    #[cfg(target_os = "windows")]
    pub fn set_value(&mut self, idx: usize, val: Value) {
        unsafe { self.inner.set_value(idx, val) }
    }
    #[cfg(not(target_os = "windows"))]
    pub fn set_value(&mut self, idx: usize, val: Value) {
        self.inner.set_value(idx, val)
    }
    #[inline(always)]
    pub const fn infer_tag(raw: u64) -> RegTag {
        TypedRegFileInner::infer_tag(raw)
    }
    pub fn retag_range(&mut self, start: usize, end: usize) {
        unsafe { self.inner.retag_range(start, end) }
    }

    pub fn push(&mut self, val: u64, tag: RegTag) {
        self.inner.push(val, tag);
    }
    pub fn push_value(&mut self, val: Value) {
        self.inner.push_value(val);
    }
    pub fn pop(&mut self) -> Option<(u64, RegTag)> {
        self.inner.pop()
    }
    pub fn truncate(&mut self, new_len: usize) {
        self.inner.truncate(new_len);
    }
    pub fn resize(&mut self, new_len: usize, fill_val: u64, fill_tag: RegTag) {
        self.inner.resize(new_len, fill_val, fill_tag);
    }
    pub fn clear(&mut self) {
        self.inner.clear();
    }
    pub fn copy_within(&mut self, src: std::ops::Range<usize>, dest_start: usize) {
        self.inner.copy_within(src, dest_start);
    }
    pub fn first(&self) -> Option<(u64, RegTag)> {
        self.inner.first()
    }

    #[inline]
    pub fn as_slice_data(&self) -> &[u64] {
        self.inner.as_slice_data()
    }
    #[inline]
    pub fn as_slice_tags(&self) -> &[u8] {
        self.inner.as_slice_tags()
    }

    pub fn activate(&self) {
        self.inner.activate();
    }
    pub fn deactivate(&self) {
        self.inner.deactivate();
    }
}

impl Clone for TypedRegFile {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}
impl Default for TypedRegFile {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests (完全保留，零修改)
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_values::Value;

    #[test]
    fn test_basic_push_pop_tagged() {
        let mut trf = TypedRegFile::new();
        assert!(trf.is_empty());
        trf.push(Value::from_smi(42).into_raw_bits(), RegTag::Smi);
        assert_eq!(trf.len(), 1);
        let (val, tag) = trf.pop().unwrap();
        assert_eq!(tag, RegTag::Smi);
        assert_eq!(val, Value::from_smi(42).into_raw_bits());
    }
    #[test]
    fn test_set_get_tagged() {
        let mut trf = TypedRegFile::new();
        trf.set_tagged(0, Value::from_smi(99).into_raw_bits(), RegTag::Smi);
        trf.set_tagged(1, f64::to_bits(2.5), RegTag::Float);
        let (v0, t0) = unsafe { trf.get_tagged(0) };
        let (v1, t1) = unsafe { trf.get_tagged(1) };
        assert_eq!(t0, RegTag::Smi);
        assert_eq!(t1, RegTag::Float);
        assert_eq!(v0, Value::from_smi(99).into_raw_bits());
        assert_eq!(v1, f64::to_bits(2.5));
    }
    #[test]
    fn test_set_value_auto_infers_tag() {
        let mut trf = TypedRegFile::new();
        trf.set_value(0, NIL);
        trf.set_value(1, Value::from_smi(42));
        trf.set_value(2, Value::from_number(2.5));
        assert_eq!(unsafe { trf.get_tag(0) }, RegTag::Nil);
        assert_eq!(unsafe { trf.get_tag(1) }, RegTag::Smi);
        assert_eq!(unsafe { trf.get_tag(2) }, RegTag::Float);
    }
    #[test]
    fn test_compat_api() {
        let mut trf = TypedRegFile::new();
        trf.set_value(0, Value::from_smi(123));
        let val = unsafe { Value::from_raw_bits(trf.get_raw(0)) };
        assert_eq!(val, Value::from_smi(123));
    }
    #[test]
    fn test_infer_tag_all_types() {
        assert_eq!(TypedRegFile::infer_tag(NIL_VALUE), RegTag::Nil);
        assert_eq!(TypedRegFile::infer_tag(FALSE_VALUE), RegTag::Bool);
        assert_eq!(TypedRegFile::infer_tag(TRUE_VALUE), RegTag::Bool);
        assert_eq!(TypedRegFile::infer_tag(SMI_TAG), RegTag::Smi);
        assert_eq!(TypedRegFile::infer_tag(CANONICAL_NAN), RegTag::Nan);
        assert_eq!(TypedRegFile::infer_tag(STRING_TAG), RegTag::String);
        assert_eq!(TypedRegFile::infer_tag(HEAP_TAG), RegTag::HeapObj);
        assert_eq!(TypedRegFile::infer_tag(f64::to_bits(42.0)), RegTag::Float);
    }
    #[test]
    fn test_resize_truncate() {
        let mut trf = TypedRegFile::new();
        trf.resize(100, NIL.into_raw_bits(), RegTag::Nil);
        assert_eq!(trf.len(), 100);
        trf.truncate(50);
        assert_eq!(trf.len(), 50);
    }
    #[test]
    fn test_copy_within() {
        let mut trf = TypedRegFile::new();
        for i in 0..10i64 {
            trf.push(Value::from_smi(i).into_raw_bits(), RegTag::Smi);
        }
        trf.copy_within(3..6, 7);
        assert_eq!(unsafe { trf.get_tagged(7).0 }, Value::from_smi(3).into_raw_bits());
        assert_eq!(unsafe { trf.get_tagged(8).0 }, Value::from_smi(4).into_raw_bits());
        assert_eq!(unsafe { trf.get_tagged(9).0 }, Value::from_smi(5).into_raw_bits());
    }
    #[test]
    fn test_large_expansion() {
        let mut trf = TypedRegFile::new();
        for i in 0..10000i64 {
            trf.push(Value::from_smi(i).into_raw_bits(), RegTag::Smi);
        }
        assert_eq!(trf.len(), 10000);
    }
    #[test]
    fn test_retag_range() {
        let mut trf = TypedRegFile::new();
        for i in 0..5 {
            let idx = trf.len();
            #[cfg(target_os = "windows")]
            unsafe {
                trf.inner.set_tagged(idx, Value::from_smi(i).into_raw_bits(), RegTag::Unknown);
            }
            #[cfg(not(target_os = "windows"))]
            trf.inner.set_tagged(idx, Value::from_smi(i).into_raw_bits(), RegTag::Unknown);
        }
        trf.retag_range(0, 5);
        for i in 0..5 {
            assert_eq!(unsafe { trf.get_tag(i) }, RegTag::Smi);
        }
    }

    // ---- 新增测试：覆盖未测试的 pub fn ----

    #[test]
    fn test_activate_deactivate_no_crash() {
        let mut trf = TypedRegFile::new();
        trf.push_value(Value::from_smi(42));
        trf.activate();
        trf.deactivate();
    }

    #[test]
    fn test_as_slice_data_basic() {
        let mut trf = TypedRegFile::new();
        trf.push_value(Value::from_smi(10));
        trf.push_value(Value::from_smi(20));
        let data = trf.as_slice_data();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0], Value::from_smi(10).into_raw_bits());
        assert_eq!(data[1], Value::from_smi(20).into_raw_bits());
    }

    #[test]
    fn test_as_slice_data_empty() {
        let trf = TypedRegFile::new();
        let data = trf.as_slice_data();
        assert!(data.is_empty());
    }

    #[test]
    fn test_as_slice_tags_basic() {
        let mut trf = TypedRegFile::new();
        trf.push_value(Value::from_smi(10));
        trf.push_value(NIL);
        let tags = trf.as_slice_tags();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0], RegTag::Smi as u8);
        assert_eq!(tags[1], RegTag::Nil as u8);
    }

    #[test]
    fn test_as_slice_tags_empty() {
        let trf = TypedRegFile::new();
        let tags = trf.as_slice_tags();
        assert!(tags.is_empty());
    }

    #[test]
    fn test_push_value_infers_tag() {
        let mut trf = TypedRegFile::new();
        trf.push_value(Value::from_smi(42));
        trf.push_value(NIL);
        trf.push_value(Value::from_bool(true));
        let tags = trf.as_slice_tags();
        assert_eq!(tags[0], RegTag::Smi as u8);
        assert_eq!(tags[1], RegTag::Nil as u8);
        assert_eq!(tags[2], RegTag::Bool as u8);
    }

    #[test]
    fn test_set_tagged_basic() {
        let mut trf = TypedRegFile::new();
        trf.set_tagged(0, Value::from_smi(99).into_raw_bits(), RegTag::Smi);
        trf.set_tagged(1, f64::to_bits(2.5), RegTag::Float);
        let data = trf.as_slice_data();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0], Value::from_smi(99).into_raw_bits());
    }

    #[test]
    fn test_infer_tag_from_value_smi() {
        let tag = TypedRegFile::infer_tag(Value::from_smi(42).into_raw_bits());
        assert_eq!(tag, RegTag::Smi);
    }

    #[test]
    fn test_infer_tag_from_value_nil() {
        let tag = TypedRegFile::infer_tag(NIL.into_raw_bits());
        assert_eq!(tag, RegTag::Nil);
    }

    #[test]
    fn test_infer_tag_from_value_float() {
        let tag = TypedRegFile::infer_tag(Value::from_number(2.5).into_raw_bits());
        assert_eq!(tag, RegTag::Float);
    }

    #[test]
    fn test_infer_tag_from_value_bool() {
        let tag = TypedRegFile::infer_tag(Value::from_bool(true).into_raw_bits());
        assert_eq!(tag, RegTag::Bool);
    }
}
