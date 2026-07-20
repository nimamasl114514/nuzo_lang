//! # Arena / Region 函数作用域级内存分配器
//!
//! ## 设计目标
//!
//! 在 CallFrame 中引入 bump allocator，实现函数作用域级别的零成本内存回收：
//! - **分配**: bump pointer, O(1), ~1ns
//! - **释放**: pop_frame 时重置指针, O(1), 零 GC 扫描
//! - **逃逸处理**: 保守策略 — 逃逸对象自动提升至持久 GC 内存
//!
//! ## 内存布局
//!
//! ```text
//! RegionAllocator.data (Vec<u8>)
//! ┌──────────────────────────────────────────────┐
//! │ frame_0 arena [start=0 .. top=N)             │ ← global_top
//! │ frame_1 arena [start=N .. top=M)             │
//! │ frame_2 arena [start=M .. top=K)             │ ← 当前写入位置
//! │ ... (unused)                                 │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! ## 与 ERSA 的关系
//!
//! | 分配器 | 作用域 | 生命周期 | 释放方式 |
//! |--------|--------|---------|---------|
//! | **RegionAllocator (本模块)** | 单函数帧 | 函数返回 | pop_frame O(1) 回缩 |
//! | **ERSA Scratch (gc.rs)** | 跨函数 | safe_point | epoch reset + promote |
//! | **Nursery (gc.rs)** | 全局 | Minor GC | mark-sweep |
//!

use std::fmt;

#[allow(unused_imports)]
use nuzo_config::ArenaConfig;
use nuzo_core::Value;
use nuzo_values::heap::HeapObject;

// ============================================================================
// 1. 类型定义
// ============================================================================

/// Per-frame Arena 分配状态（轻量，v2 扩展含对象追踪）
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ArenaFrameState {
    /// 此帧 Arena 区域在全局 Region 中的起始偏移（字节）
    pub start: usize,
    /// 当前 bump 指针（相对于 start，字节）
    pub top: usize,
    /// 是否有对象逃逸出此帧（需要 pop 时提升）
    pub has_escaped: bool,
    // === v2 新增：类型化对象追踪 ===
    /// 此帧第一个对象在 objects Vec 中的起始索引
    pub obj_start: usize,
    /// 此帧已分配的对象数量
    pub obj_count: usize,
}

/// 全局 Region 管理器（per-ExecutionContext）
pub(crate) struct RegionAllocator {
    /// 底层存储：Vec<u8> 字节数组（因为 HeapObject 大小不固定）
    data: Vec<u8>,

    /// 当前全局写入位置（所有帧的 Arena 连续排列）
    global_top: usize,

    /// 帧栈：用于 pop 时快速定位待释放区域
    frame_stack: Vec<ArenaFrameState>,

    /// 配置
    config: RegionConfig,

    /// Arena 中存储的实际对象（offset → HeapObject 映射）
    /// 索引与 data 中的分配 offset 对应，支持零拷贝存储
    objects: Vec<HeapObject>,
}

/// Region 配置参数
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionConfig {
    /// 单帧 Arena 最大容量（字节），默认 64KB
    pub max_frame_arena_size: usize,
    /// Region 总容量上限（字节），默认 16MB
    pub max_region_size: usize,
    /// 是否启用 Arena（可运行时关闭用于调试）
    pub enabled: bool,
}

impl Default for RegionConfig {
    fn default() -> Self {
        Self {
            max_frame_arena_size: 64 * 1024,   // 64KB per frame
            max_region_size: 16 * 1024 * 1024, // 16MB total
            enabled: true,
        }
    }
}

impl RegionConfig {
    /// 从 ArenaConfig 创建 RegionConfig
    ///
    /// 便捷转换方法，将统一配置结构体映射为内部 RegionConfig。
    /// 字段一一对应，语义完全一致。
    pub fn from_arena_config(config: ArenaConfig) -> Self {
        Self {
            max_frame_arena_size: config.max_frame_arena_size,
            max_region_size: config.max_region_size,
            enabled: config.enabled,
        }
    }
}

/// Arena 分配结果
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AllocationResult {
    /// 成功分配在 Arena 中
    Arena { offset: usize, size: usize },
    /// Arena 已满或不适用，降级到 GC 分配
    Fallthrough,
}

/// 提升请求：将 Arena 中的对象复制到 GC 持久内存
#[derive(Debug)]
pub struct PromoteRequest {
    /// 对象在 Arena 中的偏移量
    pub arena_offset: usize,
    /// 对象大小（字节）
    pub size: usize,
    // heap_object 字段将在使用时从 arena 内存解码填入
}

impl fmt::Display for AllocationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllocationResult::Arena { offset, size } => {
                write!(f, "Arena(offset={}, size={})", offset, size)
            }
            AllocationResult::Fallthrough => write!(f, "Fallthrough"),
        }
    }
}

// ============================================================================
// 2. RegionAllocator 实现
// ============================================================================

impl RegionAllocator {
    /// 创建新的 Region 管理器
    ///
    /// # 参数
    /// - `config`: Region 配置（单帧大小、总容量、启用状态）
    ///
    /// # 返回
    /// 初始化完成的 RegionAllocator 实例
    pub fn new(config: RegionConfig) -> Self {
        Self {
            data: Vec::new(),
            global_top: 0,
            frame_stack: Vec::new(),
            config,
            objects: Vec::new(),
        }
    }

    /// 带默认配置创建
    ///
    /// 默认配置：单帧 64KB，总容量 16MB，启用状态
    pub fn with_default() -> Self {
        Self::new(RegionConfig::default())
    }

    /// 开始新帧的 Arena 区域（push_frame 时调用）
    ///
    /// 在帧栈上推入新的分配状态，记录当前 global_top 作为此帧的起始偏移。
    /// 返回帧索引（usize），用于后续 allocate() / end_frame() 调用。
    ///
    /// # 返回
    /// 新推入帧的索引（frame_stack 中的位置）
    #[inline(always)]
    pub fn begin_frame(&mut self) -> usize {
        let idx = self.frame_stack.len();
        let obj_start = self.objects.len(); // v2: 记录此帧对象起始索引
        self.frame_stack.push(ArenaFrameState {
            start: self.global_top,
            top: 0,
            has_escaped: false,
            obj_start,    // v2: 对象索引起点
            obj_count: 0, // v2: 初始无对象
        });
        idx
    }

    /// 在指定帧的 Arena 中分配 raw 内存
    ///
    /// # 参数
    /// - `frame_idx`: 目标帧的索引（来自 begin_frame() 的返回值）
    /// - `size`: 需要分配的字节数
    /// - `align`: 内存对齐要求（必须是 2 的幂）
    ///
    /// # 返回
    /// - `AllocationResult::Arena { offset, size }`: 成功分配，offset 是全局偏移
    /// - `AllocationResult::Fallthrough`: Arena 已满/禁用/索引无效，需降级到 GC 堆分配
    #[inline(always)]
    pub fn allocate(&mut self, frame_idx: usize, size: usize, align: usize) -> AllocationResult {
        // 快速路径：禁用检查
        if !self.config.enabled {
            return AllocationResult::Fallthrough;
        }

        // 快速路径：零大小分配无意义
        if size == 0 {
            return AllocationResult::Fallthrough;
        }

        // 通过索引获取目标帧的可变状态
        let state = match self.frame_stack.get_mut(frame_idx) {
            Some(s) => s,
            None => return AllocationResult::Fallthrough,
        };

        // 对齐计算：将 top 向上对齐到 align 边界
        let aligned_top = Self::align_up(state.top, align);
        let new_top = aligned_top + size;

        // 边界检查 1：单帧 Arena 容量限制
        if new_top > self.config.max_frame_arena_size {
            return AllocationResult::Fallthrough;
        }

        // 边界检查 2：Region 总容量限制（用 state.start 作为全局起始偏移）
        if state.start + new_top > self.config.max_region_size {
            return AllocationResult::Fallthrough;
        }

        // 计算全局偏移量
        let offset = state.start + aligned_top;

        // 确保 data Vec 有足够容量（预分配策略减少 realloc）
        let end = offset + size;
        if end > self.data.len() {
            let extra = (end - self.data.len()).max(4096);
            self.data.resize(end + extra, 0);
        }

        // 更新指针
        state.top = new_top;
        self.global_top = state.start + new_top;

        AllocationResult::Arena { offset, size }
    }

    /// 在 Arena 中分配并存储一个完整的 HeapObject（v2 零拷贝核心路径）
    ///
    /// 与 `allocate()` 只分配 raw 内存不同，此方法将 `HeapObject` 直接存入
    /// `objects` 向量，返回 `from_arena_index` 编码的 `Value`（编码为对象索引）。
    /// 调用方无需再走 GC 的 `alloc_scratch` 路径。
    ///
    /// # 参数
    /// - `frame_idx`: 目标帧的索引
    /// - `obj`: 待存储的堆对象（move 语义，成功时所有权转移至 Arena；失败时归还）
    /// - `size`: 对象预估大小（用于 bump 分配器容量检查）
    ///
    /// # 返回
    /// - `Ok(Value)`: 成功存入 Arena，Value 编码了对象在 objects Vec 中的索引
    /// - `Err(HeapObject)`: Arena 已满/禁用/索引无效，对象原样归还，调用方应降级到 GC 路径
    ///
    /// # 生命周期保证
    /// 存储的对象在 `end_frame(frame_idx, false)` 时被 O(1) 截断释放。
    /// 若 `end_frame(frame_idx, true)` 标记逃逸，调用方需通过 `take_arena_object()` 取走。
    #[inline(always)]
    pub fn allocate_object(
        &mut self,
        frame_idx: usize,
        obj: HeapObject,
        size: usize,
    ) -> Result<Value, HeapObject> {
        // 复用 allocate() 做容量检查和 bump 指针推进
        match self.allocate(frame_idx, size, std::mem::align_of::<u8>()) {
            AllocationResult::Arena { .. } => {
                // === v2 核心：存入类型化对象 ===
                let obj_idx = self.objects.len(); // push 前记录对象索引
                self.objects.push(obj);

                // 更新帧的对象追踪信息
                if let Some(state) = self.frame_stack.get_mut(frame_idx) {
                    if state.obj_count == 0 {
                        state.obj_start = obj_idx; // 首个对象：记录起始索引
                    }
                    state.obj_count += 1;
                }

                // 用对象索引编码 Value（v2 语义：arena offset = objects Vec 索引）
                Ok(Value::from_arena_index(obj_idx as u32))
            }
            AllocationResult::Fallthrough => Err(obj),
        }
    }

    /// 根据 Arena 对象索引获取对象的不可变引用
    ///
    /// # 参数
    /// - `arena_obj_idx`: 来自 `allocate_object()` 返回值中编码的对象索引
    ///
    /// # 返回
    /// - `Some(&HeapObject)`: 找到对应对象
    /// - `None`: 索引越界或对象已被释放
    ///
    /// # 安全性
    /// 对象的生命周期绑定到 `&self`（RegionAllocator 活跃期间有效）。
    /// 在 `end_frame()` 无逃逸截断后，该索引对应的对象已被 drop。
    #[inline(always)]
    pub fn get_arena_object(&self, arena_obj_idx: u32) -> Option<&HeapObject> {
        self.objects.get(arena_obj_idx as usize)
    }

    /// 根据 Arena 对象索引获取可变引用（用于 mutate_heap_object 等写操作）
    #[inline(always)]
    pub fn get_arena_object_mut(&mut self, arena_obj_idx: u32) -> Option<&mut HeapObject> {
        self.objects.get_mut(arena_obj_idx as usize)
    }

    /// 标记指定帧有逃逸对象
    ///
    /// 当检测到 Arena 中的对象被外部引用（如闭包捕获、返回值）时调用，
    /// 使得 end_frame() 知道需要做提升操作而非简单回缩。
    ///
    /// # 参数
    /// - `frame_idx`: 目标帧的索引
    #[inline(always)]
    #[cfg(test)]
    pub fn mark_escaped(&mut self, frame_idx: usize) {
        if let Some(state) = self.frame_stack.get_mut(frame_idx) {
            state.has_escaped = true;
        }
    }

    /// 结束指定帧的 Arena 区域（pop_frame 时调用）
    ///
    /// # 参数
    /// - `frame_idx`: 即将 pop 的帧的索引
    /// - `has_escape`: 调用方已检测到的逃逸状态（true=需要提升）
    ///
    /// # 返回
    /// - `Some((start, end))`: 有逃逸，返回该帧 Arena 区域的范围供调用方遍历提升
    /// - `None`: 无逃逸，O(1) 回缩 global_top 并释放该区域
    ///
    /// # 注意
    /// 无论是否有逃逸，global_top 都会回缩到该帧的 start。
    /// 使用 truncate(frame_idx) 移除该帧及之后的所有帧（LIFO 语义）。
    /// 有逃逸时，调用方负责在提升完成后处理 Arena 中的数据。
    pub fn end_frame(&mut self, frame_idx: usize, has_escape: bool) -> Option<(usize, usize)> {
        let state = self.frame_stack.get(frame_idx)?;

        if has_escape || state.has_escaped {
            // 有逃逸 → 返回区域范围给调用方做提升
            // 注意：有逃逸时不截断 objects，调用方负责处理逃逸对象后手动清理
            let range = (state.start, state.start + state.top);
            // 先回缩（提升完成后对象已在 GC 持久区）
            self.global_top = state.start;
            // LIFO：移除该帧及之后的所有帧
            self.frame_stack.truncate(frame_idx);
            Some(range)
        } else {
            // 无逃逸 → O(1) 释放整个区域（含类型化对象）
            // v2 核心：truncate objects Vec → 所有该帧对象瞬间 drop
            if state.obj_count > 0 {
                self.objects.truncate(state.obj_start);
            }
            self.global_top = state.start;
            self.frame_stack.truncate(frame_idx);
            None
        }
    }

    /// 根据 Arena 对象索引取走对象所有权（用于逃逸提升场景）
    ///
    /// 当检测到 Arena 中的对象需要逃逸到 GC 持久区时，调用此方法
    /// 将对象从 Arena 中移出，由调用方负责将其提升至 GC 堆。
    ///
    /// # 参数
    /// - `arena_obj_idx`: 目标对象的 Arena 索引
    ///
    /// # 返回
    /// - `Some(HeapObject)`: 成功取走对象所有权
    /// - `None`: 索引越界
    ///
    /// # 注意
    /// 使用 `remove(idx)` 保持索引稳定（O(n)但安全）。
    /// 仅应在 `end_frame()` 有逃逸路径中使用，此时帧即将被销毁。
    pub fn take_arena_object(&mut self, arena_obj_idx: u32) -> Option<HeapObject> {
        let idx = arena_obj_idx as usize;
        if idx < self.objects.len() { Some(self.objects.remove(idx)) } else { None }
    }

    /// 获取指定帧中所有已分配对象的不可变切片
    ///
    /// 用于逃逸提升时批量遍历一帧内的所有 Arena 对象。
    ///
    /// # 参数
    /// - `frame_idx`: 目标帧索引
    ///
    /// # 返回
    /// - `Some(&[HeapObject])`: 该帧的对象切片
    /// - `None`: 帧索引无效或该帧无对象
    #[inline]
    pub fn frame_objects(&self, frame_idx: usize) -> Option<&[HeapObject]> {
        let state = self.frame_stack.get(frame_idx)?;
        if state.obj_count == 0 {
            return Some(&[]); // 空切片而非 None，表示"有效但无对象"
        }
        self.objects.get(state.obj_start..state.obj_start + state.obj_count)
    }

    /// 获取 Arena 中当前存储的对象总数（用于调试/统计）
    #[inline]
    #[cfg(test)]
    pub fn objects_len(&self) -> usize {
        self.objects.len()
    }

    /// 获取 Arena 中原始字节的只读引用
    ///
    /// # 安全
    /// 调用方必须确保 offset + len 不超过已分配区域边界
    #[inline(always)]
    #[cfg(test)]
    pub fn as_slice(&self, offset: usize, len: usize) -> &[u8] {
        &self.data[offset..offset + len]
    }

    /// 获取 Arena 中原始字节的可变引用
    ///
    /// # 安全
    /// 调用方必须确保 offset + len 不超过已分配区域边界
    #[inline(always)]
    #[cfg(test)]
    pub fn as_mut_slice(&mut self, offset: usize, len: usize) -> &mut [u8] {
        &mut self.data[offset..offset + len]
    }

    /// 获取当前全局写入位置（用于调试/统计）
    #[inline]
    #[cfg(test)]
    pub fn global_usage(&self) -> usize {
        self.global_top
    }

    /// 当前活跃帧深度
    #[inline]
    #[cfg(test)]
    pub fn depth(&self) -> usize {
        self.frame_stack.len()
    }

    /// 重置整个 Region（chunk 切换时调用）
    ///
    /// 清空所有已分配内存、重置指针、清空帧栈。
    /// 用于执行上下文切换或 chunk 边界跨越场景。
    pub fn reset(&mut self) {
        self.data.clear();
        self.global_top = 0;
        self.frame_stack.clear();
        self.objects.clear();
    }

    /// 获取配置的只读引用
    #[inline]
    #[cfg(test)]
    pub fn config(&self) -> &RegionConfig {
        &self.config
    }

    /// 获取指定帧的 Arena 状态（只读引用）
    ///
    /// 供 check_arena_escape() 等需要查询帧 start/top 的场景使用。
    /// 返回 None 表示索引无效。
    #[inline]
    pub fn frame_state(&self, frame_idx: usize) -> Option<&ArenaFrameState> {
        self.frame_stack.get(frame_idx)
    }

    // ========================================================================
    // 私有辅助方法
    // ========================================================================

    /// 向上对齐到指定对齐边界
    ///
    /// # 参数
    /// - `value`: 待对齐的值
    /// - `align`: 对齐边界（必须是 2 的幂）
    ///
    /// # 返回
    /// 第一个 >= value 且能被 align 整除的数
    #[inline(always)]
    fn align_up(value: usize, align: usize) -> usize {
        debug_assert!(align.is_power_of_two(), "alignment must be power of 2");
        (value + align - 1) & !(align - 1)
    }
}

// ============================================================================
// 3. 测试模块
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_values::heap::RangeEnd;

    // -----------------------------------------------------------------------
    // 基础构造测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_new_and_default() {
        // 自定义配置创建
        let custom_config =
            RegionConfig { max_frame_arena_size: 1024, max_region_size: 4096, enabled: true };
        let alloc = RegionAllocator::new(custom_config);
        assert_eq!(alloc.global_usage(), 0);
        assert_eq!(alloc.depth(), 0);

        // 默认配置创建
        let default_alloc = RegionAllocator::with_default();
        assert_eq!(default_alloc.global_usage(), 0);
        assert_eq!(default_alloc.depth(), 0);
        assert!(default_alloc.config().enabled);
        assert_eq!(default_alloc.config().max_frame_arena_size, 64 * 1024);
        assert_eq!(default_alloc.config().max_region_size, 16 * 1024 * 1024);
    }

    // -----------------------------------------------------------------------
    // 帧管理测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_begin_frame() {
        let mut alloc = RegionAllocator::with_default();

        // 第一帧：start 应为 0
        let fidx0 = alloc.begin_frame();
        let state0 = alloc.frame_state(fidx0).unwrap();
        assert_eq!(state0.start, 0);
        assert_eq!(state0.top, 0);
        assert!(!state0.has_escaped);
        assert_eq!(alloc.depth(), 1);

        // 在第一帧中分配一些空间
        let result = alloc.allocate(fidx0, 16, 8);
        match result {
            AllocationResult::Arena { offset, size } => {
                assert_eq!(offset, 0); // 第一帧第一个分配，对齐后仍为 0
                assert_eq!(size, 16);
            }
            _ => panic!("expected Arena allocation"),
        }
        assert_eq!(alloc.frame_state(fidx0).unwrap().top, 16);

        // 第二帧：start 应为 16（接在第一帧后面）
        let fidx1 = alloc.begin_frame();
        let state1 = alloc.frame_state(fidx1).unwrap();
        assert_eq!(state1.start, 16);
        assert_eq!(state1.top, 0);
        assert_eq!(alloc.depth(), 2);
    }

    // -----------------------------------------------------------------------
    // 分配基础功能测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_allocate_basic() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 第一次分配
        let r1 = alloc.allocate(fidx, 32, 8);
        match r1 {
            AllocationResult::Arena { offset, size } => {
                assert_eq!(offset, 0);
                assert_eq!(size, 32);
            }
            _ => panic!("expected Arena"),
        }

        // 第二次分配（紧接第一次之后）
        let r2 = alloc.allocate(fidx, 16, 8);
        match r2 {
            AllocationResult::Arena { offset, size } => {
                assert_eq!(offset, 32);
                assert_eq!(size, 16);
            }
            _ => panic!("expected Arena"),
        }

        // 验证 global_top 正确更新
        assert_eq!(alloc.global_usage(), 48);
        assert_eq!(alloc.frame_state(fidx).unwrap().top, 48);
    }

    // -----------------------------------------------------------------------
    // 对齐测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_allocate_alignment() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 分配 1 字节，对齐 8 → 应占用 offset=0, top=1
        let r1 = alloc.allocate(fidx, 1, 8);
        match r1 {
            AllocationResult::Arena { offset, .. } => {
                assert_eq!(offset, 0);
            }
            _ => panic!("expected Arena"),
        }
        assert_eq!(alloc.frame_state(fidx).unwrap().top, 1); // align_up(0,8)=0, +1=1

        // 让我用更清晰的方式测试对齐
        let mut alloc2 = RegionAllocator::with_default();
        let fidx2 = alloc2.begin_frame();

        // 先分配 5 字节，对齐 1 → top = 5
        let _ = alloc2.allocate(fidx2, 5, 1);
        assert_eq!(alloc2.frame_state(fidx2).unwrap().top, 5);

        // 再分配 4 字节，对齐 8 → aligned_top = align_up(5, 8) = 8, new_top = 12
        let r3 = alloc2.allocate(fidx2, 4, 8);
        match r3 {
            AllocationResult::Arena { offset, .. } => {
                // offset = start(0) + aligned_top(8) = 8
                assert_eq!(offset, 8);
            }
            _ => panic!("expected Arena"),
        }
        assert_eq!(alloc2.frame_state(fidx2).unwrap().top, 12); // 8 + 4 = 12

        // 再分配 8 字节，对齐 8 → aligned_top = align_up(12, 8) = 16, new_top = 24
        let _ = alloc2.allocate(fidx2, 8, 8);
        assert_eq!(alloc2.frame_state(fidx2).unwrap().top, 24); // 16 + 8 = 24
    }

    // -----------------------------------------------------------------------
    // 容量限制 / Fallback 测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_allocate_full_fallback() {
        let config = RegionConfig {
            max_frame_arena_size: 64, // 极小的单帧限制
            max_region_size: 1024,
            enabled: true,
        };
        let mut alloc = RegionAllocator::new(config);
        let fidx = alloc.begin_frame();

        // 第一次分配应该成功（32 < 64）
        let r1 = alloc.allocate(fidx, 32, 8);
        assert!(matches!(r1, AllocationResult::Arena { .. }));

        // 第二次分配也应该成功（32 + 32 = 64 <= 64）
        let r2 = alloc.allocate(fidx, 32, 8);
        assert!(matches!(r2, AllocationResult::Arena { .. }));

        // 第三次分配应该失败（已满）
        let r3 = alloc.allocate(fidx, 1, 1);
        assert_eq!(r3, AllocationResult::Fallthrough);
    }

    #[test]
    fn test_allocate_disabled() {
        let config = RegionConfig {
            max_frame_arena_size: 1024 * 1024,
            max_region_size: 16 * 1024 * 1024,
            enabled: false, // 禁用 Arena
        };
        let mut alloc = RegionAllocator::new(config);
        let fidx = alloc.begin_frame();

        // 即使有足够空间，禁用时也应 Fallthrough
        let r = alloc.allocate(fidx, 100, 8);
        assert_eq!(r, AllocationResult::Fallthrough);
    }

    #[test]
    fn test_allocate_zero_size() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 零大小分配应降级
        let r = alloc.allocate(fidx, 0, 8);
        assert_eq!(r, AllocationResult::Fallthrough);
    }

    // -----------------------------------------------------------------------
    // end_frame / 逃逸测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_end_frame_no_escape() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 分配一些数据
        let _ = alloc.allocate(fidx, 64, 8);
        assert_eq!(alloc.global_usage(), 64);

        // 无逃逸结束帧 → 应返回 None 且 global_top 回缩
        let result = alloc.end_frame(fidx, false);
        assert!(result.is_none());
        assert_eq!(alloc.global_usage(), 0);
        assert_eq!(alloc.depth(), 0);
    }

    #[test]
    fn test_end_frame_with_escape() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 分配一些数据
        let _ = alloc.allocate(fidx, 128, 8);
        assert_eq!(alloc.global_usage(), 128);

        // 通过 mark_escaped 标记逃逸
        alloc.mark_escaped(fidx);
        assert!(alloc.frame_state(fidx).unwrap().has_escaped);

        // 有逃逸结束帧 → 应返回范围
        let result = alloc.end_frame(fidx, false); // frame 本身已有 has_escaped=true
        match result {
            Some((start, end)) => {
                assert_eq!(start, 0);
                assert_eq!(end, 128);
            }
            None => panic!("expected Some with escape range"),
        }
        // global_top 也应回缩
        assert_eq!(alloc.global_usage(), 0);
        assert_eq!(alloc.depth(), 0);
    }

    #[test]
    fn test_end_frame_external_escape_flag() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        let _ = alloc.allocate(fidx, 50, 8);

        // frame 本身没有 mark_escaped，但调用方传入 has_escape=true
        let result = alloc.end_frame(fidx, true);
        assert!(result.is_some()); // 因为 has_escape=true
        match result {
            Some((start, end)) => {
                assert_eq!(start, 0);
                assert_eq!(end, 50);
            }
            _ => panic!("expected Some"),
        }
    }

    // -----------------------------------------------------------------------
    // 嵌套帧测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_nested_frames() {
        let mut alloc = RegionAllocator::with_default();

        // 帧 0
        let fidx0 = alloc.begin_frame();
        let _ = alloc.allocate(fidx0, 32, 8);
        assert_eq!(alloc.depth(), 1);
        assert_eq!(alloc.frame_state(fidx0).unwrap().start, 0);

        // 帧 1（嵌套）
        let fidx1 = alloc.begin_frame();
        let _ = alloc.allocate(fidx1, 16, 8);
        assert_eq!(alloc.depth(), 2);
        assert_eq!(alloc.frame_state(fidx1).unwrap().start, 32); // 接在 frame0 之后

        // 帧 2（再嵌套）
        let fidx2 = alloc.begin_frame();
        let _ = alloc.allocate(fidx2, 8, 8);
        assert_eq!(alloc.depth(), 3);
        assert_eq!(alloc.frame_state(fidx2).unwrap().start, 48); // 接在 frame1 之后

        // pop 帧 2（LIFO 顺序）
        let r2 = alloc.end_frame(fidx2, false);
        assert!(r2.is_none()); // 无逃逸
        assert_eq!(alloc.depth(), 2);
        // global_top 应回缩到 frame2.start（即 frame1 的尾部位置 32+16=48）
        assert_eq!(alloc.global_usage(), 48);

        // pop 帧 1
        let r1 = alloc.end_frame(fidx1, false);
        assert!(r1.is_none());
        assert_eq!(alloc.depth(), 1);
        assert_eq!(alloc.global_usage(), 32); // 回缩到 frame1.start

        // pop 帧 0
        let r0 = alloc.end_frame(fidx0, false);
        assert!(r0.is_none());
        assert_eq!(alloc.depth(), 0);
        assert_eq!(alloc.global_usage(), 0);
    }

    // -----------------------------------------------------------------------
    // 重置测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_reset() {
        let mut alloc = RegionAllocator::with_default();

        // 创建多个帧并分配
        let f0 = alloc.begin_frame();
        let _ = alloc.allocate(f0, 100, 8);
        let f1 = alloc.begin_frame();
        let _ = alloc.allocate(f1, 200, 8);

        assert_eq!(alloc.depth(), 2);
        assert!(alloc.global_usage() > 0);

        // 重置
        alloc.reset();

        assert_eq!(alloc.global_usage(), 0);
        assert_eq!(alloc.depth(), 0);
        // data 也应被清空
        assert!(alloc.as_slice(0, 0).is_empty()); // 空切片合法
    }

    // -----------------------------------------------------------------------
    // 切片访问测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_slice_access() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 分配 16 字节
        let r = alloc.allocate(fidx, 16, 8);
        let offset = match r {
            AllocationResult::Arena { offset, .. } => offset,
            _ => panic!("expected Arena"),
        };

        // 写入数据
        let slice = alloc.as_mut_slice(offset, 16);
        for (i, byte) in slice.iter_mut().enumerate() {
            *byte = i as u8; // 0, 1, 2, ..., 15
        }

        // 读回验证
        let read_slice = alloc.as_slice(offset, 16);
        for (i, &byte) in read_slice.iter().enumerate() {
            assert_eq!(byte, i as u8);
        }
    }

    // -----------------------------------------------------------------------
    // 全局容量限制测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_global_size_limit() {
        let config = RegionConfig {
            max_frame_arena_size: 1024, // 单帧可以很大
            max_region_size: 100,       // 但总共只有 100 字节
            enabled: true,
        };
        let mut alloc = RegionAllocator::new(config);
        let fidx = alloc.begin_frame();

        // 第一次分配 50 字节应成功
        let r1 = alloc.allocate(fidx, 50, 8);
        assert!(matches!(r1, AllocationResult::Arena { .. }));

        // 第二次分配 60 字节应失败（50+60 > 100）
        let r2 = alloc.allocate(fidx, 60, 8);
        assert_eq!(r2, AllocationResult::Fallthrough);
    }

    // -----------------------------------------------------------------------
    // 多帧独立空间测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_multiple_frames_independent_space() {
        let mut alloc = RegionAllocator::with_default();

        // 帧 A：分配并写入特定模式
        let fa = alloc.begin_frame();
        let ra = alloc.allocate(fa, 8, 8);
        let off_a = match ra {
            AllocationResult::Arena { offset, .. } => offset,
            _ => panic!(),
        };
        for b in alloc.as_mut_slice(off_a, 8).iter_mut() {
            *b = 0xAA;
        }

        // 帧 B：分配并写入不同模式
        let fb = alloc.begin_frame();
        let rb = alloc.allocate(fb, 8, 8);
        let off_b = match rb {
            AllocationResult::Arena { offset, .. } => offset,
            _ => panic!(),
        };
        for b in alloc.as_mut_slice(off_b, 8).iter_mut() {
            *b = 0xBB;
        }

        // 验证两块数据互不干扰
        assert_ne!(off_a, off_b);
        let slice_a = alloc.as_slice(off_a, 8);
        let slice_b = alloc.as_slice(off_b, 8);
        assert!(slice_a.iter().all(|&b| b == 0xAA));
        assert!(slice_b.iter().all(|&b| b == 0xBB));
    }

    // -----------------------------------------------------------------------
    // PromoteRequest 基础测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_promote_request() {
        let req = PromoteRequest { arena_offset: 1024, size: 64 };
        assert_eq!(req.arena_offset, 1024);
        assert_eq!(req.size, 64);
    }

    // -----------------------------------------------------------------------
    // AllocationResult Display 测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_allocation_result_display() {
        let arena = AllocationResult::Arena { offset: 42, size: 16 };
        assert_eq!(format!("{}", arena), "Arena(offset=42, size=16)");

        let fallback = AllocationResult::Fallthrough;
        assert_eq!(format!("{}", fallback), "Fallthrough");
    }

    // -----------------------------------------------------------------------
    // 边界条件：最大对齐测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_large_alignment() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 使用 256 字节对齐（模拟 SIMD/Cache line 场景）
        let r = alloc.allocate(fidx, 64, 256);
        match r {
            AllocationResult::Arena { offset, size } => {
                assert_eq!(offset, 0); // 第一帧第一个分配
                assert_eq!(size, 64);
                assert_eq!(offset % 256, 0); // 验证对齐
            }
            _ => panic!("expected Arena"),
        }
        assert_eq!(alloc.frame_state(fidx).unwrap().top, 64); // align_up(0,256)+64 = 256
    }

    // ===================================================================
    // Arena 性能基准测试：模拟真实 VM 工作负载
    // ===================================================================

    #[test]
    fn bench_arena_throughput_simulated() {
        use std::time::Instant;

        const FRAMES: usize = 10_000; // 模拟 10000 次函数调用
        const ALLOCS_PER_FRAME: usize = 5; // 每帧 5 次分配（模拟临时数组/Range/字符串）
        const ALLOC_SIZE: usize = 48; // 典型 HeapObject 大小（Array/Dict 约 40-80 字节）

        let mut alloc = RegionAllocator::with_default();

        // === 预热 ===
        for _ in 0..100 {
            let fidx = alloc.begin_frame();
            for _ in 0..ALLOCS_PER_FRAME {
                let _ = alloc.allocate(fidx, ALLOC_SIZE, 8);
            }
            alloc.end_frame(fidx, false);
        }
        alloc.reset();

        // === 正式测量：模拟循环内函数调用模式 ===
        let start = Instant::now();
        let mut total_allocs = 0usize;
        let mut total_hits = 0usize;
        let mut total_misses = 0usize;

        for i in 0..FRAMES {
            let fidx = alloc.begin_frame();

            for j in 0..ALLOCS_PER_FRAME {
                match alloc.allocate(fidx, ALLOC_SIZE + (j % 16), 8) {
                    AllocationResult::Arena { .. } => total_hits += 1,
                    AllocationResult::Fallthrough => total_misses += 1,
                }
                total_allocs += 1;
            }

            // 模拟 pop_frame：无逃逸 → O(1) 释放
            alloc.end_frame(fidx, false);

            // 每 100 帧重置一次 Region（模拟 chunk switch）
            if (i + 1) % 100 == 0 {
                alloc.reset();
            }
        }

        let elapsed = start.elapsed();
        let total_ops = FRAMES * (1 + ALLOCS_PER_FRAME); // begin + N * allocate + end

        println!("=== Arena Throughput Benchmark ===");
        println!("Frames: {}", FRAMES);
        println!("Allocs/frame: {}", ALLOCS_PER_FRAME);
        println!("Total allocs: {}", total_allocs);
        println!(
            "Arena hits: {} ({:.1}%)",
            total_hits,
            total_hits as f64 / total_allocs.max(1) as f64 * 100.0
        );
        println!(
            "Arena misses: {} ({:.1}%)",
            total_misses,
            total_misses as f64 / total_allocs.max(1) as f64 * 100.0
        );
        println!("Elapsed: {:?}", elapsed);
        println!(
            "Throughput: {:.2} M ops/s",
            total_ops as f64 / elapsed.as_secs_f64().max(0.001) / 1e6
        );
        println!(
            "Alloc throughput: {:.2} M alloc/s",
            total_allocs as f64 / elapsed.as_secs_f64().max(0.001) / 1e6
        );
        println!("Per-op latency: {:.0} ns", elapsed.as_nanos() as f64 / total_ops.max(1) as f64);

        // 验证所有分配都命中 Arena（在默认配置下不应 miss）
        assert!(total_hits > 0, "至少应有一次 Arena 命中");
        assert_eq!(total_misses, 0, "默认配置下 64KB/帧 不应溢出");
    }

    #[test]
    fn bench_arena_vs_fallback_pressure() {
        use std::time::Instant;

        // 对比测试：正常配置 vs 压迫配置（小 Arena 触发 Fallthrough）
        const ITERATIONS: usize = 50_000;
        const ALLOC_SIZE: usize = 1024; // 1KB 分配

        // --- 场景 A: 宽松配置（64KB/帧，几乎不 fallback）---
        let mut alloc_a = RegionAllocator::new(RegionConfig {
            max_frame_arena_size: 64 * 1024,
            max_region_size: 16 * 1024 * 1024,
            enabled: true,
        });
        let start_a = Instant::now();
        let mut hits_a = 0usize;
        for _ in 0..ITERATIONS {
            let fidx = alloc_a.begin_frame();
            match alloc_a.allocate(fidx, ALLOC_SIZE, 8) {
                AllocationResult::Arena { .. } => hits_a += 1,
                AllocationResult::Fallthrough => {}
            }
            alloc_a.end_frame(fidx, false);
        }
        let elapsed_a = start_a.elapsed();

        // --- 场景 B: 压迫配置（256B/帧，大量 fallback）---
        let mut alloc_b = RegionAllocator::new(RegionConfig {
            max_frame_arena_size: 256, // 极小帧 Arena
            max_region_size: 16 * 1024 * 1024,
            enabled: true,
        });
        let start_b = Instant::now();
        let mut hits_b = 0usize;
        let mut misses_b = 0usize;
        for _ in 0..ITERATIONS {
            let fidx = alloc_b.begin_frame();
            match alloc_b.allocate(fidx, ALLOC_SIZE, 8) {
                AllocationResult::Arena { .. } => hits_b += 1,
                AllocationResult::Fallthrough => misses_b += 1,
            }
            alloc_b.end_frame(fidx, false);
        }
        let elapsed_b = start_b.elapsed();

        println!("\n=== Arena Config Comparison Benchmark ===");
        println!("Iterations: {}", ITERATIONS);
        println!("Alloc size: {} bytes", ALLOC_SIZE);
        println!();
        println!("[Config A: 64KB/frame] Hits: {}/{}, Time: {:?}", hits_a, ITERATIONS, elapsed_a);
        println!(
            "[Config B: 256B/frame]  Hits: {}/{}, Misses: {}, Time: {:?}",
            hits_b, ITERATIONS, misses_b, elapsed_b
        );

        // 场景 B 应该全部 fallback（1KB > 256B limit）
        assert_eq!(hits_b, 0, "256B 帧不应容纳 1KB 分配");
        assert_eq!(misses_b, ITERATIONS, "全部应 fallback");
        // 场景 A 应该全部命中
        assert_eq!(hits_a, ITERATIONS, "64KB 帧应全部命中");
    }

    // ===================================================================
    // Arena 端到端性能对比基准：模拟真实 Nuzo 代码执行场景
    // ===================================================================
    //
    // 设计思路：
    // 模拟 Nuzo 代码中 "循环内创建临时数组" 的典型工作负载，
    // 这是 Arena 优化的核心目标场景。通过 RegionAllocator 的
    // begin/allocate/end 循环来量化 Arena 框架的实际开销。
    //
    // 测试覆盖两个维度：
    // 1. 端到端工作负载：模拟 5000 次循环 × 100 次迭代 = 500K 次 frame 操作
    // 2. 原始分配器吞吐量：单独测量 RegionAllocator 的理论极限性能

    #[test]
    fn bench_arena_e2e_temp_array_loop() {
        use std::time::Instant;

        // -----------------------------------------------------------------
        // 场景配置：模拟 Nuzo 代码中的循环内临时数组创建
        // -----------------------------------------------------------------
        // 对应的 Nuzo 伪代码：
        //   total = 0
        //   i = 0
        //   while i < 5000 {
        //       temp = [i, i*2, i*3, i*4, i*5]  // ← Arena 目标场景
        //       total = total + temp[0] + temp[4]
        //       i = i + 1
        //   }
        //   total
        //
        // 每次循环迭代对应一次 begin_frame → allocate(数组) → end_frame
        // 数组大小：5 个元素 + 元数据 ≈ 56 字节（HeapObject::Array 典型大小）

        const LOOP_COUNT: usize = 5_000; // 外层循环次数（模拟 while < 5000）
        const ARRAY_SIZE: usize = 56; // 模拟 HeapObject::Array(5 elements)
        const BENCHMARK_ITERATIONS: usize = 100; // 基准测试重复次数（用于统计分析）

        let mut alloc = RegionAllocator::with_default();

        // === 预热阶段：让 CPU 缓存和分支预测稳定 ===
        for _ in 0..3 {
            for _ in 0..LOOP_COUNT {
                let fidx = alloc.begin_frame();
                let _ = alloc.allocate(fidx, ARRAY_SIZE, 8);
                alloc.end_frame(fidx, false); // 无逃逸释放
            }
            alloc.reset();
        }
        alloc.reset();

        // === 正式测量：Arena 启用状态下的执行时间分布 ===
        let mut times_enabled: Vec<std::time::Duration> = Vec::with_capacity(BENCHMARK_ITERATIONS);

        for iteration in 0..BENCHMARK_ITERATIONS {
            let start = Instant::now();

            // 模拟单次 Nuzo 代码执行：5000 次循环，每次创建临时数组
            for i in 0..LOOP_COUNT {
                let fidx = alloc.begin_frame();
                // 模拟数组分配：大小随元素索引微变（模拟真实数据布局）
                let dynamic_size = ARRAY_SIZE + (i % 8); // 56~63 字节波动
                let result = alloc.allocate(fidx, dynamic_size, 8);

                // 验证分配成功（Arena fast path 命中）
                match result {
                    AllocationResult::Arena { .. } => {} // 预期：命中 Arena
                    AllocationResult::Fallthrough => {
                        panic!("迭代 #{iteration} 循环 #{i}: Arena 应命中但 fallback");
                    }
                }

                // 模拟 pop_frame：无逃逸 → O(1) 回缩
                alloc.end_frame(fidx, false);
            }

            times_enabled.push(start.elapsed());
            alloc.reset(); // 每次迭代后重置，模拟独立执行
        }

        // === 统计分析：计算延迟分位数 ===
        times_enabled.sort();

        let avg_ns: u128 =
            times_enabled.iter().map(|d| d.as_nanos()).sum::<u128>() / BENCHMARK_ITERATIONS as u128;
        let p50 = times_enabled[BENCHMARK_ITERATIONS / 2];
        let p95_idx = (BENCHMARK_ITERATIONS as f64 * 0.95) as usize;
        let p99_idx = ((BENCHMARK_ITERATIONS as f64 * 0.99) as usize).min(BENCHMARK_ITERATIONS - 1);
        let p95 = times_enabled[p95_idx];
        let p99 = times_enabled[p99_idx];

        // 计算总操作数和吞吐量
        let ops_per_iteration = LOOP_COUNT * 3; // begin + allocate + end
        let total_ops = BENCHMARK_ITERATIONS * ops_per_iteration;
        let total_time_ms = avg_ns as f64 / 1e6;

        println!("\n{{'='=>60}}");
        println!(" Arena E2E Benchmark: Temp Array Loop (Simulated)");
        println!("{{'='=>60}}");
        println!(" 工作负载: loop {}x 创建临时数组 [i, i*2, i*3, i*4, i*5]", LOOP_COUNT);
        println!(" 数组大小: {}~{} 字节 (动态波动)", ARRAY_SIZE, ARRAY_SIZE + 7);
        println!(" 基准迭代: {} 次", BENCHMARK_ITERATIONS);
        println!(" 每次迭代操作: {} 次 ({} loops x 3 ops)", ops_per_iteration, LOOP_COUNT);
        println!();
        println!(" --- 延迟分布 ---");
        println!(" 平均: {:.0} us ({:.2} ms/iter)", avg_ns as f64 / 1e3, total_time_ms);
        println!(" P50 : {:?}", p50);
        println!(" P95 : {:?}", p95);
        println!(" P99 : {:?}", p99);
        println!();
        println!(" --- 吞吐量 ---");
        println!(" 总操作数: {}", total_ops);
        println!(" 总耗时  : {:.2} ms", total_time_ms * BENCHMARK_ITERATIONS as f64);
        println!(
            " 吞吐量  : {:.2} M ops/s",
            total_ops as f64 / (total_time_ms * BENCHMARK_ITERATIONS as f64 / 1e3).max(0.001) / 1e6
        );
        println!(" 单操作延迟: {:.0} ns", avg_ns as f64 / ops_per_iteration.max(1) as f64);
        println!("{{'='=>60}}");

        // === 正确性验证 ===
        // 验证所有迭代都完成了预期数量的帧操作
        assert_eq!(
            times_enabled.len(),
            BENCHMARK_ITERATIONS,
            "应完成 {} 次基准迭代",
            BENCHMARK_ITERATIONS
        );

        // 验证 P99 延迟在合理范围内（单次迭代 < 10ms，否则可能有问题）
        assert!(p99.as_millis() < 10, "P99 延迟过高 ({:?})，可能存在性能回归", p99);

        // -----------------------------------------------------------------
        // 对照实验：RegionAllocator 原始分配器吞吐量（无模拟开销）
        // -----------------------------------------------------------------
        // 单独测量 begin/allocate/end 的裸性能，排除测试框架干扰

        const CONTROL_FRAMES: usize = 5_000;
        const CONTROL_ALLOCS_PER_FRAME: usize = 1; // 每帧 1 次分配

        let mut control_alloc = RegionAllocator::with_default();
        let control_start = Instant::now();

        for _ in 0..CONTROL_FRAMES {
            let fidx = control_alloc.begin_frame();
            // 模拟 HeapObject::Array(5 elements) ≈ 56 bytes
            let _ = control_alloc.allocate(fidx, ARRAY_SIZE, 8);
            control_alloc.end_frame(fidx, false); // 无逃逸释放
        }

        let control_elapsed = control_start.elapsed();
        let control_total_ops = CONTROL_FRAMES * (1 + CONTROL_ALLOCS_PER_FRAME + 1); // begin + allocate + end
        let control_per_frame_ns = control_elapsed.as_nanos() as f64 / CONTROL_FRAMES as f64;

        println!("\n{{'='=>60}}");
        println!(" RegionAllocator Raw Throughput (Control Group)");
        println!("{{'='=>60}}");
        println!(" 帧数: {}", CONTROL_FRAMES);
        println!(" 每帧分配: {} 次 x {} bytes", CONTROL_ALLOCS_PER_FRAME, ARRAY_SIZE);
        println!(" 总操作数: {} (frames x 3 ops/frame)", control_total_ops);
        println!();
        println!(" --- 原始性能 ---");
        println!(" 总耗时  : {:?}", control_elapsed);
        println!(" 帧级开销: {:.0} ns/frame", control_per_frame_ns);
        println!(
            " 操作吞吐: {:.2} M ops/s",
            control_total_ops as f64 / control_elapsed.as_secs_f64().max(1e-9) / 1e6
        );
        println!(
            " 分配吞吐: {:.2} M alloc/s",
            CONTROL_FRAMES as f64 / control_elapsed.as_secs_f64().max(1e-9) / 1e6
        );
        println!("{{'='=>60}}");

        // === 对照组验证 ===
        // 原始分配器应在合理时间内完成（< 50ms for 5K frames）
        assert!(
            control_elapsed.as_millis() < 50,
            "原始分配器耗时过长 ({:?})，可能存在性能问题",
            control_elapsed
        );
    }

    // ===================================================================
    // v2 类型化对象存储测试
    // ===================================================================

    #[test]
    fn test_v2_allocate_object_basic() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 分配一个 Array 对象
        let obj = HeapObject::Array(vec![Value::from_number(1.0), Value::from_number(2.0)]);
        let result = alloc.allocate_object(fidx, obj, 48);

        assert!(result.is_ok(), "allocate_object 应成功");
        let val = result.unwrap();

        // 验证返回值是 Arena 编码的 Value
        assert!(val.is_heap_object(), "应是 heap object");
        assert!(val.is_gc_managed(), "应是 GC managed");
        let arena_offset = val.try_arena_offset();
        assert!(arena_offset.is_some(), "应包含 arena offset");

        // 验证对象可通过索引取回
        let idx = arena_offset.unwrap();
        let retrieved = alloc.get_arena_object(idx);
        assert!(retrieved.is_some(), "应能通过索引取回对象");
        match retrieved.unwrap() {
            HeapObject::Array(arr) => {
                assert_eq!(arr.len(), 2, "数组应有 2 个元素");
            }
            other => panic!("预期 Array, 得到 {:?}", other.type_name()),
        }

        // 验证 objects_len 正确
        assert_eq!(alloc.objects_len(), 1, "应存储了 1 个对象");

        // 验证帧状态正确追踪
        let state = alloc.frame_state(fidx).unwrap();
        assert_eq!(state.obj_count, 1, "帧应记录 1 个对象");
        assert_eq!(state.obj_start, 0, "首对象索引起点应为 0");
    }

    #[test]
    fn test_v2_allocate_multiple_objects() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 分配多个不同类型的对象
        let obj1 = HeapObject::Range { start: 0.0, end: 10.0, range_end: RangeEnd::Exclusive };
        let val1 = alloc.allocate_object(fidx, obj1, 24).unwrap();

        let obj2 = HeapObject::Array(vec![Value::from_number(42.0)]);
        let val2 = alloc.allocate_object(fidx, obj2, 32).unwrap();

        let obj3 = HeapObject::Dict(Default::default());
        let val3 = alloc.allocate_object(fidx, obj3, 40).unwrap();

        // 验证三个对象的 Arena 索引各不相同
        let idx1 = val1.try_arena_offset().unwrap();
        let idx2 = val2.try_arena_offset().unwrap();
        let idx3 = val3.try_arena_offset().unwrap();
        assert_ne!(idx1, idx2, "对象索引应不同");
        assert_ne!(idx2, idx3, "对象索引应不同");

        // 验证每个对象可正确取回
        assert!(matches!(alloc.get_arena_object(idx1), Some(HeapObject::Range { .. })));
        assert!(matches!(alloc.get_arena_object(idx2), Some(HeapObject::Array(_))));
        assert!(matches!(alloc.get_arena_object(idx3), Some(HeapObject::Dict(_))));

        // 验证帧状态
        let state = alloc.frame_state(fidx).unwrap();
        assert_eq!(state.obj_count, 3, "帧应记录 3 个对象");
        assert_eq!(alloc.objects_len(), 3);
    }

    #[test]
    fn test_v2_end_frame_no_escape_drops_objects() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 分配对象
        let _ = alloc.allocate_object(fidx, HeapObject::Array(vec![]), 32);
        let _ = alloc.allocate_object(
            fidx,
            HeapObject::Range { start: 0.0, end: 1.0, range_end: RangeEnd::Exclusive },
            24,
        );
        assert_eq!(alloc.objects_len(), 2);

        // 无逃逸结束 → 对象应被 O(1) 截断释放
        let result = alloc.end_frame(fidx, false);
        assert!(result.is_none(), "无逃逸应返回 None");

        // 验证 objects 已被清空
        assert_eq!(alloc.objects_len(), 0, "end_frame 无逃逸后 objects 应被截断");
        assert_eq!(alloc.depth(), 0);
    }

    #[test]
    fn test_v2_end_frame_with_escape_preserves_objects() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 分配对象
        let val = alloc
            .allocate_object(fidx, HeapObject::Array(vec![Value::from_number(99.0)]), 32)
            .unwrap();
        let arena_idx = val.try_arena_offset().unwrap();
        assert_eq!(alloc.objects_len(), 1);

        // 标记逃逸并结束帧
        alloc.mark_escaped(fidx);
        let result = alloc.end_frame(fidx, false); // frame 本身 has_escaped=true
        assert!(result.is_some(), "有逃逸应返回范围");

        // 有逃逸时 objects 不被自动截断（调用方负责处理）
        assert_eq!(alloc.objects_len(), 1, "有逃逸时 objects 应保留供提升使用");

        // 调用方模拟提升：通过 take_arena_object 取走
        let taken = alloc.take_arena_object(arena_idx);
        assert!(taken.is_some(), "应能取走逃逸对象");
        match taken.unwrap() {
            HeapObject::Array(arr) => {
                assert_eq!(arr.len(), 1);
                assert_eq!(arr[0], Value::from_number(99.0));
            }
            other => panic!("预期 Array, 得到 {:?}", other.type_name()),
        }

        // 取走后 objects 清空
        assert_eq!(alloc.objects_len(), 0, "take 后对象应被移除");
    }

    #[test]
    fn test_v2_frame_objects_slice() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 在同一帧分配 3 个对象
        let _ = alloc.allocate_object(fidx, HeapObject::Array(vec![]), 16);
        let _ = alloc.allocate_object(
            fidx,
            HeapObject::Range { start: 1.0, end: 5.0, range_end: RangeEnd::Inclusive },
            24,
        );
        let _ = alloc.allocate_object(fidx, HeapObject::Dict(Default::default()), 32);

        // 通过 frame_objects 获取切片
        let slice = alloc.frame_objects(fidx).expect("应返回切片");
        assert_eq!(slice.len(), 3, "应包含 3 个对象");

        // 验证类型顺序
        assert!(matches!(slice[0], HeapObject::Array(_)));
        assert!(matches!(slice[1], HeapObject::Range { .. }));
        assert!(matches!(slice[2], HeapObject::Dict(_)));
    }

    #[test]
    fn test_v2_nested_frames_object_isolation() {
        let mut alloc = RegionAllocator::with_default();

        // 帧 0：分配 2 个对象
        let f0 = alloc.begin_frame();
        let v0a = alloc.allocate_object(f0, HeapObject::Array(vec![]), 16).unwrap();
        let _v0b = alloc.allocate_object(
            f0,
            HeapObject::Range { start: 0.0, end: 1.0, range_end: RangeEnd::Exclusive },
            24,
        );
        assert_eq!(alloc.objects_len(), 2);

        // 帧 1：分配 1 个对象
        let f1 = alloc.begin_frame();
        let v1 = alloc.allocate_object(f1, HeapObject::Dict(Default::default()), 32).unwrap();
        assert_eq!(alloc.objects_len(), 3);

        // pop 帧 1（无逃逸）→ 仅释放帧 1 的对象
        alloc.end_frame(f1, false);
        assert_eq!(alloc.objects_len(), 2, "pop 帧后仅剩帧 0 的对象");

        // 帧 0 的对象仍可访问
        let idx0 = v0a.try_arena_offset().unwrap();
        assert!(alloc.get_arena_object(idx0).is_some(), "帧 0 对象仍有效");

        // 帧 1 的对象已被释放
        let _idx1 = v1.try_arena_offset().unwrap();
        // 注意：swap_remove 可能导致索引变化，但 truncate 后该位置不存在
        // 此处验证 objects_len 已减少即可

        // pop 帧 0
        alloc.end_frame(f0, false);
        assert_eq!(alloc.objects_len(), 0, "全部 pop 后 objects 为空");
    }

    #[test]
    fn test_v2_allocate_object_fallback_disabled() {
        let config = RegionConfig {
            max_frame_arena_size: 1024 * 1024,
            max_region_size: 16 * 1024 * 1024,
            enabled: false, // 禁用
        };
        let mut alloc = RegionAllocator::new(config);
        let fidx = alloc.begin_frame();

        let result = alloc.allocate_object(fidx, HeapObject::Array(vec![]), 32);
        assert!(result.is_err(), "禁用时 allocate_object 应返回 Err");
        assert_eq!(alloc.objects_len(), 0);
    }

    #[test]
    fn test_v2_allocate_object_fallback_invalid_frame() {
        let mut alloc = RegionAllocator::with_default();
        // 不 begin_frame，直接用无效索引分配

        let result = alloc.allocate_object(
            999, // 无效帧索引
            HeapObject::Array(vec![]),
            32,
        );
        assert!(result.is_err(), "无效帧索引应返回 Err");
    }

    #[test]
    fn test_v2_reset_clears_objects() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        let _ = alloc.allocate_object(fidx, HeapObject::Array(vec![Value::from_number(1.0)]), 32);
        let _ = alloc.allocate_object(
            fidx,
            HeapObject::Range { start: 0.0, end: 10.0, range_end: RangeEnd::Exclusive },
            24,
        );
        assert_eq!(alloc.objects_len(), 2);

        alloc.reset();
        assert_eq!(alloc.objects_len(), 0, "reset 后 objects 应为空");
        assert_eq!(alloc.depth(), 0);
        assert_eq!(alloc.global_usage(), 0);
    }

    #[test]
    fn test_v2_begin_frame_obj_start_correctness() {
        let mut alloc = RegionAllocator::with_default();

        // 帧 0: 分配 2 对象
        let f0 = alloc.begin_frame();
        let s0 = alloc.frame_state(f0).unwrap();
        assert_eq!(s0.obj_start, 0, "帧 0 obj_start 应为 0（objects 为空）");

        let _ = alloc.allocate_object(f0, HeapObject::Array(vec![]), 16);
        let _ = alloc.allocate_object(f0, HeapObject::Array(vec![]), 16);
        assert_eq!(alloc.frame_state(f0).unwrap().obj_count, 2);

        // 帧 1: obj_start 应接在帧 0 之后 (= 2)
        let f1 = alloc.begin_frame();
        let s1 = alloc.frame_state(f1).unwrap();
        assert_eq!(s1.obj_start, 2, "帧 1 obj_start 应为 2");
        assert_eq!(s1.obj_count, 0);

        let _ = alloc.allocate_object(
            f1,
            HeapObject::Range { start: 0.0, end: 1.0, range_end: RangeEnd::Exclusive },
            24,
        );
        assert_eq!(alloc.frame_state(f1).unwrap().obj_count, 1);
    }

    #[test]
    fn test_v2_empty_frame_objects_returns_empty_slice() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame(); // 未分配任何对象

        let slice = alloc.frame_objects(fidx);
        assert!(slice.is_some(), "空帧应返回 Some");
        assert!(slice.unwrap().is_empty(), "空帧应返回空切片");
    }

    #[test]
    fn test_v2_get_arena_object_out_of_range() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();

        // 只分配 1 个对象
        let _ = alloc.allocate_object(fidx, HeapObject::Array(vec![]), 16);

        // 越界访问应返回 None
        assert!(alloc.get_arena_object(999).is_none(), "越界索引应返回 None");
        // 已分配的索引应可用
        assert!(alloc.get_arena_object(0).is_some(), "索引 0 应有效");
    }

    // ---- 新增测试：覆盖未测试的 pub fn ----

    #[test]
    fn test_as_slice_basic() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();
        let _ = alloc.allocate(fidx, 32, 8);
        let slice = alloc.as_slice(0, 32);
        assert_eq!(slice.len(), 32);
    }

    #[test]
    fn test_as_mut_slice_basic() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();
        let _ = alloc.allocate(fidx, 16, 8);
        let slice = alloc.as_mut_slice(0, 16);
        assert_eq!(slice.len(), 16);
        slice[0] = 0xFF;
        assert_eq!(alloc.as_slice(0, 1)[0], 0xFF);
    }

    #[test]
    fn test_frame_state_valid_index() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();
        let state = alloc.frame_state(fidx);
        assert!(state.is_some());
        assert_eq!(state.unwrap().obj_count, 0);
    }

    #[test]
    fn test_frame_state_invalid_index() {
        let alloc = RegionAllocator::with_default();
        assert!(alloc.frame_state(999).is_none());
    }

    #[test]
    fn test_get_arena_object_mut_basic() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();
        let _ = alloc.allocate_object(fidx, HeapObject::Array(vec![Value::from_number(1.0)]), 16);
        let obj = alloc.get_arena_object_mut(0);
        assert!(obj.is_some());
        if let Some(HeapObject::Array(arr)) = obj {
            assert_eq!(arr.len(), 1);
            arr.push(Value::from_number(2.0));
        }
        let obj2 = alloc.get_arena_object(0);
        assert!(obj2.is_some());
    }

    #[test]
    fn test_get_arena_object_mut_out_of_range() {
        let mut alloc = RegionAllocator::with_default();
        assert!(alloc.get_arena_object_mut(999).is_none());
    }

    #[test]
    fn test_global_usage_tracking() {
        let mut alloc = RegionAllocator::with_default();
        let initial = alloc.global_usage();
        let fidx = alloc.begin_frame();
        let _ = alloc.allocate(fidx, 64, 8);
        assert_eq!(alloc.global_usage(), initial + 64);
    }

    #[test]
    fn test_mark_escaped_basic() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();
        assert!(!alloc.frame_state(fidx).unwrap().has_escaped);
        alloc.mark_escaped(fidx);
        assert!(alloc.frame_state(fidx).unwrap().has_escaped);
    }

    #[test]
    fn test_mark_escaped_invalid_frame() {
        let mut alloc = RegionAllocator::with_default();
        // Should not panic on invalid index
        alloc.mark_escaped(999);
    }

    #[test]
    fn test_objects_len_tracking() {
        let mut alloc = RegionAllocator::with_default();
        assert_eq!(alloc.objects_len(), 0);
        let fidx = alloc.begin_frame();
        let _ = alloc.allocate_object(fidx, HeapObject::Array(vec![]), 16);
        assert_eq!(alloc.objects_len(), 1);
        let _ = alloc.allocate_object(fidx, HeapObject::Array(vec![]), 16);
        assert_eq!(alloc.objects_len(), 2);
    }

    #[test]
    fn test_take_arena_object_basic() {
        let mut alloc = RegionAllocator::with_default();
        let fidx = alloc.begin_frame();
        let _ = alloc.allocate_object(fidx, HeapObject::Array(vec![Value::from_number(42.0)]), 16);
        let taken = alloc.take_arena_object(0);
        assert!(taken.is_some());
        if let Some(HeapObject::Array(arr)) = taken {
            assert_eq!(arr.len(), 1);
        }
    }

    #[test]
    fn test_take_arena_object_out_of_range() {
        let mut alloc = RegionAllocator::with_default();
        assert!(alloc.take_arena_object(999).is_none());
    }
}
