//! # 派发缓存类型定义
//!
//! 集中定义 GMVC (Global Multi-Versioned Cache)、PIC (Polymorphic Inline Cache)、
//! CSTS (Call Target Snapshot)、CDD (Closure Direct Dispatch) 相关的类型。
//!
//! 这些类型被 `dispatch` 模块外的代码（如 `vm.rs`、`vm_lic.rs`）通过
//! `crate::vm::dispatch::TypeName` 路径访问，因此本模块的所有公共类型
//! 由 `dispatch.rs` 通过 `pub(crate) use` 重导出。

use std::sync::Arc;

// ========================================================================
// 🧬 论文级创新 1: CIGC (Constant-Indexed Global Cache)
// 已升级为 GMVC —— 每个全局变量独立版本
// ========================================================================

/// 8 字节，恰好一个 cache line 可容纳 8 个 entry。
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct GlobalCacheEntry {
    /// 该全局变量的当前版本号，若与 global_versions[idx] 不同则失效
    pub(crate) version: u32,
    pub(crate) index: u32, // u32 足够（全局变量数 < 4B），保持 8B 对齐
}

// ========================================================================
// 🧬 论文级创新 2: ZOS-IC → PIC (Polymorphic Inline Cache)
// 每个 IC 槽点变为 4 路组相联
// ========================================================================
pub(crate) const PIC_WAYS: usize = 4;

/// 16 字节，4 个 entry 恰好 64 字节 = 1 cache line。
#[derive(Clone, Copy, Default, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct PropICEntry {
    pub ip: u32,
    pub prop_intern_id: u32,
    pub shape_id: u32,
    pub slot_index: u32,
}

/// 4 路组相联槽，附带简单 LRU 近似（上一次命中/更新的路号）。
///
/// 布局：64 bytes (ways) + 1 byte (lru) + 7 padding = 72 bytes。
/// 使用 `#[repr(C, align(64))]` 确保每个 slot 起始于 cache line 边界，
/// 避免 false sharing 并最大化顺序扫描吞吐。
#[derive(Clone, Copy)]
#[repr(C, align(64))]
pub(crate) struct PropICSlot {
    pub(crate) ways: [PropICEntry; PIC_WAYS],
    pub(crate) lru: u8, // 0..PIC_WAYS 表示最久未使用的路（近似）
}

impl Default for PropICSlot {
    #[inline(always)]
    fn default() -> Self {
        PropICSlot { ways: [PropICEntry::default(); PIC_WAYS], lru: 0 }
    }
}

#[allow(dead_code)]
impl PropICSlot {
    /// 快速查找：返回命中的 slot_index，同时更新 LRU。
    /// 展开 4 路比较，避免循环开销。
    #[inline(always)]
    pub(crate) fn lookup(&mut self, ip: u32, prop_id: u32, shape_id: u32) -> Option<u32> {
        // 手动展开 4 路，编译器可生成 4× cmov 序列
        let w = &self.ways;
        if w[0].ip == ip && w[0].prop_intern_id == prop_id && w[0].shape_id == shape_id {
            self.lru = 0;
            return Some(w[0].slot_index);
        }
        if w[1].ip == ip && w[1].prop_intern_id == prop_id && w[1].shape_id == shape_id {
            self.lru = 1;
            return Some(w[1].slot_index);
        }
        if w[2].ip == ip && w[2].prop_intern_id == prop_id && w[2].shape_id == shape_id {
            self.lru = 2;
            return Some(w[2].slot_index);
        }
        if w[3].ip == ip && w[3].prop_intern_id == prop_id && w[3].shape_id == shape_id {
            self.lru = 3;
            return Some(w[3].slot_index);
        }
        None
    }

    /// 插入/替换：使用 LRU 路进行驱逐。
    #[inline(always)]
    pub(crate) fn insert(&mut self, entry: PropICEntry) {
        let way = self.lru as usize;
        self.ways[way] = entry;
        // 简单 LRU 推进：指向下一路
        self.lru = ((way + 1) & (PIC_WAYS - 1)) as u8;
    }
}

// ========================================================================
// 🧬 论文级创新 3: CSTS (Call Target Snapshot)
// 保持不变，但增加 CDD 支持
// ========================================================================

/// 闭包调用快照。
///
/// 字段按访问频率排列：`chunk_ptr`（每次调用解引用）→ `arity`/`locals_count`
/// （帧建立时读取）→ `chunk` Arc（仅在 chunk 切换时 clone）。
#[derive(Clone, Debug)]
pub struct ClosureSnapshot {
    /// 缓存的裸指针，避免热路径上 `Arc::as_ptr()` 的间接寻址。
    /// 生命周期由 `chunk: Arc<Chunk>` 保证。
    #[allow(dead_code)]
    pub(crate) chunk_ptr: *const nuzo_bytecode::Chunk,
    pub chunk: Arc<nuzo_bytecode::Chunk>,
    pub arity: u8,
    pub locals_count: u16,
}

// SAFETY: ClosureSnapshot 通过 Arc 共享 Chunk，裸指针仅用于只读快速访问。
unsafe impl Send for ClosureSnapshot {}
unsafe impl Sync for ClosureSnapshot {}

impl ClosureSnapshot {
    #[inline(always)]
    pub(crate) fn new(chunk: Arc<nuzo_bytecode::Chunk>, arity: u8, locals_count: u16) -> Self {
        let chunk_ptr = Arc::as_ptr(&chunk);
        ClosureSnapshot { chunk_ptr, chunk, arity, locals_count }
    }

    /// 获取 chunk 引用（零开销，无原子操作）。
    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn chunk_ref(&self) -> &nuzo_bytecode::Chunk {
        // SAFETY: chunk_ptr 由 Arc 保活，Arc 在 self 中持有
        unsafe { &*self.chunk_ptr }
    }
}

// ========================================================================
// CDD 调用器：消除 Box<dyn Fn> 的虚分派 + 堆分配开销
// ========================================================================

/// 静态函数指针签名：替代 `Box<dyn Fn>`，消除 vtable 间接跳转。
///
/// 每个调用点的 invoker 实际上是同一个函数（`closure_invoker_thunk`），
/// 区别仅在于传入的 `&ClosureSnapshot`。将 snapshot 存储在 call site 中，
/// invoker 退化为一个 `fn` 指针——零分配、零虚分派。
#[allow(dead_code)]
pub(crate) type ClosureInvokerFn =
    fn(&mut crate::vm::VM, &ClosureSnapshot, u16, usize) -> Result<(), nuzo_values::NuzoError>;

/// 唯一的 thunk 实现：所有调用点共享此函数指针。
#[inline(always)]
#[allow(dead_code)]
pub(crate) fn closure_invoker_thunk(
    vm: &mut crate::vm::VM,
    snap: &ClosureSnapshot,
    func_reg: u16,
    argc: usize,
) -> Result<(), nuzo_values::NuzoError> {
    vm.execute_closure_fast(snap, func_reg, argc)
}

/// 兼容旧接口：若外部仍需要 `Box<dyn Fn>` 形式（如测试），提供转换。
/// 热路径应直接使用 `ClosureInvokerFn` + `&ClosureSnapshot`。
pub(crate) type ClosureInvoker =
    Box<dyn Fn(&mut crate::vm::VM, u16, usize) -> Result<(), nuzo_values::NuzoError> + Send + Sync>;

// ========================================================================
// 编译期尺寸断言：防止意外的布局膨胀
// ========================================================================
const _: () = {
    assert!(std::mem::size_of::<GlobalCacheEntry>() == 8);
    assert!(std::mem::size_of::<PropICEntry>() == 16);
    // PropICSlot: 4×16 + 1 + padding → align(64) → 128 bytes (1 cache line 对齐膨胀)
    assert!(std::mem::size_of::<PropICSlot>() == 128);
};
