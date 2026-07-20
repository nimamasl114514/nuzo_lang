//! Elastic Register File — Memory-mapped register storage with guard-page auto-expansion.
//!
//! # Innovation
//!
//! Applies the "elastic stack" concept to the VM's register file:
//! - **VirtualAlloc** reserves a large address space (256 MB) with zero physical cost
//! - **Guard pages** trigger automatic expansion via VEH on ACCESS_VIOLATION
//! - **No arbitrary limits**: the only bound is system memory
//! - **Physical memory efficiency**: decommit pages when the stack shrinks
//!
//! # Architecture
//!
//! ```text
//! High address
//! ┌──────────────────────┐ ← reserved_end
//! │   Reserved (NOACCESS) │   ← Not yet committed, zero physical cost
//! │   ...                  │
//! ├──────────────────────┤ ← guard_page (PROT_NONE / PAGE_NOACCESS)
//! │   Guard Page           │   ← Triggers VEH → auto-commit + fill NIL
//! ├──────────────────────┤ ← committed_end
//! │   Committed (RW)       │   ← Active registers, physical pages backed
//! │   ... Values ...       │
//! │   [0] [1] ... [len-1] │
//! └──────────────────────┘ ← base
//! Low address
//! ```
//!
//! # Safety
//!
//! - Raw pointer manipulation is encapsulated within this module
//! - All unsafe blocks are annotated with safety invariants
//! - VEH handler only touches memory within the reserved region
//! - NIL initialization guarantees no uninitialized Value escapes

use nuzo_core::Value;

use nuzo_values::NIL;
#[cfg(target_os = "windows")]
use std::cell::Cell;
#[cfg(target_os = "windows")]
use std::ptr;
#[cfg(target_os = "windows")]
use std::slice;

// ============================================================================
// Windows API FFI Bindings (minimal, no external crate needed)
// ============================================================================

#[cfg(target_os = "windows")]
#[allow(non_camel_case_types, non_snake_case, dead_code)]
pub(crate) mod winapi {
    use std::ffi::c_void;

    #[allow(clippy::upper_case_acronyms)] // Windows API FFI 约定，必须与 Win32 类型名一致
    pub type DWORD = u32;
    pub type SIZE_T = usize;
    #[allow(clippy::upper_case_acronyms)] // Windows API FFI 约定，必须与 Win32 类型名一致
    pub type LPVOID = *mut c_void;
    #[allow(clippy::upper_case_acronyms)] // Windows API FFI 约定，必须与 Win32 类型名一致
    pub type HANDLE = *mut c_void;
    #[allow(clippy::upper_case_acronyms)] // Windows API FFI 约定，必须与 Win32 类型名一致
    pub type LONG = i32;
    #[allow(clippy::upper_case_acronyms)] // Windows API FFI 约定，必须与 Win32 类型名一致
    pub type PVOID = *mut c_void;

    // Memory allocation constants
    pub const MEM_RESERVE: DWORD = 0x00002000;
    pub const MEM_COMMIT: DWORD = 0x00001000;
    pub const MEM_RELEASE: DWORD = 0x00008000;
    pub const MEM_DECOMMIT: DWORD = 0x00004000;
    pub const MEM_RESET: DWORD = 0x00080000;

    // Protection constants
    pub const PAGE_NOACCESS: DWORD = 0x01;
    pub const PAGE_READWRITE: DWORD = 0x04;
    pub const PAGE_GUARD: DWORD = 0x100;

    // Exception constants
    pub const EXCEPTION_ACCESS_VIOLATION: DWORD = 0xC0000005;
    pub const EXCEPTION_CONTINUE_SEARCH: LONG = 0;
    pub const EXCEPTION_CONTINUE_EXECUTION: LONG = -1;

    // VirtualAlloc
    unsafe extern "system" {
        pub fn VirtualAlloc(
            lpAddress: LPVOID,
            dwSize: SIZE_T,
            flAllocationType: DWORD,
            flProtect: DWORD,
        ) -> LPVOID;

        pub fn VirtualProtect(
            lpAddress: LPVOID,
            dwSize: SIZE_T,
            flNewProtect: DWORD,
            lpflOldProtect: *mut DWORD,
        ) -> i32;

        pub fn VirtualFree(lpAddress: LPVOID, dwSize: SIZE_T, dwFreeType: DWORD) -> i32;

        pub fn GetSystemInfo(lpSystemInfo: *mut SYSTEM_INFO);
    }

    // VEH
    pub type PVECTORED_EXCEPTION_HANDLER =
        Option<unsafe extern "system" fn(*mut EXCEPTION_POINTERS) -> LONG>;

    unsafe extern "system" {
        pub fn AddVectoredExceptionHandler(
            first: DWORD,
            handler: PVECTORED_EXCEPTION_HANDLER,
        ) -> PVOID;

        pub fn RemoveVectoredExceptionHandler(handle: PVOID) -> u32;
    }

    // Structures
    #[repr(C)]
    pub struct EXCEPTION_POINTERS {
        pub ExceptionRecord: *mut EXCEPTION_RECORD,
        pub ContextRecord: *mut c_void, // CONTEXT — we don't need the details
    }

    #[repr(C)]
    pub struct EXCEPTION_RECORD {
        pub ExceptionCode: DWORD,
        pub ExceptionFlags: DWORD,
        pub ExceptionRecord: *mut EXCEPTION_RECORD,
        pub ExceptionAddress: LPVOID,
        pub NumberParameters: DWORD,
        pub ExceptionInformation: [usize; 2], // EXCEPTION_MAXIMUM_PARAMETERS = 2 for ACCESS_VIOLATION
    }

    #[repr(C)]
    pub struct SYSTEM_INFO {
        pub wProcessorArchitecture: u16,
        pub wReserved: u16,
        pub dwPageSize: DWORD,
        pub lpMinimumApplicationAddress: LPVOID,
        pub lpMaximumApplicationAddress: LPVOID,
        pub dwActiveProcessorMask: usize,
        pub dwNumberOfProcessors: DWORD,
        pub dwProcessorType: DWORD,
        pub dwAllocationGranularity: DWORD,
        pub wProcessorLevel: u16,
        pub wProcessorRevision: u16,
    }

    pub fn page_size() -> usize {
        // SAFETY: SYSTEM_INFO is a plain C struct containing only integer and pointer
        // types valid as zero; GetSystemInfo writes to all fields, so the zeroed value
        // is never observed. std::mem::zeroed() is safe here.
        unsafe {
            let mut info: SYSTEM_INFO = std::mem::zeroed();
            GetSystemInfo(&mut info);
            info.dwPageSize as usize
        }
    }
}

// ============================================================================
// Thread-local: current ElasticRegisterFile for VEH handler
// ============================================================================

#[cfg(target_os = "windows")]
thread_local! {
    static CURRENT_ERF: Cell<*const ElasticRegisterFileInner> = const { Cell::new(std::ptr::null()) };
    static IN_VEH_HANDLER: Cell<bool> = const { Cell::new(false) };
}

// ============================================================================
// ElasticRegisterFileInner — Windows implementation
// ============================================================================

#[cfg(target_os = "windows")]
struct ElasticRegisterFileInner {
    base: *mut Value,
    len: usize,
    /// First non-committed address (guard page starts here).
    /// Wrapped in UnsafeCell because the VEH handler mutates this through a shared
    /// reference — the handler runs on the same thread while the VM is paused.
    committed_end: std::cell::UnsafeCell<*mut Value>,
    reserved_end: *mut Value, // end of entire reserved region
    reserved_slots: usize,
    page_size: usize,
    values_per_page: usize,
    veh_handle: Option<winapi::PVOID>,
}

#[cfg(target_os = "windows")]
impl ElasticRegisterFileInner {
    /// Create a new elastic register file.
    ///
    /// - `reserve_slots`: total virtual address space to reserve (e.g., 32M slots = 256MB)
    /// - `initial_slots`: initial committed region (e.g., 256 slots)
    pub fn new(reserve_slots: usize, initial_slots: usize) -> Self {
        let page_size = winapi::page_size();
        let values_per_page = page_size / std::mem::size_of::<Value>();

        let reserve_bytes = reserve_slots * std::mem::size_of::<Value>();
        let initial_pages = initial_slots.div_ceil(values_per_page);
        let initial_bytes = initial_pages * page_size;

        // 1. Reserve the entire virtual address space
        // SAFETY: VirtualAlloc with MEM_RESERVE|PAGE_NOACCESS reserves address space
        // without allocating physical memory. reserve_bytes > 0 (caller ensures
        // reserve_slots > 0). lpAddress is null, letting the OS choose the base
        // address. Return value is checked for null below.
        let base = unsafe {
            winapi::VirtualAlloc(
                std::ptr::null_mut(),
                reserve_bytes,
                winapi::MEM_RESERVE,
                winapi::PAGE_NOACCESS,
            )
        };

        if base.is_null() {
            panic!("ElasticRegisterFile: VirtualAlloc reserve failed for {} bytes", reserve_bytes);
        }

        let base_ptr = base as *mut Value;

        // 2. Commit the initial region (high-address end, since stack grows down conceptually,
        //    but our register file grows UP from base)
        // SAFETY: VirtualAlloc with MEM_COMMIT|PAGE_READWRITE commits physical pages
        // within the previously reserved region starting at `base`. `base` is a valid
        // VirtualAlloc return (checked above). `initial_bytes` is page-aligned and
        // within the reserved range. Return value is checked for null below.
        let commit_ok = unsafe {
            winapi::VirtualAlloc(base, initial_bytes, winapi::MEM_COMMIT, winapi::PAGE_READWRITE)
        };

        if commit_ok.is_null() {
            // SAFETY: `base` was returned by a successful VirtualAlloc reserve above.
            // MEM_RELEASE with size=0 is the correct way to release an entire region.
            // This is cleanup on an error path — we must free the reserved memory.
            unsafe {
                winapi::VirtualFree(base, 0, winapi::MEM_RELEASE);
            }
            panic!("ElasticRegisterFile: VirtualAlloc commit failed for {} bytes", initial_bytes);
        }

        // 3. Fill the initial committed region with NIL
        // SAFETY: base_ptr is a valid VirtualAlloc return cast to *mut Value. The
        // committed region spans initial_bytes bytes (page-aligned, verified non-null
        // above). write_bytes zeroes the memory first, then each slot is written with
        // NIL because zero is not NIL in NaN-tagging. The count
        // `initial_bytes / size_of::<Value>()` is exact because initial_bytes is
        // page-aligned and page_size >= size_of::<Value>().
        unsafe {
            ptr::write_bytes(base_ptr, 0, initial_bytes / std::mem::size_of::<Value>());
            // Re-initialize with NIL (zero is NOT NIL in NaN-tagging)
            for i in 0..(initial_bytes / std::mem::size_of::<Value>()) {
                *base_ptr.add(i) = NIL;
            }
        }

        // SAFETY: base_ptr is valid and the offset `initial_bytes / size_of::<Value>()`
        // is within the committed region (count of Values in initial_bytes bytes).
        let committed_end = unsafe { base_ptr.add(initial_bytes / std::mem::size_of::<Value>()) };
        // SAFETY: base_ptr is valid and reserve_slots is the total count of Values
        // in the reserved region; the resulting pointer is one-past-the-end of the
        // reserved address space and is never dereferenced directly.
        let reserved_end = unsafe { base_ptr.add(reserve_slots) };

        // 4. Set guard page at the end of committed region
        //    The page just after committed_end is already PAGE_NOACCESS (reserved but not committed)
        //    So we don't need to explicitly set a guard page — the reserved region acts as the guard

        // 5. Register VEH handler
        // SAFETY: elastic_veh_handler has the correct Win32 PVECTORED_EXCEPTION_HANDLER
        // signature (returns LONG, takes *mut EXCEPTION_POINTERS). Passing `1` as
        // `first` registers it at the head of the handler chain. Return value is
        // checked for null below.
        let veh_handle = unsafe {
            winapi::AddVectoredExceptionHandler(
                1, // first in chain
                Some(elastic_veh_handler),
            )
        };

        if veh_handle.is_null() {
            // SAFETY: `base` was returned by a successful VirtualAlloc reserve above.
            // MEM_RELEASE with size=0 releases the entire region. Cleanup on error path.
            unsafe {
                winapi::VirtualFree(base, 0, winapi::MEM_RELEASE);
            }
            panic!("ElasticRegisterFile: AddVectoredExceptionHandler failed");
        }

        Self {
            base: base_ptr,
            len: 0,
            committed_end: std::cell::UnsafeCell::new(committed_end),
            reserved_end,
            reserved_slots: reserve_slots,
            page_size,
            values_per_page,
            veh_handle: Some(veh_handle),
        }
    }

    /// Read the current committed_end pointer.
    #[inline(always)]
    fn committed_end(&self) -> *mut Value {
        // SAFETY: No concurrent mutation — VEH handler only runs while VM is paused
        // on the same thread, and normal code doesn't write committed_end during reads.
        unsafe { *self.committed_end.get() }
    }

    /// Write the committed_end pointer.
    #[inline(always)]
    fn set_committed_end(&self, ptr: *mut Value) {
        // SAFETY: Same justification as committed_end() — single-threaded access.
        unsafe { *self.committed_end.get() = ptr };
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

    /// Get value at index (with bounds check in debug mode).
    #[inline(always)]
    pub fn get(&self, index: usize) -> Option<Value> {
        if index < self.len {
            // SAFETY: index < len, and all slots up to len are in committed memory
            Some(unsafe { *self.base.add(index) })
        } else {
            None
        }
    }

    /// Get value at index without bounds check.
    ///
    /// # Safety
    /// Caller must ensure `index < self.len()`.
    #[inline(always)]
    pub unsafe fn get_unchecked(&self, index: usize) -> Value {
        // SAFETY: Caller guarantees index < self.len, so the slot is within committed
        // memory and has been initialized.
        unsafe { *self.base.add(index) }
    }

    /// Set value at index. Auto-expands if index >= len.
    pub fn set(&mut self, index: usize, value: Value) {
        self.ensure_committed(index + 1);
        // SAFETY: ensure_committed guarantees the memory is accessible
        unsafe {
            *self.base.add(index) = value;
        }
        if index >= self.len {
            // Fill gap with NIL
            for i in self.len..index {
                // SAFETY: ensure_committed(index + 1) was called above, so slots
                // self.len..index+1 are committed. i < index, so i is within range.
                unsafe {
                    *self.base.add(i) = NIL;
                }
            }
            self.len = index + 1;
        }
    }

    /// Set value at index without bounds check.
    ///
    /// # Safety
    /// Caller must ensure `index < self.len()`.
    #[inline(always)]
    pub unsafe fn set_unchecked(&mut self, index: usize, value: Value) {
        // SAFETY: Caller guarantees index < self.len, so the slot is within committed memory.
        unsafe {
            *self.base.add(index) = value;
        }
    }

    /// Push a value to the end.
    #[inline]
    pub fn push(&mut self, value: Value) {
        self.ensure_committed(self.len + 1);
        // SAFETY: ensure_committed guarantees the memory is accessible
        unsafe {
            *self.base.add(self.len) = value;
        }
        self.len += 1;
    }

    /// Pop the last value.
    pub fn pop(&mut self) -> Option<Value> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        // SAFETY: len was > 0, so len is a valid index
        Some(unsafe { *self.base.add(self.len) })
    }

    /// Truncate to new length.
    pub fn truncate(&mut self, new_len: usize) {
        if new_len >= self.len {
            return;
        }
        // Zero out truncated region for GC safety
        // SAFETY: new_len < self.len (checked above), so base.add(new_len) is valid.
        // The range [new_len, self.len) is within committed memory (slots up to
        // self.len were previously written). write_bytes zeroes the slots, then each
        // is re-filled with NIL because zero is not NIL in NaN-tagging.
        unsafe {
            ptr::write_bytes(self.base.add(new_len), 0, self.len - new_len);
            // Re-fill with NIL (zero is not NIL)
            for i in new_len..self.len {
                *self.base.add(i) = NIL;
            }
        }
        self.len = new_len;
        // Optionally decommit unused pages
        self.try_decommit();
    }

    /// Resize the register file.
    pub fn resize(&mut self, new_len: usize, value: Value) {
        if new_len > self.len {
            self.ensure_committed(new_len);
            for i in self.len..new_len {
                // SAFETY: ensure_committed(new_len) guarantees slots up to new_len
                // are committed. i < new_len, so the write is in range.
                unsafe {
                    *self.base.add(i) = value;
                }
            }
        } else if new_len < self.len {
            // SAFETY: new_len < self.len (checked above), so [new_len, self.len)
            // is within committed memory. NIL-filling for GC safety before shrinking.
            unsafe {
                for i in new_len..self.len {
                    *self.base.add(i) = NIL;
                }
            }
        }
        self.len = new_len;
    }

    /// Clear all registers.
    pub fn clear(&mut self) {
        // Fill with NIL for safety
        // SAFETY: All slots [0, self.len) are committed and initialized. NIL-filling
        // before resetting len to 0 prevents stale values from being observed by GC.
        unsafe {
            for i in 0..self.len {
                *self.base.add(i) = NIL;
            }
        }
        self.len = 0;
    }

    /// Copy values within the register file (like Vec::copy_within).
    pub fn copy_within(&mut self, src_start: usize, src_end: usize, dest_start: usize) {
        if src_start >= src_end || src_end > self.len {
            return;
        }
        let count = src_end - src_start;
        let dest_end = dest_start + count;
        self.ensure_committed(dest_end);

        // Ensure len covers the destination
        if dest_end > self.len {
            for i in self.len..dest_end {
                // SAFETY: ensure_committed(dest_end) was called above, so slots up
                // to dest_end are committed. i < dest_end, so the write is in range.
                unsafe {
                    *self.base.add(i) = NIL;
                }
            }
            self.len = dest_end;
        }

        // SAFETY: src and dest are within committed memory, ptr::copy handles overlap
        unsafe {
            ptr::copy(self.base.add(src_start), self.base.add(dest_start), count);
        }
    }

    /// Get a slice of the logical contents.
    #[inline]
    pub fn as_slice(&self) -> &[Value] {
        // SAFETY: base is valid for len elements, all initialized
        unsafe { slice::from_raw_parts(self.base, self.len) }
    }

    /// Get a mutable slice of the logical contents.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [Value] {
        // SAFETY: base is valid for len elements, all initialized
        unsafe { slice::from_raw_parts_mut(self.base, self.len) }
    }

    /// Get the first value.
    pub fn first(&self) -> Option<Value> {
        if self.len > 0 {
            // SAFETY: self.len > 0 (checked above), so base[0] is committed and initialized.
            Some(unsafe { *self.base })
        } else {
            None
        }
    }

    /// Ensure pages up to `needed` slots are committed.
    fn ensure_committed(&mut self, needed: usize) {
        // SAFETY: self.base is a valid VirtualAlloc pointer. `needed` is bounded by
        // reserved_slots (callers never exceed that), so the resulting pointer is
        // within or one-past-the-end of the reserved region. This pointer is only
        // used for comparison, not dereferenced.
        let needed_ptr = unsafe { self.base.add(needed) };
        let committed_end = self.committed_end();
        if needed_ptr <= committed_end {
            return; // Already committed
        }

        // Calculate how many pages to commit
        let current_committed_bytes = committed_end as usize - self.base as usize;
        let needed_bytes = needed * std::mem::size_of::<Value>();
        let pages_to_commit = (needed_bytes - current_committed_bytes).div_ceil(self.page_size);
        let commit_bytes = pages_to_commit * self.page_size;

        let commit_start = committed_end as winapi::LPVOID;

        // SAFETY: commit_start is the current committed_end (within the reserved
        // region), commit_bytes is page-aligned and does not exceed the reserved
        // space. VirtualAlloc with MEM_COMMIT within a previously reserved region
        // is safe. Return value is checked for null below.
        let result = unsafe {
            winapi::VirtualAlloc(
                commit_start,
                commit_bytes,
                winapi::MEM_COMMIT,
                winapi::PAGE_READWRITE,
            )
        };

        if result.is_null() {
            panic!(
                "ElasticRegisterFile: failed to commit {} bytes at {:p}",
                commit_bytes, commit_start
            );
        }

        // Fill newly committed pages with NIL
        let new_values = commit_bytes / std::mem::size_of::<Value>();
        // SAFETY: Both committed_end and self.base point into the same VirtualAlloc
        // region, so offset_from is defined. committed_end >= base (monotonically
        // increasing), so the result is non-negative.
        let start_idx = unsafe { committed_end.offset_from(self.base) as usize };
        for i in start_idx..(start_idx + new_values) {
            // SAFETY: The VirtualAlloc commit above succeeded, so slots
            // [start_idx, start_idx + new_values) are now committed memory.
            // Writing NIL initializes the newly committed pages.
            unsafe {
                *self.base.add(i) = NIL;
            }
        }

        // SAFETY: committed_end is within the reserved region, and new_values slots
        // were just committed (VirtualAlloc succeeded), so the new pointer is within
        // the committed region.
        self.set_committed_end(unsafe { committed_end.add(new_values) });
    }

    /// Try to decommit pages that are no longer needed.
    fn try_decommit(&mut self) {
        // Keep at least 1 page committed
        let min_committed = self.values_per_page;
        let committed_end = self.committed_end();
        // SAFETY: Both committed_end and self.base point into the same VirtualAlloc
        // region, so offset_from is defined. committed_end >= base, result non-negative.
        let current_committed_slots = unsafe { committed_end.offset_from(self.base) as usize };

        if current_committed_slots <= min_committed {
            return;
        }

        // Calculate how many pages we actually need (len + 1 page buffer)
        let needed_pages = (self.len + self.values_per_page) / self.values_per_page + 1;
        let needed_slots = needed_pages * self.values_per_page;

        if current_committed_slots <= needed_slots {
            return; // Not much to save
        }

        // Decommit from needed_slots to current_committed_slots
        // SAFETY: needed_slots <= current_committed_slots (checked above), so
        // base.add(needed_slots) is within the committed region. The pointer is
        // only used as a VirtualFree argument, not dereferenced directly.
        let decommit_start = unsafe { self.base.add(needed_slots) as winapi::LPVOID };
        let decommit_bytes =
            (current_committed_slots - needed_slots) * std::mem::size_of::<Value>();

        // SAFETY: decommit_start is within the committed region, decommit_bytes is
        // page-aligned and does not exceed the committed range beyond needed_slots.
        // MEM_DECOMMIT releases physical pages while keeping the virtual address range.
        let ok =
            unsafe { winapi::VirtualFree(decommit_start, decommit_bytes, winapi::MEM_DECOMMIT) };

        if ok != 0 {
            // SAFETY: needed_slots <= current_committed_slots, so base.add(needed_slots)
            // is within the still-committed region after decommit.
            self.set_committed_end(unsafe { self.base.add(needed_slots) });
        }
    }

    /// Activate this register file for the current thread (for VEH handler).
    pub fn activate(&self) {
        CURRENT_ERF.with(|cell| cell.set(self));
    }

    /// Deactivate this register file (clear thread-local pointer).
    pub fn deactivate(&self) {
        CURRENT_ERF.with(|cell| {
            if std::ptr::eq(cell.get(), self) {
                cell.set(std::ptr::null());
            }
        });
    }
}

#[cfg(target_os = "windows")]
impl Drop for ElasticRegisterFileInner {
    fn drop(&mut self) {
        // Deactivate thread-local pointer
        self.deactivate();

        // Remove VEH handler
        if let Some(handle) = self.veh_handle.take() {
            // SAFETY: handle was returned by AddVectoredExceptionHandler in new(),
            // so it is a valid VEH registration handle. Removing it is safe at any
            // time and prevents the handler from being called after self is dropped.
            unsafe {
                winapi::RemoveVectoredExceptionHandler(handle);
            }
        }

        // Release all virtual memory
        if !self.base.is_null() {
            // SAFETY: self.base was returned by VirtualAlloc with MEM_RESERVE.
            // MEM_RELEASE with size=0 releases the entire reserved region and all
            // committed pages within it. After this call, the address space is
            // returned to the OS and self.base becomes dangling (set to null below).
            unsafe {
                winapi::VirtualFree(self.base as winapi::LPVOID, 0, winapi::MEM_RELEASE);
            }
            self.base = std::ptr::null_mut();
        }
    }
}

#[cfg(target_os = "windows")]
impl Clone for ElasticRegisterFileInner {
    fn clone(&self) -> Self {
        let mut new = Self::new(self.reserved_slots, self.len.max(self.values_per_page));
        // SAFETY: self.base is valid for self.len reads (all committed and initialized).
        // new.base is valid for self.len writes (new was just created with at least
        // self.len committed slots). The regions do not overlap (different VirtualAlloc
        // allocations).
        unsafe {
            ptr::copy_nonoverlapping(self.base, new.base, self.len);
        }
        new.len = self.len;
        new
    }
}

// SAFETY: `ElasticRegisterFileInner` owns raw pointers (`base`, `committed_end`
// via `UnsafeCell`, `reserved_end`) into a privately-managed VirtualAlloc region.
//
// `Send` is sound because the type carries no per-thread affinity that would
// make moving it between threads unsafe: ownership of `ElasticRegisterFile`
// (and thus `ElasticRegisterFileInner`) may be transferred across threads,
// provided `activate()`/`deactivate()` — which register/unregister the
// thread-local `CURRENT_ERF` pointer and the VEH handler — are invoked on the
// owning thread. No interior mutability is observable through a `&self` view
// from another thread: the only mutation path is the VEH handler, which reads
// the current thread's thread-local `CURRENT_ERF` pointer, and that pointer is
// `null` on any thread that has not called `activate()` itself.
//
// `Sync` is intentionally NOT implemented. Sharing `&ElasticRegisterFileInner`
// across threads would let another thread observe the `UnsafeCell`
// (`committed_end`) being mutated by the VEH handler running on the owning
// thread, which is undefined behavior. There is also no need for `Sync`:
// `ElasticRegisterFile` is held exclusively by `VM` as
// `pub(super) registers: ElasticRegisterFile` and is never wrapped in `Arc`
// or otherwise shared across threads.
#[cfg(target_os = "windows")]
unsafe impl Send for ElasticRegisterFileInner {}

// ============================================================================
// VEH Handler (Windows only)
// ============================================================================

#[cfg(target_os = "windows")]
unsafe extern "system" fn elastic_veh_handler(
    exception_pointers: *mut winapi::EXCEPTION_POINTERS,
) -> winapi::LONG {
    if IN_VEH_HANDLER.with(|cell| cell.get()) {
        return winapi::EXCEPTION_CONTINUE_SEARCH;
    }
    IN_VEH_HANDLER.with(|cell| cell.set(true));

    // SAFETY: elastic_veh_handler_inner requires the same preconditions as this
    // function (valid exception_pointers, single-threaded access). This function
    // is the only caller, and the IN_VEH_HANDLER guard prevents reentry.
    let result = unsafe { elastic_veh_handler_inner(exception_pointers) };

    IN_VEH_HANDLER.with(|cell| cell.set(false));
    result
}

#[cfg(target_os = "windows")]
unsafe fn elastic_veh_handler_inner(
    exception_pointers: *mut winapi::EXCEPTION_POINTERS,
) -> winapi::LONG {
    // SAFETY: This function is only invoked from `elastic_veh_handler`, which the
    // OS calls as a Vectored Exception Handler. The OS guarantees that
    // `exception_pointers` points to a valid `EXCEPTION_POINTERS` for the
    // duration of the handler. We only ever touch the current thread's
    // thread-local `CURRENT_ERF` pointer (set by `activate()` on this same
    // thread); we never dereference cross-thread state. Every pointer
    // arithmetic below is bounded by the reserved region `[base, reserved_end)`
    // validated before any dereference, and all size computations use
    // checked/saturating arithmetic so they cannot overflow even if upstream
    // invariants are violated.
    unsafe {
        let ep = match exception_pointers.as_ref() {
            Some(ep) => ep,
            None => return winapi::EXCEPTION_CONTINUE_SEARCH,
        };

        let record = match ep.ExceptionRecord.as_ref() {
            Some(r) => r,
            None => return winapi::EXCEPTION_CONTINUE_SEARCH,
        };

        if record.ExceptionCode != winapi::EXCEPTION_ACCESS_VIOLATION {
            return winapi::EXCEPTION_CONTINUE_SEARCH;
        }

        // ExceptionInformation[0]: 0 = read, 1 = write
        // ExceptionInformation[1]: fault address
        let fault_addr = record.ExceptionInformation[1] as *mut Value;

        let erf = CURRENT_ERF.with(|cell| cell.get());
        if erf.is_null() {
            return winapi::EXCEPTION_CONTINUE_SEARCH;
        }

        let erf = &*erf;

        if fault_addr < erf.base || fault_addr >= erf.reserved_end {
            return winapi::EXCEPTION_CONTINUE_SEARCH;
        }

        // Check if fault is in the uncommitted region (between committed_end and reserved_end)
        let committed_end = erf.committed_end();
        if fault_addr >= committed_end {
            let page_size = erf.page_size;
            let value_size = std::mem::size_of::<Value>();
            let fault_byte_addr = fault_addr as usize;
            let page_aligned = (fault_byte_addr / page_size) * page_size;
            let _page_start = page_aligned as winapi::LPVOID;

            // Calculate how many bytes to commit (from page_start to committed_end, plus extra buffer).
            // `fault_offset` is already bounded by the reserved-region check above, but the
            // checked/saturating arithmetic below defends against extreme fault addresses or
            // overflow attempts — it can never panic nor wrap.
            let base_addr = erf.base as usize;
            let committed_bytes = committed_end as usize - base_addr;
            let fault_offset = fault_byte_addr - base_addr;
            let needed_bytes = fault_offset
                .saturating_add(page_size.saturating_mul(4))
                .min(erf.reserved_slots.saturating_mul(value_size));

            // Explicit underflow guard: if `needed_bytes <= committed_bytes` there is
            // nothing new to commit (the fault address should not have raised
            // ACCESS_VIOLATION in that case), so defer to the next handler.
            let commit_bytes = match needed_bytes.checked_sub(committed_bytes) {
                Some(0) | None => return winapi::EXCEPTION_CONTINUE_SEARCH,
                Some(n) => n,
            };

            let result = winapi::VirtualAlloc(
                (base_addr + committed_bytes) as winapi::LPVOID,
                commit_bytes,
                winapi::MEM_COMMIT,
                winapi::PAGE_READWRITE,
            );

            if result.is_null() {
                // Failed to commit — let the default handler deal with it
                return winapi::EXCEPTION_CONTINUE_SEARCH;
            }

            // Fill newly committed pages with NIL.
            //
            // Invariant: the write range `[start_idx, start_idx + new_values)` lies
            // entirely within the just-committed region
            // `[committed_bytes, committed_bytes + commit_bytes)`:
            //   - `start_idx * value_size == committed_bytes` (exact, since
            //     `committed_bytes` and `commit_bytes` are page-aligned, hence
            //     multiples of `value_size` for any power-of-two `Value` size),
            //   - `new_values * value_size <= commit_bytes` (by integer division).
            //
            // The `debug_assert!` additionally guarantees the range stays within
            // `reserved_slots`, so `erf.base.add(i)` cannot escape the reserved
            // region. The assert is debug-only to avoid perf cost in release builds.
            let new_values = commit_bytes / value_size;
            let start_idx = committed_bytes / value_size;
            debug_assert!(
                start_idx.checked_add(new_values).is_some_and(|end| end <= erf.reserved_slots),
                "NIL fill range [{}, {}) exceeds reserved_slots={}",
                start_idx,
                start_idx + new_values,
                erf.reserved_slots
            );
            for i in start_idx..(start_idx + new_values) {
                *erf.base.add(i) = NIL;
            }

            let new_committed_end = erf.base.add(start_idx + new_values);
            erf.set_committed_end(new_committed_end);

            return winapi::EXCEPTION_CONTINUE_EXECUTION;
        }

        winapi::EXCEPTION_CONTINUE_SEARCH
    }
}

// ============================================================================
// ElasticRegisterFile — Public API (platform abstraction)
// ============================================================================

/// Elastic register file with memory-mapped backend (Windows) or Vec fallback.
///
/// On Windows, uses VirtualAlloc + VEH for guard-page auto-expansion.
/// On other platforms, falls back to a standard Vec.
#[cfg(all(target_os = "windows", not(feature = "no-veh")))]
pub struct ElasticRegisterFile {
    inner: ElasticRegisterFileInner,
}

#[cfg(any(not(target_os = "windows"), feature = "no-veh"))]
pub struct ElasticRegisterFile {
    // When VEH disabled or non-Windows: use Vec backend
    #[cfg(target_os = "windows")]
    inner: Vec<Value>,
    #[cfg(not(target_os = "windows"))]
    inner: Vec<Value>,
}

// ---- Windows implementation ----

impl Default for ElasticRegisterFile {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(target_os = "windows", not(feature = "no-veh")))]
impl ElasticRegisterFile {
    /// Reserved virtual space: 256 MB = 32M Values (8 bytes each)
    const RESERVED_SLOTS: usize = 32 * 1024 * 1024;
    /// Initial committed slots
    const INITIAL_SLOTS: usize = 256;

    pub fn new() -> Self {
        Self { inner: ElasticRegisterFileInner::new(Self::RESERVED_SLOTS, Self::INITIAL_SLOTS) }
    }

    /// Create with a custom initial capacity (config-driven).
    pub fn with_capacity(initial_slots: usize) -> Self {
        Self { inner: ElasticRegisterFileInner::new(Self::RESERVED_SLOTS, initial_slots) }
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

    #[inline(always)]
    pub fn get(&self, index: usize) -> Option<Value> {
        self.inner.get(index)
    }

    /// # Safety
    /// Caller must ensure `index` is within the committed range of the register file.
    #[inline(always)]
    pub unsafe fn get_unchecked(&self, index: usize) -> Value {
        // SAFETY: Caller guarantees index < self.len(), delegating the safety
        // requirement to the inner implementation.
        unsafe { self.inner.get_unchecked(index) }
    }

    pub fn set(&mut self, index: usize, value: Value) {
        self.inner.set(index, value)
    }

    /// # Safety
    /// Caller must ensure `index` is within the committed range of the register file.
    #[inline(always)]
    pub unsafe fn set_unchecked(&mut self, index: usize, value: Value) {
        // SAFETY: Caller guarantees index < self.len(), delegating the safety
        // requirement to the inner implementation.
        unsafe { self.inner.set_unchecked(index, value) }
    }

    #[inline]
    pub fn push(&mut self, value: Value) {
        self.inner.push(value)
    }

    pub fn pop(&mut self) -> Option<Value> {
        self.inner.pop()
    }

    pub fn truncate(&mut self, new_len: usize) {
        self.inner.truncate(new_len)
    }

    pub fn resize(&mut self, new_len: usize, value: Value) {
        self.inner.resize(new_len, value)
    }

    pub fn clear(&mut self) {
        self.inner.clear()
    }

    /// Copy values within the register file (Vec-compatible signature).
    /// Accepts `Range<usize>` to match `Vec::copy_within(Range<usize>, usize)`.
    pub fn copy_within(&mut self, src: std::ops::Range<usize>, dest_start: usize) {
        self.inner.copy_within(src.start, src.end, dest_start)
    }

    #[inline]
    pub fn as_slice(&self) -> &[Value] {
        self.inner.as_slice()
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [Value] {
        self.inner.as_mut_slice()
    }

    pub fn first(&self) -> Option<Value> {
        self.inner.first()
    }

    /// Activate for VEH handler (call before VM execution loop).
    pub fn activate(&self) {
        self.inner.activate()
    }

    /// Deactivate (call after VM execution loop).
    pub fn deactivate(&self) {
        self.inner.deactivate()
    }
}

#[cfg(all(target_os = "windows", not(feature = "no-veh")))]
impl Clone for ElasticRegisterFile {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

#[cfg(all(target_os = "windows", not(feature = "no-veh")))]
impl std::ops::Index<usize> for ElasticRegisterFile {
    type Output = Value;
    #[inline(always)]
    fn index(&self, index: usize) -> &Value {
        // SAFETY: caller ensures index < len
        unsafe { &*self.inner.base.add(index) }
    }
}

#[cfg(all(target_os = "windows", not(feature = "no-veh")))]
impl std::ops::IndexMut<usize> for ElasticRegisterFile {
    #[inline(always)]
    fn index_mut(&mut self, index: usize) -> &mut Value {
        // SAFETY: caller ensures index < len
        unsafe { &mut *self.inner.base.add(index) }
    }
}

#[cfg(all(target_os = "windows", not(feature = "no-veh")))]
impl std::ops::Deref for ElasticRegisterFile {
    type Target = [Value];
    #[inline]
    fn deref(&self) -> &[Value] {
        self.as_slice()
    }
}

#[cfg(all(target_os = "windows", not(feature = "no-veh")))]
impl std::ops::DerefMut for ElasticRegisterFile {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Value] {
        self.as_mut_slice()
    }
}

// ---- Non-Windows fallback (Vec) ----

#[cfg(not(target_os = "windows"))]
impl ElasticRegisterFile {
    const INITIAL_SLOTS: usize = 256;

    pub fn new() -> Self {
        Self { inner: Vec::with_capacity(Self::INITIAL_SLOTS) }
    }

    /// Create with a custom initial capacity (config-driven).
    pub fn with_capacity(initial_slots: usize) -> Self {
        Self { inner: Vec::with_capacity(initial_slots) }
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

    #[inline(always)]
    pub fn get(&self, index: usize) -> Option<Value> {
        self.inner.get(index).copied()
    }

    /// # Safety
    ///
    /// `index` must be less than `self.len()`. Accessing out-of-bounds
    /// is undefined behaviour and may read uninitialised memory.
    #[inline(always)]
    pub unsafe fn get_unchecked(&self, index: usize) -> Value {
        // SAFETY: Caller guarantees index < self.len(). Vec::get_unchecked
        // is safe for valid indices within the Vec's len.
        unsafe { *self.inner.get_unchecked(index) }
    }

    pub fn set(&mut self, index: usize, value: Value) {
        if index >= self.inner.len() {
            self.inner.resize(index + 1, NIL);
        }
        self.inner[index] = value;
    }

    /// # Safety
    ///
    /// `index` must be less than `self.len()`. Writing out-of-bounds
    /// is undefined behaviour and may corrupt memory.
    #[inline(always)]
    pub unsafe fn set_unchecked(&mut self, index: usize, value: Value) {
        // SAFETY: Caller guarantees index < self.len(). Vec::get_unchecked_mut
        // is safe for valid indices within the Vec's len.
        unsafe {
            *self.inner.get_unchecked_mut(index) = value;
        }
    }

    #[inline]
    pub fn push(&mut self, value: Value) {
        self.inner.push(value)
    }

    pub fn pop(&mut self) -> Option<Value> {
        self.inner.pop()
    }

    pub fn truncate(&mut self, new_len: usize) {
        self.inner.truncate(new_len)
    }

    pub fn resize(&mut self, new_len: usize, value: Value) {
        self.inner.resize(new_len, value)
    }

    pub fn clear(&mut self) {
        self.inner.clear()
    }

    /// Copy values within the register file (Vec-compatible signature).
    pub fn copy_within(&mut self, src: std::ops::Range<usize>, dest_start: usize) {
        self.inner.copy_within(src, dest_start)
    }

    #[inline]
    pub fn as_slice(&self) -> &[Value] {
        &self.inner
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [Value] {
        &mut self.inner
    }

    pub fn first(&self) -> Option<Value> {
        self.inner.first().copied()
    }

    pub fn activate(&self) {} // No-op on non-Windows
    pub fn deactivate(&self) {} // No-op on non-Windows
}

// ---- VEH-disabled fallback (Vec) for debugging ----
// When `feature = "no-veh"` is set, use simple Vec backend even on Windows

#[cfg(all(target_os = "windows", feature = "no-veh"))]
impl ElasticRegisterFile {
    const INITIAL_SLOTS: usize = 256;

    pub fn new() -> Self {
        Self { inner: Vec::with_capacity(Self::INITIAL_SLOTS) }
    }

    /// Create with a custom initial capacity (config-driven).
    pub fn with_capacity(initial_slots: usize) -> Self {
        Self { inner: Vec::with_capacity(initial_slots) }
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

    #[inline(always)]
    pub fn get(&self, index: usize) -> Option<Value> {
        self.inner.get(index).copied()
    }

    /// # Safety
    ///
    /// `index` must be less than `self.len()`. Accessing out-of-bounds
    /// is undefined behaviour and may read uninitialised memory.
    #[inline(always)]
    pub unsafe fn get_unchecked(&self, index: usize) -> Value {
        // SAFETY: Caller guarantees index < self.len(). Vec::get_unchecked
        // is safe for valid indices within the Vec's len.
        unsafe { *self.inner.get_unchecked(index) }
    }

    pub fn set(&mut self, index: usize, value: Value) {
        if index >= self.inner.len() {
            self.inner.resize(index + 1, NIL);
        }
        self.inner[index] = value;
    }

    /// # Safety
    ///
    /// `index` must be less than `self.len()`. Writing out-of-bounds
    /// is undefined behaviour and may corrupt memory.
    #[inline(always)]
    pub unsafe fn set_unchecked(&mut self, index: usize, value: Value) {
        // SAFETY: Caller guarantees index < self.len(). Vec::get_unchecked_mut
        // is safe for valid indices within the Vec's len.
        unsafe {
            *self.inner.get_unchecked_mut(index) = value;
        }
    }

    #[inline]
    pub fn push(&mut self, value: Value) {
        self.inner.push(value)
    }

    pub fn pop(&mut self) -> Option<Value> {
        self.inner.pop()
    }

    pub fn truncate(&mut self, new_len: usize) {
        self.inner.truncate(new_len)
    }

    pub fn resize(&mut self, new_len: usize, value: Value) {
        self.inner.resize(new_len, value)
    }

    pub fn clear(&mut self) {
        self.inner.clear()
    }

    /// Copy values within the register file (Vec-compatible signature).
    pub fn copy_within(&mut self, src: std::ops::Range<usize>, dest_start: usize) {
        self.inner.copy_within(src, dest_start)
    }

    #[inline]
    pub fn as_slice(&self) -> &[Value] {
        &self.inner
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [Value] {
        &mut self.inner
    }

    pub fn first(&self) -> Option<Value> {
        self.inner.first().copied()
    }

    pub fn activate(&self) {} // No-op when VEH disabled
    pub fn deactivate(&self) {} // No-op when VEH disabled
}

#[cfg(all(target_os = "windows", feature = "no-veh"))]
impl Clone for ElasticRegisterFile {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

#[cfg(all(target_os = "windows", feature = "no-veh"))]
impl std::ops::Deref for ElasticRegisterFile {
    type Target = [Value];
    #[inline]
    fn deref(&self) -> &[Value] {
        &self.inner
    }
}

#[cfg(all(target_os = "windows", feature = "no-veh"))]
impl std::ops::DerefMut for ElasticRegisterFile {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Value] {
        &mut self.inner
    }
}

#[cfg(all(target_os = "windows", feature = "no-veh"))]
impl std::ops::Index<usize> for ElasticRegisterFile {
    type Output = Value;
    #[inline(always)]
    fn index(&self, index: usize) -> &Value {
        &self.inner[index]
    }
}

#[cfg(all(target_os = "windows", feature = "no-veh"))]
impl std::ops::IndexMut<usize> for ElasticRegisterFile {
    #[inline(always)]
    fn index_mut(&mut self, index: usize) -> &mut Value {
        &mut self.inner[index]
    }
}

#[cfg(not(target_os = "windows"))]
impl Clone for ElasticRegisterFile {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

#[cfg(not(target_os = "windows"))]
impl std::ops::Deref for ElasticRegisterFile {
    type Target = [Value];
    #[inline]
    fn deref(&self) -> &[Value] {
        &self.inner
    }
}

#[cfg(not(target_os = "windows"))]
impl std::ops::DerefMut for ElasticRegisterFile {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Value] {
        &mut self.inner
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_values::NIL;

    #[test]
    fn test_basic_push_pop() {
        let mut rf = ElasticRegisterFile::new();
        assert!(rf.is_empty());
        assert_eq!(rf.len(), 0);

        rf.push(NIL);
        assert_eq!(rf.len(), 1);

        let val = rf.pop();
        assert!(val.is_some());
        assert!(rf.is_empty());
    }

    #[test]
    fn test_set_get() {
        let mut rf = ElasticRegisterFile::new();
        rf.push(NIL);
        rf.push(NIL);
        rf.set(0, Value::from_smi(42));
        rf.set(1, Value::from_smi(99));

        assert_eq!(rf.get(0), Some(Value::from_smi(42)));
        assert_eq!(rf.get(1), Some(Value::from_smi(99)));
        assert_eq!(rf.get(2), None);
    }

    #[test]
    fn test_resize() {
        let mut rf = ElasticRegisterFile::new();
        rf.resize(100, NIL);
        assert_eq!(rf.len(), 100);

        rf.resize(50, NIL);
        assert_eq!(rf.len(), 50);
    }

    #[test]
    fn test_truncate() {
        let mut rf = ElasticRegisterFile::new();
        for i in 0..10 {
            rf.push(Value::from_smi(i));
        }
        rf.truncate(5);
        assert_eq!(rf.len(), 5);
        assert_eq!(rf.get(0), Some(Value::from_smi(0)));
    }

    #[test]
    fn test_copy_within() {
        let mut rf = ElasticRegisterFile::new();
        for i in 0..10 {
            rf.push(Value::from_smi(i));
        }
        // Copy [3..6] to position 7
        rf.copy_within(3..6, 7);
        assert_eq!(rf.get(7), Some(Value::from_smi(3)));
        assert_eq!(rf.get(8), Some(Value::from_smi(4)));
        assert_eq!(rf.get(9), Some(Value::from_smi(5)));
    }

    #[test]
    fn test_large_expansion() {
        let mut rf = ElasticRegisterFile::new();
        // Push 10000 values — should trigger multiple page commits
        for i in 0..10000i64 {
            rf.push(Value::from_smi(i));
        }
        assert_eq!(rf.len(), 10000);
        assert_eq!(rf.get(9999), Some(Value::from_smi(9999)));
    }

    #[test]
    fn test_deref_slice() {
        let mut rf = ElasticRegisterFile::new();
        for i in 0..5 {
            rf.push(Value::from_smi(i));
        }
        let slice: &[Value] = &rf;
        assert_eq!(slice.len(), 5);
    }

    #[test]
    fn test_index_operator() {
        let mut rf = ElasticRegisterFile::new();
        for i in 0..5 {
            rf.push(Value::from_smi(i));
        }
        assert_eq!(rf[0], Value::from_smi(0));
        assert_eq!(rf[4], Value::from_smi(4));
    }

    // ---- 新增测试：覆盖未测试的 pub fn ----

    #[test]
    fn test_activate_deactivate_no_crash() {
        let mut rf = ElasticRegisterFile::new();
        rf.push(NIL);
        rf.activate();
        rf.deactivate();
    }

    #[test]
    fn test_as_slice_basic() {
        let mut rf = ElasticRegisterFile::new();
        rf.push(Value::from_smi(1));
        rf.push(Value::from_smi(2));
        rf.push(Value::from_smi(3));
        let slice = rf.as_slice();
        assert_eq!(slice.len(), 3);
        assert_eq!(slice[0], Value::from_smi(1));
        assert_eq!(slice[2], Value::from_smi(3));
    }

    #[test]
    fn test_as_slice_empty() {
        let rf = ElasticRegisterFile::new();
        let slice = rf.as_slice();
        assert!(slice.is_empty());
    }

    #[test]
    fn test_as_mut_slice_basic() {
        let mut rf = ElasticRegisterFile::new();
        rf.push(Value::from_smi(1));
        rf.push(Value::from_smi(2));
        let slice = rf.as_mut_slice();
        assert_eq!(slice.len(), 2);
        slice[0] = Value::from_smi(99);
        assert_eq!(rf.get(0), Some(Value::from_smi(99)));
    }

    #[test]
    fn test_as_mut_slice_empty() {
        let mut rf = ElasticRegisterFile::new();
        let slice = rf.as_mut_slice();
        assert!(slice.is_empty());
    }

    #[test]
    fn test_activate_idempotent() {
        let mut rf = ElasticRegisterFile::new();
        rf.push(NIL);
        rf.activate();
        rf.activate();
        rf.deactivate();
    }

    // ---- 补充覆盖空白：Default / with_capacity / capacity / 边界 ----

    #[test]
    fn test_default_equals_new() {
        let a = ElasticRegisterFile::new();
        let b = ElasticRegisterFile::default();
        assert_eq!(a.len(), b.len());
        assert_eq!(a.is_empty(), b.is_empty());
    }

    #[test]
    fn test_with_capacity_sets_initial_capacity() {
        let rf = ElasticRegisterFile::with_capacity(1024);
        assert!(rf.capacity() >= 1024, "capacity {} should be >= 1024", rf.capacity());
    }

    #[test]
    fn test_capacity_after_push() {
        let mut rf = ElasticRegisterFile::new();
        let initial_cap = rf.capacity();
        rf.push(Value::from_smi(1));
        assert!(rf.capacity() >= initial_cap, "capacity should not shrink after push");
    }

    #[test]
    fn test_pop_empty_returns_none() {
        let mut rf = ElasticRegisterFile::new();
        assert_eq!(rf.pop(), None);
        rf.push(Value::from_smi(42));
        assert_eq!(rf.pop(), Some(Value::from_smi(42)));
        assert_eq!(rf.pop(), None);
    }

    #[test]
    fn test_set_beyond_len_fills_gap_with_nil() {
        let mut rf = ElasticRegisterFile::new();
        rf.set(2, Value::from_smi(99));
        assert_eq!(rf.len(), 3);
        assert_eq!(rf.get(0), Some(NIL));
        assert_eq!(rf.get(1), Some(NIL));
        assert_eq!(rf.get(2), Some(Value::from_smi(99)));
    }

    #[test]
    fn test_clear_resets_to_empty() {
        let mut rf = ElasticRegisterFile::new();
        rf.push(Value::from_smi(1));
        rf.push(Value::from_smi(2));
        rf.push(Value::from_smi(3));
        assert_eq!(rf.len(), 3);
        rf.clear();
        assert!(rf.is_empty());
        assert_eq!(rf.len(), 0);
        assert_eq!(rf.get(0), None);
    }

    #[test]
    fn test_first_returns_head() {
        let mut rf = ElasticRegisterFile::new();
        assert_eq!(rf.first(), None);
        rf.push(Value::from_smi(99));
        rf.push(Value::from_smi(100));
        assert_eq!(rf.first(), Some(Value::from_smi(99)));
    }

    #[test]
    fn test_clone_preserves_state() {
        let mut rf = ElasticRegisterFile::new();
        rf.push(Value::from_smi(1));
        rf.push(Value::from_smi(2));
        let cloned = rf.clone();
        assert_eq!(cloned.len(), rf.len());
        assert_eq!(cloned.get(0), rf.get(0));
        assert_eq!(cloned.get(1), rf.get(1));
    }
}
