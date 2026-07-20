//! # Nuzo VM 缓存系统 - 统一缓存管理架构
//!
//! 本模块实现了 VM 层面的多级缓存系统，包含三个核心子系统：
//!
//! ## 子系统概览
//!
//! ### 1. 字符串常量池（StringConstantPool）
//! - **用途**: 字符串驻留（String Interning），消除重复字符串的内存开销
//! - **数据结构**: `XxHashMap<String, u32>` (字符串 → 唯一 ID)
//! - **时间复杂度**: O(1) 查找和插入（均摊）
//! - **适用场景**: 标识符、字面量字符串、属性名的去重
//!
//! ### 2. 内联缓存（InlineCache）
//! - **用途**: 属性访问加速，基于 Shape 的偏移量缓存
//! - **状态机**: Uninitialized → Monomorphic → Polymorphic → Megamorphic
//! - **设计灵感**: V8/SpiderMonkey 的 IC 机制
//! - **性能提升**: 将 O(n) 属性查找降为 O(1) 缓存命中
//!
//! ### 3. 字节码缓存（BytecodeCache）
//! - **用途**: 已编译字节码的持久化存储，避免重复编译
//! - **键**: 源码的 xxHash3 哈希值（SourceHash）
//! - **值**: 完整的 Chunk 对象（含常量表和指令流）
//! - **淘汰策略**: 容量满时拒绝新条目（非 LRU）
//!
//! ## 统一管理器（CacheManager）
//!
//! `CacheManager` 作为门面模式（Facade Pattern）的实现，
//! 协调三个子系统的生命周期：
//!
//! ```text
//! CacheManager
//! ├── strings: StringConstantPool    → 字符串驻留
//! ├── bytecode: BytecodeCache        → 字节码缓存
//! └── inline_caches: XxHashMap<String, InlineCache> → 属性级 IC
//! ```
//!
//! ## 缓存策略设计决策
//!
//! ### 为什么选择 HashMap 而非 LRU？
//!
//! **BytecodeCache** 使用简单的容量限制而非 LRU 淘汰：
//! - **理由 1**: 字节码编译成本高，但编译次数相对较少
//! - **理由 2**: 避免维护额外的时间戳或链表结构
//! - **理由 3**: 简化实现复杂度，减少并发竞争点
//! - **权衡**: 在内存受限场景下可能需要手动清理
//!
//! ### InlineCache 的渐进式升级策略
//!
//! 采用**保守升级**（Conservative Promotion）原则：
//! - 只有在明确检测到多态行为时才升级
//! - 单态假设对大多数动态语言程序成立（>85% 的调用点是单态的）
//! - 避免过早优化导致的内存浪费
//!
//! ## 内存占用估算
//!
//! | 子系统 | 空条目开销 | 每条目增量 | 典型配置下的总占用 |
//! |--------|-----------|-----------|------------------|
//! | StringPool | ~200B | ~52B (String + u32) | ~50KB (1000 strings) |
//! | BytecodeCache | ~100B | ~1-10KB (Chunk) | ~1MB (100 chunks) |
//! | InlineCache | ~100B | ~48B per shape entry | ~5KB (100 properties) |
//!
//! ## 性能敏感代码标注
//!
//! 以下方法位于热路径中，已进行特殊优化：
//!
//! - [`InlineCache::lookup_or_update()`][]: 内联候选，避免函数调用开销
//! - [`StringConstantPool::intern()`][]: 热路径上的字符串处理
//! - [`BytecodeCache::lookup()`][]: 编译结果的快速检索
//!
//! 这些方法都标记为 `#[inline]` 或使用内联友好的编码风格。

use nuzo_core::{XxHashMap, xx_hash_map, xx_hash_map_new, xxh3_64};
use std::fmt;

use nuzo_bytecode::Chunk;

pub type ShapeId = usize;
pub type PropertyOffset = usize;
const DEFAULT_STRING_POOL_CAPACITY: usize = 1024;

/// BytecodeCache 默认最大条目数
const BYTECODE_CACHE_DEFAULT_CAPACITY: usize = 256;

// ============================================================================
// 错误类型定义
// ============================================================================

/// 缓存系统错误类型
#[derive(Debug, Clone, PartialEq)]
pub enum CacheError {
    /// 字符串常量池已满
    StringPoolFull { current_size: usize, max_capacity: usize },
    /// 无效的内联缓存状态转换
    InvalidICStateTransition { from: &'static str, to: &'static str },
    /// 字节码缓存失效失败
    BytecodeCacheInvalidationFailed(String),
    /// 通用缓存错误
    GenericError(String),
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CacheError::StringPoolFull { current_size, max_capacity } => {
                write!(
                    f,
                    "String constant pool full: current size {}, max capacity {}",
                    current_size, max_capacity
                )
            }
            CacheError::InvalidICStateTransition { from, to } => {
                write!(f, "Invalid IC state transition: {} -> {}", from, to)
            }
            CacheError::BytecodeCacheInvalidationFailed(reason) => {
                write!(f, "Bytecode cache invalidation failed: {}", reason)
            }
            CacheError::GenericError(msg) => {
                write!(f, "Cache error: {}", msg)
            }
        }
    }
}

impl std::error::Error for CacheError {}

// ============================================================================
// StringConstantPool - 字符串常量池
// ============================================================================

/// 字符串常量池，用于字符串驻留(interning)。
///
/// 通过 HashMap 实现 O(1) 的字符串查找和去重，
/// 减少内存使用并加速字符串比较。
pub struct StringConstantPool {
    pool: XxHashMap<String, u32>,
    next_idx: u32,
    max_capacity: usize,
    lookups: usize,
    hits: usize,
}

impl StringConstantPool {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_STRING_POOL_CAPACITY)
    }

    pub fn with_capacity(max_capacity: usize) -> Self {
        StringConstantPool {
            pool: xx_hash_map_new(),
            next_idx: 0,
            max_capacity: if max_capacity == 0 {
                DEFAULT_STRING_POOL_CAPACITY
            } else {
                max_capacity
            },
            lookups: 0,
            hits: 0,
        }
    }

    pub fn intern(&mut self, s: &str) -> Result<u32, CacheError> {
        self.lookups += 1;

        if let Some(&existing_idx) = self.pool.get(s) {
            self.hits += 1;
            return Ok(existing_idx);
        }

        if self.pool.len() >= self.max_capacity {
            return Err(CacheError::StringPoolFull {
                current_size: self.pool.len(),
                max_capacity: self.max_capacity,
            });
        }

        let new_idx = self.next_idx;
        self.next_idx += 1;
        self.pool.insert(s.to_string(), new_idx);

        Ok(new_idx)
    }

    /// 检查字符串是否已在池中，返回其索引
    #[inline]
    pub fn contains(&self, s: &str) -> Option<u32> {
        self.pool.get(s).copied()
    }

    /// 返回池中字符串数量
    #[inline]
    pub fn len(&self) -> usize {
        self.pool.len()
    }

    /// 池是否为空
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }

    /// 返回缓存命中率 (0.0 ~ 1.0)
    pub fn hit_rate(&self) -> f64 {
        if self.lookups == 0 { 0.0 } else { self.hits as f64 / self.lookups as f64 }
    }

    /// 返回统计信息：(size, lookups, hits, hit_rate)
    pub fn stats(&self) -> (usize, usize, usize, f64) {
        (self.pool.len(), self.lookups, self.hits, self.hit_rate())
    }

    /// 清空常量池并重置统计
    pub fn clear(&mut self) {
        self.pool.clear();
        self.next_idx = 0;
        self.lookups = 0;
        self.hits = 0;
    }

    /// 返回最大容量
    #[inline]
    pub fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    /// 设置最大容量
    pub fn max_capacity_mut(&mut self, capacity: usize) {
        self.max_capacity = if capacity == 0 { DEFAULT_STRING_POOL_CAPACITY } else { capacity };
    }

    /// 设置最大容量（Builder 风格）
    ///
    /// 使用 `with_` 前缀表示配置方法。
    pub fn with_max_capacity(&mut self, capacity: usize) {
        self.max_capacity_mut(capacity);
    }
}

impl Default for StringConstantPool {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for StringConstantPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StringConstantPool")
            .field("size", &self.pool.len())
            .field("max_capacity", &self.max_capacity)
            .field("lookups", &self.lookups)
            .field("hits", &self.hits)
            .field("hit_rate", &self.hit_rate())
            .finish()
    }
}

// ============================================================================
// InlineCache - 内联缓存
// ============================================================================

/// 内联缓存状态，类似 V8 的 IC (Inline Cache) 机制。
///
/// 状态转换：Uninitialized → Monomorphic → Polymorphic → Megamorphic
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ICState {
    /// 未初始化
    #[default]
    Uninitialized,
    /// 单态 - 缓存了一个 shape
    Monomorphic { shape_id: ShapeId, offset: PropertyOffset },
    /// 多态 - 缓存了少量 shape（≤ 4）
    Polymorphic { entries: Vec<(ShapeId, PropertyOffset)> },
    /// 超态 - 缓存了大量 shape，使用 HashMap
    Megamorphic { entries: XxHashMap<ShapeId, PropertyOffset> },
}

impl ICState {
    /// 返回状态名称
    pub fn name(&self) -> &'static str {
        match self {
            ICState::Uninitialized => "Uninitialized",
            ICState::Monomorphic { .. } => "Monomorphic",
            ICState::Polymorphic { .. } => "Polymorphic",
            ICState::Megamorphic { .. } => "Megamorphic",
        }
    }

    pub fn is_uninitialized(&self) -> bool {
        matches!(self, ICState::Uninitialized)
    }

    pub fn is_monomorphic(&self) -> bool {
        matches!(self, ICState::Monomorphic { .. })
    }

    pub fn is_polymorphic(&self) -> bool {
        matches!(self, ICState::Polymorphic { .. })
    }

    pub fn is_megamorphic(&self) -> bool {
        matches!(self, ICState::Megamorphic { .. })
    }

    pub fn entry_count(&self) -> usize {
        match self {
            ICState::Uninitialized => 0,
            ICState::Monomorphic { .. } => 1,
            ICState::Polymorphic { entries } => entries.len(),
            ICState::Megamorphic { entries } => entries.len(),
        }
    }
}

impl fmt::Display for ICState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ICState::Uninitialized => write!(f, "IC[Uninitialized]"),
            ICState::Monomorphic { shape_id, offset } => {
                write!(f, "IC[Monomorphic: shape={}, offset={}]", shape_id, offset)
            }
            ICState::Polymorphic { entries } => {
                write!(f, "IC[Polymorphic: {} entries]", entries.len())
            }
            ICState::Megamorphic { entries } => {
                write!(f, "IC[Megamorphic: {} entries]", entries.len())
            }
        }
    }
}

/// 多态缓存最大条目数
const POLYMORPHIC_MAX_ENTRIES: usize = 4;

/// 内联缓存实例
///
/// 用于优化属性访问。跟踪对象 shape 并缓存属性偏移量。
/// 当 shape 变化超出多态阈值时，退化为超态（megamorphic）。
pub struct InlineCache {
    state: ICState,
    total_lookups: usize,
    cache_hits: usize,
    state_transitions: usize,
}

impl InlineCache {
    pub fn new() -> Self {
        InlineCache {
            state: ICState::Uninitialized,
            total_lookups: 0,
            cache_hits: 0,
            state_transitions: 0,
        }
    }

    /// 查找缓存或通过回调获取偏移量并更新缓存。
    ///
    /// 如果缓存命中，直接返回偏移量；否则调用 `lookup_fn` 获取偏移量并缓存。
    pub fn lookup_or_update<F>(&mut self, shape_id: ShapeId, lookup_fn: F) -> Option<PropertyOffset>
    where
        F: FnOnce(ShapeId) -> Option<PropertyOffset>,
    {
        self.total_lookups += 1;

        // 尝试在当前状态中查找
        if let Some(offset) = self.lookup_in_state(shape_id) {
            self.cache_hits += 1;
            return Some(offset);
        }

        let offset = lookup_fn(shape_id)?;

        self.update_cache(shape_id, offset);

        Some(offset)
    }

    fn lookup_in_state(&self, shape_id: ShapeId) -> Option<PropertyOffset> {
        match &self.state {
            ICState::Uninitialized => None,

            ICState::Monomorphic { shape_id: cached_id, offset } => {
                if *cached_id == shape_id {
                    Some(*offset)
                } else {
                    None
                }
            }

            ICState::Polymorphic { entries } => {
                entries.iter().find(|(id, _)| *id == shape_id).map(|(_, offset)| *offset)
            }

            ICState::Megamorphic { entries } => entries.get(&shape_id).copied(),
        }
    }

    fn update_cache(&mut self, shape_id: ShapeId, offset: PropertyOffset) {
        // Move the whole state out (ICState: Default → Uninitialized) so we can
        // mutate the Polymorphic/Megamorphic collections in place instead of
        // cloning them on every cache update. The old state is replaced with
        // Uninitialized during the match; if any allocation below panics,
        // `self.state` simply remains Uninitialized (acceptable on unwind).
        let old_state = std::mem::take(&mut self.state);
        let old_discriminant = std::mem::discriminant(&old_state);

        let new_state = match old_state {
            ICState::Uninitialized => ICState::Monomorphic { shape_id, offset },

            ICState::Monomorphic { shape_id: cached_id, offset: cached_offset } => {
                if cached_id == shape_id {
                    ICState::Monomorphic { shape_id, offset }
                } else {
                    let entries = vec![(cached_id, cached_offset), (shape_id, offset)];
                    ICState::Polymorphic { entries }
                }
            }

            ICState::Polymorphic { mut entries } => {
                if let Some(entry) = entries.iter_mut().find(|(id, _)| *id == shape_id) {
                    entry.1 = offset;
                    ICState::Polymorphic { entries }
                } else if entries.len() < POLYMORPHIC_MAX_ENTRIES {
                    entries.push((shape_id, offset));
                    ICState::Polymorphic { entries }
                } else {
                    // Promote to Megamorphic: drain the Vec into a HashMap.
                    let mut map = xx_hash_map(entries.len() + 1);
                    for (id, off) in entries {
                        map.insert(id, off);
                    }
                    map.insert(shape_id, offset);
                    ICState::Megamorphic { entries: map }
                }
            }

            ICState::Megamorphic { mut entries } => {
                entries.insert(shape_id, offset);
                ICState::Megamorphic { entries }
            }
        };

        if std::mem::discriminant(&new_state) != old_discriminant {
            self.state_transitions += 1;
        }

        self.state = new_state;
    }

    pub fn update(&mut self, shape_id: ShapeId, offset: PropertyOffset) {
        self.update_cache(shape_id, offset);
    }

    pub fn reset(&mut self) {
        self.state = ICState::Uninitialized;
        self.state_transitions += 1;
    }

    pub fn state(&self) -> &ICState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut ICState {
        &mut self.state
    }

    pub fn hit_rate(&self) -> f64 {
        if self.total_lookups == 0 {
            0.0
        } else {
            self.cache_hits as f64 / self.total_lookups as f64
        }
    }

    pub fn stats(&self) -> (&'static str, usize, usize, f64, usize) {
        (
            self.state.name(),
            self.total_lookups,
            self.cache_hits,
            self.hit_rate(),
            self.state_transitions,
        )
    }

    pub fn invalidate(&mut self) {
        self.state = ICState::Uninitialized;
        self.state_transitions += 1;
    }

    pub fn reset_stats(&mut self) {
        self.total_lookups = 0;
        self.cache_hits = 0;
        self.state_transitions = 0;
    }
}

impl Default for InlineCache {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for InlineCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InlineCache")
            .field("state", &self.state)
            .field("total_lookups", &self.total_lookups)
            .field("cache_hits", &self.cache_hits)
            .field("hit_rate", &self.hit_rate())
            .field("state_transitions", &self.state_transitions)
            .finish()
    }
}

impl fmt::Display for InlineCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "InlineCache[{}, lookups={}, hits={:.2}%]",
            self.state(),
            self.total_lookups,
            self.hit_rate() * 100.0
        )
    }
}

// ============================================================================
// BytecodeCache - 字节码缓存
// ============================================================================

/// 源码哈希值，用于字节码缓存的键
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceHash(u64);

impl SourceHash {
    /// Compute the source hash using xxHash3 (64-bit).
    ///
    /// xxHash3 has lower collision rates than FNV-1a and is faster than SipHash,
    /// making it suitable for bytecode cache keys where stability across runs
    /// matters (so cached bytecode files don't get invalidated across versions).
    pub fn compute(source: &[u8]) -> Self {
        SourceHash(xxh3_64(source))
    }

    pub fn compute_str(source: &str) -> Self {
        Self::compute(source.as_bytes())
    }

    pub fn value(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for SourceHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

struct CacheEntry {
    chunk: Chunk,
    last_accessed: web_time::Instant,
    access_count: usize,
}

/// 字节码缓存，按源码哈希缓存已编译的 Chunk。
pub struct BytecodeCache {
    cache: XxHashMap<SourceHash, CacheEntry>,
    max_entries: usize,
    lookups: usize,
    hits: usize,
    invalidations: usize,
    evictions: usize,
}

impl BytecodeCache {
    pub fn new() -> Self {
        Self::with_capacity(BYTECODE_CACHE_DEFAULT_CAPACITY)
    }

    pub fn with_capacity(max_entries: usize) -> Self {
        BytecodeCache {
            cache: xx_hash_map_new(),
            max_entries: if max_entries == 0 {
                BYTECODE_CACHE_DEFAULT_CAPACITY
            } else {
                max_entries
            },
            lookups: 0,
            hits: 0,
            invalidations: 0,
            evictions: 0,
        }
    }

    pub fn cache(&mut self, hash: &SourceHash, chunk: Chunk) -> Result<(), CacheError> {
        if !self.cache.contains_key(hash) && self.cache.len() >= self.max_entries {
            self.evictions += 1;
            return Err(CacheError::GenericError(format!(
                "Bytecode cache full: current {} entries, max capacity {}",
                self.cache.len(),
                self.max_entries
            )));
        }

        let entry = CacheEntry { chunk, last_accessed: web_time::Instant::now(), access_count: 0 };

        self.cache.insert(*hash, entry);

        Ok(())
    }

    pub fn lookup(&mut self, hash: &SourceHash) -> Option<&Chunk> {
        self.lookups += 1;

        if let Some(entry) = self.cache.get_mut(hash) {
            entry.last_accessed = web_time::Instant::now();
            entry.access_count += 1;
            self.hits += 1;
            Some(&entry.chunk)
        } else {
            None
        }
    }

    pub fn lookup_mut(&mut self, hash: &SourceHash) -> Option<&mut Chunk> {
        self.lookups += 1;

        if let Some(entry) = self.cache.get_mut(hash) {
            entry.last_accessed = web_time::Instant::now();
            entry.access_count += 1;
            self.hits += 1;
            Some(&mut entry.chunk)
        } else {
            None
        }
    }

    /// 使指定条目失效
    pub fn invalidate(&mut self, hash: &SourceHash) -> Result<bool, CacheError> {
        self.invalidations += 1;
        Ok(self.cache.remove(hash).is_some())
    }

    /// 批量使条目失效
    pub fn invalidate_batch(&mut self, hashes: &[SourceHash]) -> usize {
        let mut count = 0;
        for hash in hashes {
            if self.cache.remove(hash).is_some() {
                count += 1;
                self.invalidations += 1;
            }
        }
        count
    }

    /// 清空所有缓存
    pub fn clear(&mut self) {
        let count = self.cache.len();
        self.cache.clear();
        self.invalidations += count;
    }

    /// 检查缓存是否包含指定哈希
    pub fn contains(&self, hash: &SourceHash) -> bool {
        self.cache.contains_key(hash)
    }

    /// 返回缓存条目数
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// 缓存是否为空
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// 返回缓存命中率
    pub fn hit_rate(&self) -> f64 {
        if self.lookups == 0 { 0.0 } else { self.hits as f64 / self.lookups as f64 }
    }

    /// 返回统计信息：(entries, lookups, hits, hit_rate, invalidations, evictions)
    pub fn stats(&self) -> (usize, usize, usize, f64, usize, usize) {
        (
            self.cache.len(),
            self.lookups,
            self.hits,
            self.hit_rate(),
            self.invalidations,
            self.evictions,
        )
    }

    /// 返回访问次数最多的前 n 个条目
    pub fn top_accessed(&self, n: usize) -> Vec<(SourceHash, usize)> {
        let mut entries: Vec<(SourceHash, usize)> =
            self.cache.iter().map(|(hash, entry)| (*hash, entry.access_count)).collect();

        entries.sort_by_key(|b| std::cmp::Reverse(b.1));

        entries.into_iter().take(n).collect()
    }

    /// 设置最大容量
    pub fn max_capacity_mut(&mut self, capacity: usize) {
        self.max_entries = if capacity == 0 { BYTECODE_CACHE_DEFAULT_CAPACITY } else { capacity };
    }

    /// 设置最大容量（Builder 风格）
    ///
    /// 使用 `with_` 前缀表示配置方法。
    pub fn with_max_capacity(&mut self, capacity: usize) {
        self.max_capacity_mut(capacity);
    }

    /// 返回最大容量
    pub fn max_capacity(&self) -> usize {
        self.max_entries
    }

    /// 重置统计信息
    pub fn reset_stats(&mut self) {
        self.lookups = 0;
        self.hits = 0;
        self.invalidations = 0;
        self.evictions = 0;

        for entry in self.cache.values_mut() {
            entry.access_count = 0;
        }
    }
}

impl Default for BytecodeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for BytecodeCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BytecodeCache")
            .field("entries", &self.cache.len())
            .field("max_capacity", &self.max_entries)
            .field("lookups", &self.lookups)
            .field("hits", &self.hits)
            .field("hit_rate", &self.hit_rate())
            .field("invalidations", &self.invalidations)
            .field("evictions", &self.evictions)
            .finish()
    }
}

// ============================================================================
// CacheManager - 统一缓存管理器
// ============================================================================

/// 统一缓存管理器，协调所有缓存子系统。
///
/// 负责：
/// - 字符串常量池的生命周期管理
/// - 字节码缓存的管理
/// - 内联缓存的创建和失效
/// - 全局统计和健康检查
pub struct CacheManager {
    /// 字符串常量池
    strings: StringConstantPool,
    /// 字节码缓存
    bytecode: BytecodeCache,
    /// 按属性名索引的内联缓存
    inline_caches: XxHashMap<String, InlineCache>,
}

impl CacheManager {
    /// 创建默认配置的缓存管理器
    pub fn new() -> Self {
        CacheManager {
            strings: StringConstantPool::new(),
            bytecode: BytecodeCache::new(),
            inline_caches: xx_hash_map_new(),
        }
    }

    /// 创建指定配置的缓存管理器
    pub fn with_config(string_pool_capacity: usize, bytecode_cache_capacity: usize) -> Self {
        CacheManager {
            strings: StringConstantPool::with_capacity(string_pool_capacity),
            bytecode: BytecodeCache::with_capacity(bytecode_cache_capacity),
            inline_caches: xx_hash_map_new(),
        }
    }

    // ========================================================================
    // 字符串常量池访问器
    // ========================================================================

    /// 获取字符串常量池的不可变引用
    pub fn strings(&self) -> &StringConstantPool {
        &self.strings
    }

    /// 获取字符串常量池的可变引用
    pub fn strings_mut(&mut self) -> &mut StringConstantPool {
        &mut self.strings
    }

    // ========================================================================
    // 字节码缓存访问器
    // ========================================================================

    /// 获取字节码缓存的不可变引用
    pub fn bytecode(&self) -> &BytecodeCache {
        &self.bytecode
    }

    /// 获取字节码缓存的可变引用
    pub fn bytecode_mut(&mut self) -> &mut BytecodeCache {
        &mut self.bytecode
    }

    // ========================================================================
    // 内联缓存管理
    // ========================================================================

    /// 获取指定属性的内联缓存（不存在则创建）
    pub fn get_inline_cache(&mut self, property_name: &str) -> &mut InlineCache {
        self.inline_caches.entry(property_name.to_string()).or_default()
    }

    /// 使指定属性的内联缓存失效
    pub fn invalidate_inline_cache(&mut self, property_name: &str) -> bool {
        if let Some(ic) = self.inline_caches.get_mut(property_name) {
            ic.invalidate();
            true
        } else {
            false
        }
    }

    /// 使所有内联缓存失效
    pub fn invalidate_all_inline_caches(&mut self) {
        for ic in self.inline_caches.values_mut() {
            ic.invalidate();
        }
    }

    /// 返回内联缓存数量
    pub fn inline_cache_count(&self) -> usize {
        self.inline_caches.len()
    }

    // ========================================================================
    // 全局操作
    // ========================================================================

    /// 清空所有缓存
    pub fn clear_all(&mut self) {
        self.strings.clear();
        self.bytecode.clear();
        self.invalidate_all_inline_caches();
    }

    /// 重置所有统计信息
    pub fn reset_all_stats(&mut self) {
        // StringConstantPool 没有独立的 reset_stats，通过 clear 重置统计
        // 但 clear 会清空数据，所以这里只重置 bytecode 和 inline caches
        self.bytecode.reset_stats();

        for ic in self.inline_caches.values_mut() {
            ic.reset_stats();
        }
    }

    // ========================================================================
    // 统计信息
    // ========================================================================

    /// 获取全局缓存统计信息
    pub fn global_stats(&self) -> CacheGlobalStats {
        let (str_pool_size, str_lookups, str_hits, str_hit_rate) = self.strings.stats();
        let (bc_entries, bc_lookups, bc_hits, bc_hit_rate, bc_invals, bc_evicts) =
            self.bytecode.stats();

        // 计算内联缓存的聚合统计
        let mut ic_total_lookups = 0;
        let mut ic_total_hits = 0;
        let mut ic_total_transitions = 0;
        let mut ic_mono_count = 0;
        let mut ic_poly_count = 0;
        let mut ic_mega_count = 0;
        let mut ic_uninit_count = 0;

        for ic in self.inline_caches.values() {
            let (_, lookups, hits, _, transitions) = ic.stats();
            ic_total_lookups += lookups;
            ic_total_hits += hits;
            ic_total_transitions += transitions;

            match ic.state() {
                ICState::Uninitialized => ic_uninit_count += 1,
                ICState::Monomorphic { .. } => ic_mono_count += 1,
                ICState::Polymorphic { .. } => ic_poly_count += 1,
                ICState::Megamorphic { .. } => ic_mega_count += 1,
            }
        }

        let ic_hit_rate = if ic_total_lookups == 0 {
            0.0
        } else {
            ic_total_hits as f64 / ic_total_lookups as f64
        };

        CacheGlobalStats {
            // 字符串常量池统计
            string_pool_size: str_pool_size,
            string_pool_lookups: str_lookups,
            string_pool_hits: str_hits,
            string_pool_hit_rate: str_hit_rate,

            // 字节码缓存统计
            bytecode_cache_entries: bc_entries,
            bytecode_cache_lookups: bc_lookups,
            bytecode_cache_hits: bc_hits,
            bytecode_cache_hit_rate: bc_hit_rate,
            bytecode_cache_invalidations: bc_invals,
            bytecode_cache_evictions: bc_evicts,

            // 内联缓存统计
            inline_cache_count: self.inline_caches.len(),
            inline_cache_total_lookups: ic_total_lookups,
            inline_cache_total_hits: ic_total_hits,
            inline_cache_hit_rate: ic_hit_rate,
            inline_cache_state_transitions: ic_total_transitions,
            inline_cache_monomorphic_count: ic_mono_count,
            inline_cache_polymorphic_count: ic_poly_count,
            inline_cache_megamorphic_count: ic_mega_count,
            inline_cache_uninitialized_count: ic_uninit_count,
        }
    }

    /// 生成格式化的统计报告
    pub fn print_stats_report(&self) -> String {
        let stats = self.global_stats();

        format!(
            "╔══════════════════════════════════════════════════════════╗\n\
             ║             Nuzo Runtime Cache Statistics              ║\n\
             ╠══════════════════════════════════════════════════════════╣\n\
             ║ String Constant Pool:                                  ║\n\
             ║   Size: {:>6}  Lookups: {:>8}  Hits: {:>8}         ║\n\
             ║   Hit Rate: {:>6.2}%                                   ║\n\
             ╠══════════════════════════════════════════════════════════╣\n\
             ║ Bytecode Cache:                                        ║\n\
             ║   Entries: {:>5}  Lookups: {:>8}  Hits: {:>8}        ║\n\
             ║   Hit Rate: {:>6.2}%  Invalidations: {:>5}  Evictions: {:>4} ║\n\
             ╠══════════════════════════════════════════════════════════╣\n\
             ║ Inline Caches:                                         ║\n\
             ║   Total: {:>5}  Lookups: {:>8}  Hits: {:>8}          ║\n\
             ║   Hit Rate: {:>6.2}%  State Transitions: {:>6}         ║\n\
             ║   State Distribution:                                  ║\n\
             ║     - Monomorphic: {:>4}  Polymorphic: {:>4}           ║\n\
             ║     - Megamorphic: {:>3}  Uninitialized: {:>4}          ║\n\
             ╚══════════════════════════════════════════════════════════╝",
            stats.string_pool_size,
            stats.string_pool_lookups,
            stats.string_pool_hits,
            stats.string_pool_hit_rate * 100.0,
            stats.bytecode_cache_entries,
            stats.bytecode_cache_lookups,
            stats.bytecode_cache_hits,
            stats.bytecode_cache_hit_rate * 100.0,
            stats.bytecode_cache_invalidations,
            stats.bytecode_cache_evictions,
            stats.inline_cache_count,
            stats.inline_cache_total_lookups,
            stats.inline_cache_total_hits,
            stats.inline_cache_hit_rate * 100.0,
            stats.inline_cache_state_transitions,
            stats.inline_cache_monomorphic_count,
            stats.inline_cache_polymorphic_count,
            stats.inline_cache_megamorphic_count,
            stats.inline_cache_uninitialized_count,
        )
    }

    // ========================================================================
    // 配置方法
    // ========================================================================

    /// 获取字符串池最大容量
    pub fn string_pool_capacity(&self) -> usize {
        self.strings.max_capacity()
    }

    /// 设置字符串池最大容量
    pub fn string_pool_capacity_mut(&mut self, capacity: usize) {
        self.strings.max_capacity_mut(capacity);
    }

    /// 获取字节码缓存最大容量
    pub fn bytecode_cache_capacity(&self) -> usize {
        self.bytecode.max_capacity()
    }

    /// 设置字节码缓存最大容量
    pub fn bytecode_cache_capacity_mut(&mut self, capacity: usize) {
        self.bytecode.max_capacity_mut(capacity);
    }
}

impl Default for CacheManager {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CacheManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CacheManager")
            .field("strings", &self.strings)
            .field("bytecode", &self.bytecode)
            .field("inline_cache_count", &self.inline_caches.len())
            .finish()
    }
}

/// 全局缓存统计信息
#[derive(Debug, Clone)]
pub struct CacheGlobalStats {
    /// 当前字符串池大小
    pub string_pool_size: usize,
    /// 字符串池查找次数
    pub string_pool_lookups: usize,
    /// 字符串池命中次数
    pub string_pool_hits: usize,
    /// 字符串池命中率
    pub string_pool_hit_rate: f64,

    /// 当前缓存条目数
    pub bytecode_cache_entries: usize,
    /// 字节码缓存查找次数
    pub bytecode_cache_lookups: usize,
    /// 字节码缓存命中次数
    pub bytecode_cache_hits: usize,
    /// 字节码缓存命中率
    pub bytecode_cache_hit_rate: f64,
    /// 字节码缓存失效次数
    pub bytecode_cache_invalidations: usize,
    /// 字节码缓存淘汰次数
    pub bytecode_cache_evictions: usize,

    /// 内联缓存总数
    pub inline_cache_count: usize,
    /// 内联缓存总查找次数
    pub inline_cache_total_lookups: usize,
    /// 内联缓存总命中次数
    pub inline_cache_total_hits: usize,
    /// 内联缓存命中率
    pub inline_cache_hit_rate: f64,
    /// 内联缓存状态转换次数
    pub inline_cache_state_transitions: usize,
    /// 单态内联缓存数
    pub inline_cache_monomorphic_count: usize,
    /// 多态内联缓存数
    pub inline_cache_polymorphic_count: usize,
    /// 超态内联缓存数
    pub inline_cache_megamorphic_count: usize,
    /// 未初始化内联缓存数
    pub inline_cache_uninitialized_count: usize,
}

impl CacheGlobalStats {
    /// 检查缓存系统是否健康
    pub fn is_healthy(&self) -> bool {
        // 字符串池健康检查
        let str_healthy = self.string_pool_lookups == 0 || self.string_pool_hit_rate > 0.0;

        // 字节码缓存健康检查
        let bc_healthy = self.bytecode_cache_lookups == 0 || self.bytecode_cache_hit_rate > 0.0;

        // 内联缓存健康检查
        let ic_healthy = self.inline_cache_total_lookups == 0 || self.inline_cache_hit_rate > 0.0;

        str_healthy && bc_healthy && ic_healthy
    }

    /// 估算内存使用量（字节）
    pub fn estimated_memory_usage(&self) -> usize {
        // 字符串池：每条 ~20 字节 HashMap 开销 + ~32 字节字符串
        let str_mem = self.string_pool_size * (20 + 32);

        // 字节码缓存：每条 ~1KB
        let bc_mem = self.bytecode_cache_entries * 1024;

        // 内联缓存：每条 ~100 字节
        let ic_mem = self.inline_cache_count * 100;

        str_mem + bc_mem + ic_mem
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_bytecode::{Chunk, Opcode};
    use nuzo_core::Value;

    // =========================================================================
    // 测试辅助函数
    // =========================================================================

    /// 创建一个用于测试的简单 Chunk
    fn create_test_chunk(value: f64) -> Chunk {
        let mut chunk = Chunk::new();
        let const_idx = chunk.add_constant(Value::from_number(value));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_byte(0);
        chunk.write_u16(const_idx as u16);
        chunk.write_opcode(Opcode::Print);
        chunk.write_byte(0);
        chunk.write_opcode(Opcode::Halt);
        chunk
    }

    // =========================================================================
    // StringConstantPool 测试
    // =========================================================================

    #[test]
    fn test_string_pool_creation() {
        let pool = StringConstantPool::new();
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.max_capacity(), DEFAULT_STRING_POOL_CAPACITY);
    }

    #[test]
    fn test_string_pool_with_custom_capacity() {
        let pool = StringConstantPool::with_capacity(100);
        assert_eq!(pool.max_capacity(), 100);
    }

    #[test]
    fn test_string_pool_intern_basic() {
        let mut pool = StringConstantPool::new();

        let idx = pool.intern("hello").unwrap();
        assert_eq!(pool.len(), 1);

        assert!(pool.contains("hello").is_some());
        assert_eq!(pool.contains("hello").unwrap(), idx);
    }

    #[test]
    fn test_string_pool_intern_deduplication() {
        let mut pool = StringConstantPool::new();

        let idx1 = pool.intern("hello").unwrap();
        let idx2 = pool.intern("hello").unwrap();

        assert_eq!(idx1, idx2);

        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_string_pool_different_strings() {
        let mut pool = StringConstantPool::new();

        let idx1 = pool.intern("hello").unwrap();
        let idx2 = pool.intern("world").unwrap();

        assert!(idx1 != idx2, "Different strings should have different references");

        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn test_string_pool_contains() {
        let mut pool = StringConstantPool::new();

        pool.intern("test").unwrap();

        assert!(pool.contains("test").is_some());

        assert!(pool.contains("nonexistent").is_none());
    }

    #[test]
    fn test_string_pool_statistics() {
        let mut pool = StringConstantPool::new();

        let (size, lookups, hits, rate) = pool.stats();
        assert_eq!(size, 0);
        assert_eq!(lookups, 0);
        assert_eq!(hits, 0);
        assert_eq!(rate, 0.0);

        pool.intern("hello").unwrap();
        let (_, lookups1, hits1, _) = pool.stats();
        assert_eq!(lookups1, 1);
        assert_eq!(hits1, 0);

        pool.intern("hello").unwrap();
        let (_, lookups2, hits2, rate2) = pool.stats();
        assert_eq!(lookups2, 2);
        assert_eq!(hits2, 1);
        assert!(rate2 > 0.0);
    }

    #[test]
    fn test_string_pool_capacity_limit() {
        let mut pool = StringConstantPool::with_capacity(2);

        pool.intern("a").unwrap();
        pool.intern("b").unwrap();
        assert_eq!(pool.len(), 2);

        let result = pool.intern("c");
        assert!(result.is_err());
        match result.unwrap_err() {
            CacheError::StringPoolFull { current_size, max_capacity } => {
                assert_eq!(current_size, 2);
                assert_eq!(max_capacity, 2);
            }
            other => panic!("Expected StringPoolFull error, got {:?}", other),
        }
    }

    #[test]
    fn test_string_pool_clear() {
        let mut pool = StringConstantPool::new();

        pool.intern("test").unwrap();
        assert_eq!(pool.len(), 1);

        pool.clear();
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        let (_, lookups, hits, _) = pool.stats();
        assert_eq!(lookups, 0);
        assert_eq!(hits, 0);
    }

    #[test]
    fn test_string_pool_empty_string() {
        let mut pool = StringConstantPool::new();

        let idx = pool.intern("").unwrap();
        assert!(pool.contains("").is_some());
        assert_eq!(pool.contains("").unwrap(), idx);
    }

    #[test]
    fn test_string_pool_unicode_strings() {
        let mut pool = StringConstantPool::new();

        let unicode_strs =
            vec!["hello world", "test string", "unicode test", "another test", "final test"];

        for s in &unicode_strs {
            pool.intern(s).unwrap();
        }

        assert_eq!(pool.len(), unicode_strs.len());

        for s in &unicode_strs {
            let idx1 = pool.intern(s).unwrap();
            let idx2 = pool.intern(s).unwrap();
            assert_eq!(idx1, idx2);
        }
    }

    #[test]
    fn test_string_pool_many_strings() {
        let mut pool = StringConstantPool::with_capacity(10000);

        for i in 0..1000 {
            pool.intern(&format!("string_{}", i)).unwrap();
        }

        assert_eq!(pool.len(), 1000);

        for i in 0..100 {
            let idx1 = pool.intern(&format!("string_{}", i)).unwrap();
            let idx2 = pool.intern(&format!("string_{}", i)).unwrap();
            assert_eq!(idx1, idx2);
        }
    }

    // =========================================================================
    // InlineCache 测试
    // =========================================================================

    #[test]
    fn test_ic_initial_state() {
        let ic = InlineCache::new();
        assert!(ic.state().is_uninitialized());
        assert_eq!(ic.state().entry_count(), 0);
    }

    #[test]
    fn test_ic_first_lookup_creates_monomorphic() {
        let mut ic = InlineCache::new();

        // 第一次查找应该从未初始化转为单态
        let result = ic.lookup_or_update(1, |_shape_id| Some(0));
        assert!(result.is_some());
        assert_eq!(result.unwrap(), 0);

        // 现在应该是单态
        assert!(ic.state().is_monomorphic());

        if let ICState::Monomorphic { shape_id, offset } = ic.state() {
            assert_eq!(*shape_id, 1);
            assert_eq!(*offset, 0);
        } else {
            panic!("Expected Monomorphic state");
        }
    }

    #[test]
    fn test_ic_monomorphic_hit() {
        let mut ic = InlineCache::new();

        // 初始化为单态
        ic.lookup_or_update(1, |_| Some(0)).unwrap();

        // 相同 shape 的查找应该命中
        let result = ic.lookup_or_update(1, |_| {
            panic!("Lookup function should not be called on cache hit");
        });

        assert_eq!(result.unwrap(), 0);

        // 验证统计
        let (_, lookups, hits, rate, _) = ic.stats();
        assert_eq!(lookups, 2);
        assert_eq!(hits, 1); // 第二次查找命中
        assert!(rate > 0.0);
    }

    #[test]
    fn test_ic_second_shape_creates_polymorphic() {
        let mut ic = InlineCache::new();

        // 第一个 shape
        ic.lookup_or_update(1, |_| Some(0)).unwrap();
        assert!(ic.state().is_monomorphic());

        // 第二个不同的 shape
        let result = ic.lookup_or_update(2, |_| Some(1));
        assert_eq!(result.unwrap(), 1);

        // 现在应该是多态
        assert!(ic.state().is_polymorphic());

        if let ICState::Polymorphic { entries } = ic.state() {
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0], (1, 0));
            assert_eq!(entries[1], (2, 1));
        } else {
            panic!("Expected Polymorphic state");
        }
    }

    #[test]
    fn test_ic_polymorphic_lookup() {
        let mut ic = InlineCache::new();

        // 添加两个 shape
        ic.lookup_or_update(1, |_| Some(0)).unwrap();
        ic.lookup_or_update(2, |_| Some(1)).unwrap();

        // 命中已有 shape
        let r1 = ic.lookup_or_update(1, |_| panic!("Should not call"));
        assert_eq!(r1.unwrap(), 0);

        let r2 = ic.lookup_or_update(2, |_| panic!("Should not call"));
        assert_eq!(r2.unwrap(), 1);

        // 未命中 - 新 shape
        let r3 = ic.lookup_or_update(3, |_| Some(2));
        assert_eq!(r3.unwrap(), 2);
    }

    #[test]
    fn test_ic_too_many_shapes_becomes_megamorphic() {
        let mut ic = InlineCache::new();

        // 添加超过 POLYMORPHIC_MAX_ENTRIES (4) 个 shape
        for i in 0..=POLYMORPHIC_MAX_ENTRIES {
            ic.lookup_or_update(i, Some).unwrap();
        }

        // 第 5 个 shape 应该触发转换为超态
        assert!(ic.state().is_megamorphic());

        if let ICState::Megamorphic { entries } = ic.state() {
            assert_eq!(entries.len(), POLYMORPHIC_MAX_ENTRIES + 1);
        } else {
            panic!("Expected Megamorphic state");
        }
    }

    #[test]
    fn test_ic_megamorphic_lookup() {
        let mut ic = InlineCache::new();

        // 添加多个 shape 直到变成超态
        for i in 0..=POLYMORPHIC_MAX_ENTRIES {
            ic.lookup_or_update(i, Some).unwrap();
        }

        // 超态下查找应该正常工作
        let result = ic.lookup_or_update(0, |_| panic!("Should not call"));
        assert_eq!(result.unwrap(), 0);

        let result = ic.lookup_or_update(POLYMORPHIC_MAX_ENTRIES, |_| panic!("Should not call"));
        assert_eq!(result.unwrap(), POLYMORPHIC_MAX_ENTRIES);
    }

    #[test]
    fn test_ic_lookup_returns_none_when_not_found() {
        let mut ic = InlineCache::new();

        // 查找函数返回 None
        let result = ic.lookup_or_update(1, |_| None);
        assert!(result.is_none());

        // 状态应该保持未初始化
        assert!(ic.state().is_uninitialized());
    }

    #[test]
    fn test_ic_invalidate() {
        let mut ic = InlineCache::new();

        // 建立一些缓存
        ic.lookup_or_update(1, |_| Some(0)).unwrap();
        ic.lookup_or_update(2, |_| Some(1)).unwrap();
        assert!(!ic.state().is_uninitialized());

        // 使缓存失效
        ic.invalidate();
        assert!(ic.state().is_uninitialized());

        // 下次查找应该重新走完整流程
        let result = ic.lookup_or_update(1, |_| Some(5));
        assert_eq!(result.unwrap(), 5);
    }

    #[test]
    fn test_ic_statistics() {
        let mut ic = InlineCache::new();

        // 初始统计
        let (state, lookups, hits, rate, transitions) = ic.stats();
        assert_eq!(state, "Uninitialized");
        assert_eq!(lookups, 0);
        assert_eq!(hits, 0);
        assert_eq!(rate, 0.0);
        assert_eq!(transitions, 0);

        // 一些操作
        ic.lookup_or_update(1, |_| Some(0)).unwrap(); // 未命中
        ic.lookup_or_update(1, |_| panic!()).unwrap(); // 命中
        ic.lookup_or_update(2, |_| Some(1)).unwrap(); // 未命中 + 状态转换
        let (_, l, h, r, t) = ic.stats();
        assert_eq!(l, 3);
        assert_eq!(h, 1);
        assert!(r > 0.0);
        assert!(t > 0); // 至少有一次状态转换
    }

    #[test]
    fn test_ic_reset_stats() {
        let mut ic = InlineCache::new();

        // 生成一些统计
        ic.lookup_or_update(1, |_| Some(0)).unwrap();
        ic.lookup_or_update(1, |_| panic!()).unwrap();

        // 重置统计
        ic.reset_stats();

        let (_, lookups, hits, _, transitions) = ic.stats();
        assert_eq!(lookups, 0);
        assert_eq!(hits, 0);
        assert_eq!(transitions, 0);

        // 但缓存数据应该还在
        assert!(ic.state().is_monomorphic());
    }

    #[test]
    fn test_ic_display_and_debug() {
        let ic = InlineCache::new();

        // 测试 Display trait
        let display = format!("{}", ic);
        assert!(display.contains("Uninitialized"));

        // 测试 Debug trait
        let debug = format!("{:?}", ic);
        assert!(debug.contains("InlineCache"));
        assert!(debug.contains("Uninitialized"));
    }

    // =========================================================================
    // BytecodeCache 测试
    // =========================================================================

    #[test]
    fn test_bytecode_cache_creation() {
        let cache = BytecodeCache::new();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
        assert_eq!(cache.max_capacity(), 256);
    }

    #[test]
    fn test_bytecode_cache_custom_capacity() {
        let cache = BytecodeCache::with_capacity(100);
        assert_eq!(cache.max_capacity(), 100);
    }

    #[test]
    fn test_bytecode_cache_basic_operations() {
        let mut cache = BytecodeCache::new();

        let chunk = create_test_chunk(42.0);
        let hash = SourceHash::compute_str("print(42)");

        // 缓存 chunk
        cache.cache(&hash, chunk).unwrap();
        assert_eq!(cache.len(), 1);
        assert!(cache.contains(&hash));

        // 查找
        let retrieved = cache.lookup(&hash);
        assert!(retrieved.is_some());

        // 验证 chunk 内容
        let retrieved_chunk = retrieved.unwrap();
        assert_eq!(retrieved_chunk.constants().len(), 1);
        assert_eq!(retrieved_chunk.constants()[0], Value::from_number(42.0));
    }

    #[test]
    fn test_bytecode_cache_miss() {
        let mut cache = BytecodeCache::new();

        let hash = SourceHash::compute_str("nonexistent");

        // 查找不存在的条目
        let result = cache.lookup(&hash);
        assert!(result.is_none());
    }

    #[test]
    fn test_bytecode_cache_hash_collisions_handled() {
        let mut cache = BytecodeCache::new();

        // 相同哈希的后续插入覆盖前一个
        let hash = SourceHash::compute(b"test");

        let chunk1 = create_test_chunk(1.0);
        let chunk2 = create_test_chunk(2.0);

        cache.cache(&hash, chunk1).unwrap();
        cache.cache(&hash, chunk2).unwrap(); // 覆盖

        // 应该得到最后一个插入的
        let retrieved = cache.lookup(&hash).unwrap();
        assert_eq!(retrieved.constants()[0], Value::from_number(2.0));

        // 但大小仍然 1
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_bytecode_cache_invalidate() {
        let mut cache = BytecodeCache::new();

        let hash = SourceHash::compute_str("test code");
        cache.cache(&hash, create_test_chunk(1.0)).unwrap();

        // 失效
        let removed = cache.invalidate(&hash).unwrap();
        assert!(removed);
        assert_eq!(cache.len(), 0);
        assert!(!cache.contains(&hash));

        // 再次失效应该返回 false
        let removed_again = cache.invalidate(&hash).unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn test_bytecode_cache_invalidate_batch() {
        let mut cache = BytecodeCache::new();

        // 添加多个条目
        let hashes: Vec<SourceHash> =
            (0..5).map(|i| SourceHash::compute_str(&format!("code {}", i))).collect();

        for (idx, &hash) in hashes.iter().enumerate() {
            cache.cache(&hash, create_test_chunk(idx as f64)).unwrap();
        }
        assert_eq!(cache.len(), 5);

        // 批量删除前 3 个
        let removed = cache.invalidate_batch(&hashes[..3]);
        assert_eq!(removed, 3);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_bytecode_cache_clear() {
        let mut cache = BytecodeCache::new();

        for i in 0..10 {
            let hash = SourceHash::compute_str(&format!("code {}", i));
            cache.cache(&hash, create_test_chunk(i as f64)).unwrap();
        }
        assert_eq!(cache.len(), 10);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_bytecode_cache_capacity_limit() {
        let mut cache = BytecodeCache::with_capacity(2);

        // 添加 2 个
        cache.cache(&SourceHash::compute_str("a"), create_test_chunk(1.0)).unwrap();
        cache.cache(&SourceHash::compute_str("b"), create_test_chunk(2.0)).unwrap();
        assert_eq!(cache.len(), 2);

        // 第 3 个应该失败
        let result = cache.cache(&SourceHash::compute_str("c"), create_test_chunk(3.0));
        assert!(result.is_err());
        match result.unwrap_err() {
            CacheError::GenericError(msg) => {
                assert!(msg.contains("full"));
            }
            other => panic!("Expected GenericError, got {:?}", other),
        }
    }

    #[test]
    fn test_bytecode_cache_statistics() {
        let mut cache = BytecodeCache::new();

        // 初始统计
        let (entries, lookups, hits, rate, invals, evicts) = cache.stats();
        assert_eq!(entries, 0);
        assert_eq!(lookups, 0);
        assert_eq!(hits, 0);
        assert_eq!(rate, 0.0);
        assert_eq!(invals, 0);
        assert_eq!(evicts, 0);

        // 缓存和查找
        let hash = SourceHash::compute_str("test");
        cache.cache(&hash, create_test_chunk(42.0)).unwrap();
        cache.lookup(&hash); // 命中
        cache.lookup(&SourceHash::compute_str("miss")); // 未命中
        let (e, l, h, r, i, e_) = cache.stats();
        assert_eq!(e, 1);
        assert_eq!(l, 2);
        assert_eq!(h, 1);
        assert!(r > 0.0);
        assert_eq!(i, 0);
        assert_eq!(e_, 0);
    }

    #[test]
    fn test_source_hash_computation() {
        // 相同输入产生相同哈希
        let hash1 = SourceHash::compute_str("hello world");
        let hash2 = SourceHash::compute_str("hello world");
        assert_eq!(hash1, hash2);

        // 不同输入产生不同哈希
        let hash3 = SourceHash::compute_str("goodbye world");
        assert_ne!(hash1, hash3);

        // 空字符串也有有效哈希
        let empty_hash = SourceHash::compute_str("");
        assert_ne!(empty_hash.value(), 0);
    }

    #[test]
    fn test_source_hash_display() {
        let hash = SourceHash::compute(b"test");
        let display = format!("{}", hash);

        // 应该是 16 位十六进制数
        assert_eq!(display.len(), 16);
        assert!(display.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_bytecode_cache_top_accessed() {
        let mut cache = BytecodeCache::new();

        // 添加多个条目并模拟不同访问频率
        let hashes: Vec<SourceHash> =
            (0..5).map(|i| SourceHash::compute_str(&format!("code {}", i))).collect();

        for (i, &hash) in hashes.iter().enumerate() {
            cache.cache(&hash, create_test_chunk(i as f64)).unwrap();
        }

        // 模拟不同访问频率
        for _ in 0..10 {
            cache.lookup(&hashes[0]); // 最热门
        }
        for _ in 0..5 {
            cache.lookup(&hashes[1]); // 第二热门
        }
        for _ in 0..2 {
            cache.lookup(&hashes[2]);
        }
        cache.lookup(&hashes[3]); // 1 次
        // hashes[4] 从未被访问

        let top = cache.top_accessed(3);
        assert_eq!(top.len(), 3);
        assert_eq!(top[0].0, hashes[0]); // 最热门
        assert_eq!(top[0].1, 10); // 访问 10 次
        assert_eq!(top[1].0, hashes[1]); // 第二热门
        assert_eq!(top[1].1, 5);
    }

    // =========================================================================
    // CacheManager 测试
    // =========================================================================

    #[test]
    fn test_cache_manager_creation() {
        let manager = CacheManager::new();
        assert_eq!(manager.strings().len(), 0);
        assert_eq!(manager.bytecode().len(), 0);
        assert_eq!(manager.inline_cache_count(), 0);
    }

    #[test]
    fn test_cache_manager_with_config() {
        let manager = CacheManager::with_config(1000, 200);
        assert_eq!(manager.string_pool_capacity(), 1000);
        assert_eq!(manager.bytecode_cache_capacity(), 200);
    }

    #[test]
    fn test_cache_manager_string_pool_integration() {
        let mut manager = CacheManager::new();

        let idx1 = manager.strings_mut().intern("shared").unwrap();
        let idx2 = manager.strings_mut().intern("shared").unwrap();

        assert_eq!(idx1, idx2);
        assert_eq!(manager.strings().len(), 1);
    }

    #[test]
    fn test_cache_manager_bytecode_integration() {
        let mut manager = CacheManager::new();

        let hash = SourceHash::compute_str("print(99)");
        manager.bytecode_mut().cache(&hash, create_test_chunk(99.0)).unwrap();

        assert!(manager.bytecode().contains(&hash));
        let retrieved = manager.bytecode_mut().lookup(&hash).unwrap();
        assert_eq!(retrieved.constants()[0], Value::from_number(99.0));
    }

    #[test]
    fn test_cache_manager_inline_cache_management() {
        let mut manager = CacheManager::new();

        // 获取属性 "x" 的内联缓存
        let ic = manager.get_inline_cache("x");
        assert!(ic.state().is_uninitialized());
        assert_eq!(manager.inline_cache_count(), 1);

        // 获取相同属性应该返回同一个缓存（数量不变）
        let _ic2 = manager.get_inline_cache("x");
        assert_eq!(manager.inline_cache_count(), 1);

        // 不同属性有不同缓存
        let _ic_y = manager.get_inline_cache("y");
        assert_eq!(manager.inline_cache_count(), 2);
    }

    #[test]
    fn test_cache_manager_invalidate_inline_cache() {
        let mut manager = CacheManager::new();

        // 创建并使用一个内联缓存
        {
            let ic = manager.get_inline_cache("prop");
            ic.lookup_or_update(1, |_| Some(0)).unwrap();
            assert!(ic.state().is_monomorphic());
        } // ic 的借用在这里结束

        // 使其失效
        let result = manager.invalidate_inline_cache("prop");
        assert!(result);

        // 验证状态
        let ic = manager.get_inline_cache("prop");
        assert!(ic.state().is_uninitialized());
    }

    #[test]
    fn test_cache_manager_clear_all() {
        let mut manager = CacheManager::new();

        manager.strings_mut().intern("test").unwrap();
        manager
            .bytecode_mut()
            .cache(&SourceHash::compute_str("code"), create_test_chunk(1.0))
            .unwrap();
        manager.get_inline_cache("x").lookup_or_update(1, |_| Some(0)).unwrap();

        manager.clear_all();

        assert_eq!(manager.strings().len(), 0);
        assert_eq!(manager.bytecode().len(), 0);
        let ic = manager.get_inline_cache("x");
        assert!(ic.state().is_uninitialized());
    }

    #[test]
    fn test_cache_manager_global_stats() {
        let mut manager = CacheManager::new();

        manager.strings_mut().intern("hello").unwrap();
        manager.strings_mut().intern("hello").unwrap();

        let hash = SourceHash::compute_str("code");
        manager.bytecode_mut().cache(&hash, create_test_chunk(42.0)).unwrap();
        manager.bytecode_mut().lookup(&hash); // 命中

        manager.get_inline_cache("x").lookup_or_update(1, |_| Some(0)).unwrap();
        manager.get_inline_cache("x").lookup_or_update(1, |_| panic!()); // 命中

        // 获取全局统计
        let stats = manager.global_stats();

        // 验证统计信息
        assert_eq!(stats.string_pool_size, 1);
        assert_eq!(stats.string_pool_lookups, 2);
        assert_eq!(stats.string_pool_hits, 1);
        assert!(stats.string_pool_hit_rate > 0.0);

        assert_eq!(stats.bytecode_cache_entries, 1);
        assert_eq!(stats.bytecode_cache_lookups, 1);
        assert_eq!(stats.bytecode_cache_hits, 1);
        assert!(stats.bytecode_cache_hit_rate > 0.0);

        assert_eq!(stats.inline_cache_count, 1);
        assert_eq!(stats.inline_cache_total_lookups, 2);
        assert_eq!(stats.inline_cache_total_hits, 1);
        assert!(stats.inline_cache_hit_rate > 0.0);
    }

    #[test]
    fn test_cache_manager_print_report() {
        let manager = CacheManager::new();
        let report = manager.print_stats_report();

        // 验证报告格式
        assert!(report.contains("Nuzo Runtime Cache Statistics"));
        assert!(report.contains("String Constant Pool"));
        assert!(report.contains("Bytecode Cache"));
        assert!(report.contains("Inline Caches"));
    }

    #[test]
    fn test_cache_manager_configuration() {
        let mut manager = CacheManager::new();

        // 默认配置
        assert_eq!(manager.string_pool_capacity(), DEFAULT_STRING_POOL_CAPACITY);
        assert_eq!(manager.bytecode_cache_capacity(), 256);

        // 修改配置
        manager.string_pool_capacity_mut(5000);
        manager.bytecode_cache_capacity_mut(100);

        assert_eq!(manager.string_pool_capacity(), 5000);
        assert_eq!(manager.bytecode_cache_capacity(), 100);
    }

    #[test]
    fn test_cache_global_stats_health_check() {
        let stats = CacheGlobalStats {
            string_pool_size: 10,
            string_pool_lookups: 100,
            string_pool_hits: 80,
            string_pool_hit_rate: 0.8,
            bytecode_cache_entries: 5,
            bytecode_cache_lookups: 50,
            bytecode_cache_hits: 40,
            bytecode_cache_hit_rate: 0.8,
            bytecode_cache_invalidations: 2,
            bytecode_cache_evictions: 0,
            inline_cache_count: 3,
            inline_cache_total_lookups: 30,
            inline_cache_total_hits: 25,
            inline_cache_hit_rate: 0.833,
            inline_cache_state_transitions: 5,
            inline_cache_monomorphic_count: 2,
            inline_cache_polymorphic_count: 1,
            inline_cache_megamorphic_count: 0,
            inline_cache_uninitialized_count: 0,
        };

        assert!(stats.is_healthy());

        // 不健康的情况：所有命中率为 0
        let unhealthy_stats = CacheGlobalStats {
            string_pool_hits: 0,
            string_pool_lookups: 100,
            string_pool_hit_rate: 0.0,
            bytecode_cache_hits: 0,
            bytecode_cache_lookups: 100,
            bytecode_cache_hit_rate: 0.0,
            inline_cache_total_hits: 0,
            inline_cache_total_lookups: 100,
            inline_cache_hit_rate: 0.0,
            ..stats
        };

        assert!(!unhealthy_stats.is_healthy());
    }

    // =========================================================================
    // 集成测试
    // =========================================================================

    #[test]
    fn test_full_workflow_simulation() {
        let mut manager = CacheManager::with_config(1000, 100);

        let source_codes = vec![("print(42)", 42.0), ("print(\"hello\")", 0.0), ("1 + 2", 3.0)];

        let mut hashes = Vec::new();
        for (code, _) in &source_codes {
            for part in code.split_whitespace() {
                manager.strings_mut().intern(part).unwrap();
                manager.strings_mut().intern(part).unwrap();
            }

            let chunk = create_test_chunk(if code.starts_with("print(42)") { 42.0 } else { 0.0 });
            let hash = SourceHash::compute_str(code);
            manager.bytecode_mut().cache(&hash, chunk).unwrap();
            hashes.push(hash);
        }

        for hash in &hashes {
            let ic = manager.get_inline_cache("test_prop");
            let offset = ic
                .lookup_or_update(0, |shape_id| if shape_id == 0 { Some(0) } else { None })
                .unwrap();

            let cached_chunk = manager.bytecode_mut().lookup(hash).unwrap();

            let value = cached_chunk.constants()[offset];
            assert!(value.is_number());
        }

        let stats = manager.global_stats();
        assert!(stats.string_pool_size > 0);
        assert_eq!(stats.bytecode_cache_entries, source_codes.len());
        assert!(stats.inline_cache_count > 0);

        manager.clear_all();
        assert_eq!(manager.strings().len(), 0);
        assert_eq!(manager.bytecode().len(), 0);
    }

    #[test]
    fn test_stress_test_multiple_caches_interaction() {
        let mut manager = CacheManager::with_config(5000, 500);

        for i in 0..100 {
            let s = format!("string_{}", i % 20);
            manager.strings_mut().intern(&s).unwrap();

            if i % 10 == 0 {
                let code = format!("code_{}", i / 10);
                let hash = SourceHash::compute_str(&code);
                manager.bytecode_mut().cache(&hash, create_test_chunk(i as f64)).unwrap();
            }

            let prop_name = format!("prop_{}", i % 5);
            let ic = manager.get_inline_cache(&prop_name);
            ic.lookup_or_update(i % 3, |shape_id| Some(shape_id * 10)).unwrap();
        }

        let stats = manager.global_stats();
        assert!(stats.string_pool_size <= 20);
        assert!(stats.bytecode_cache_entries <= 10);
        assert!(stats.inline_cache_count <= 5);
        assert!(stats.is_healthy());
    }

    #[test]
    fn test_error_type_display() {
        let err_full = CacheError::StringPoolFull { current_size: 100, max_capacity: 50 };
        let display = format!("{}", err_full);
        assert!(display.contains("full"));

        let err_transition =
            CacheError::InvalidICStateTransition { from: "Monomorphic", to: "Invalid" };
        let display2 = format!("{}", err_transition);
        assert!(display2.contains("Invalid IC state transition"));

        let err_generic = CacheError::GenericError("test error".to_string());
        let display3 = format!("{}", err_generic);
        assert!(display3.contains("test error"));
    }

    #[test]
    fn test_ic_state_entry_count() {
        assert_eq!(ICState::Uninitialized.entry_count(), 0);
        assert_eq!(ICState::Monomorphic { shape_id: 1, offset: 0 }.entry_count(), 1);
        assert_eq!(ICState::Polymorphic { entries: vec![(1, 0), (2, 1)] }.entry_count(), 2);
        let mut map = xx_hash_map_new();
        map.insert(1, 0);
        map.insert(2, 1);
        map.insert(3, 2);
        assert_eq!(ICState::Megamorphic { entries: map }.entry_count(), 3);
    }

    // =========================================================================
    // StringConstantPool 容量管理 API 测试
    // =========================================================================

    #[test]
    fn test_string_pool_max_capacity_default() {
        let pool = StringConstantPool::new();
        assert_eq!(pool.max_capacity(), DEFAULT_STRING_POOL_CAPACITY);
    }

    #[test]
    fn test_string_pool_max_capacity_mut_sets_value() {
        let mut pool = StringConstantPool::new();
        pool.max_capacity_mut(500);
        assert_eq!(pool.max_capacity(), 500);
    }

    #[test]
    fn test_string_pool_max_capacity_mut_zero_falls_back_to_default() {
        let mut pool = StringConstantPool::new();
        pool.max_capacity_mut(0);
        assert_eq!(pool.max_capacity(), DEFAULT_STRING_POOL_CAPACITY);
    }

    #[test]
    fn test_string_pool_with_max_capacity_builder() {
        let mut pool = StringConstantPool::new();
        pool.with_max_capacity(256);
        assert_eq!(pool.max_capacity(), 256);
    }

    // =========================================================================
    // ICState 状态判断 API 测试
    // =========================================================================

    #[test]
    fn test_ic_state_is_monomorphic() {
        assert!(ICState::Monomorphic { shape_id: 1, offset: 0 }.is_monomorphic());
        assert!(!ICState::Uninitialized.is_monomorphic());
    }

    #[test]
    fn test_ic_state_is_polymorphic() {
        let poly = ICState::Polymorphic { entries: vec![(1, 0), (2, 1)] };
        assert!(poly.is_polymorphic());
        assert!(!ICState::Uninitialized.is_polymorphic());
    }

    #[test]
    fn test_ic_state_is_megamorphic() {
        let mut map = xx_hash_map_new();
        map.insert(1, 0);
        let mega = ICState::Megamorphic { entries: map };
        assert!(mega.is_megamorphic());
        assert!(!ICState::Uninitialized.is_megamorphic());
    }

    // =========================================================================
    // InlineCache API 测试
    // =========================================================================

    #[test]
    fn test_inline_cache_lookup_or_update_miss_then_hit() {
        let mut ic = InlineCache::new();
        // 首次查找：缓存未命中，lookup_fn 返回 Some → 应更新缓存
        let result = ic.lookup_or_update(1, |_| Some(0));
        assert_eq!(result, Some(0));

        // 第二次查找相同 shape：缓存命中
        let result2 = ic.lookup_or_update(1, |_| panic!("should not call on hit"));
        assert_eq!(result2, Some(0));
    }

    #[test]
    fn test_inline_cache_lookup_or_update_not_found() {
        let mut ic = InlineCache::new();
        // lookup_fn 返回 None → 缓存不更新
        let result = ic.lookup_or_update(99, |_| None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_inline_cache_state_mut_allows_direct_modification() {
        let mut ic = InlineCache::new();
        *ic.state_mut() = ICState::Monomorphic { shape_id: 5, offset: 10 };
        assert!(ic.state().is_monomorphic());
    }

    // =========================================================================
    // SourceHash API 测试
    // =========================================================================

    #[test]
    fn test_source_hash_compute_str_deterministic() {
        let h1 = SourceHash::compute_str("hello world");
        let h2 = SourceHash::compute_str("hello world");
        assert_eq!(h1.value(), h2.value());
    }

    #[test]
    fn test_source_hash_compute_str_differs_for_different_input() {
        let h1 = SourceHash::compute_str("hello");
        let h2 = SourceHash::compute_str("world");
        assert_ne!(h1.value(), h2.value());
    }

    #[test]
    fn test_source_hash_compute_str_matches_compute_bytes() {
        let s = "test string";
        let h_str = SourceHash::compute_str(s);
        let h_bytes = SourceHash::compute(s.as_bytes());
        assert_eq!(h_str.value(), h_bytes.value());
    }

    #[test]
    fn test_source_hash_stability() {
        // SourceHash is used as the bytecode cache key. Its value must stay stable
        // across runs and crate versions so cached bytecode files don't get
        // invalidated unnecessarily. These test vectors pin the exact xxHash3
        // output for representative inputs.

        // Vector 1: empty string.
        // xxh3_64(b"") is the canonical empty-input hash (also asserted in nuzo_core).
        let empty = SourceHash::compute_str("");
        assert_eq!(empty.value(), 0x2d06800538d394c2);

        // Vector 2: short identifier.
        let print_id = SourceHash::compute_str("print");
        assert_eq!(print_id, SourceHash::compute(b"print"));
        // Re-compute to verify determinism.
        assert_eq!(print_id.value(), SourceHash::compute_str("print").value());
        // Different short identifiers must collide-check (basic distribution).
        assert_ne!(print_id.value(), SourceHash::compute_str("println").value());

        // Vector 3: longer source snippet.
        let source = "fn factorial(n) { if n <= 1 { return 1; } return n * factorial(n - 1); }";
        let h1 = SourceHash::compute_str(source);
        let h2 = SourceHash::compute(source.as_bytes());
        assert_eq!(h1, h2);
        assert_eq!(h1.value(), SourceHash::compute_str(source).value());

        // All three vectors must be mutually distinct (collision sanity check).
        assert_ne!(empty.value(), print_id.value());
        assert_ne!(empty.value(), h1.value());
        assert_ne!(print_id.value(), h1.value());

        // Display form must remain 16 hex digits for all vectors (cache file naming).
        for h in [empty, print_id, h1] {
            assert_eq!(format!("{}", h).len(), 16);
        }
    }

    // =========================================================================
    // BytecodeCache API 测试
    // =========================================================================

    #[test]
    fn test_bytecode_cache_max_capacity_default() {
        let cache = BytecodeCache::new();
        assert_eq!(cache.max_capacity(), BYTECODE_CACHE_DEFAULT_CAPACITY);
    }

    #[test]
    fn test_bytecode_cache_max_capacity_mut_sets_value() {
        let mut cache = BytecodeCache::new();
        cache.max_capacity_mut(100);
        assert_eq!(cache.max_capacity(), 100);
    }

    #[test]
    fn test_bytecode_cache_max_capacity_mut_zero_falls_back() {
        let mut cache = BytecodeCache::new();
        cache.max_capacity_mut(0);
        assert_eq!(cache.max_capacity(), BYTECODE_CACHE_DEFAULT_CAPACITY);
    }

    #[test]
    fn test_bytecode_cache_with_max_capacity_builder() {
        let mut cache = BytecodeCache::new();
        cache.with_max_capacity(200);
        assert_eq!(cache.max_capacity(), 200);
    }

    #[test]
    fn test_bytecode_cache_lookup_mut_returns_entry() {
        let mut cache = BytecodeCache::new();
        let hash = SourceHash::compute_str("test source");
        let chunk = create_test_chunk(42.0);
        cache.cache(&hash, chunk).unwrap();

        // lookup_mut 应返回可变引用
        let found = cache.lookup_mut(&hash);
        assert!(found.is_some());
    }

    #[test]
    fn test_bytecode_cache_lookup_mut_miss() {
        let mut cache = BytecodeCache::new();
        let hash = SourceHash::compute_str("nonexistent");
        let found = cache.lookup_mut(&hash);
        assert!(found.is_none());
    }

    // =========================================================================
    // CacheManager API 测试
    // =========================================================================

    #[test]
    fn test_cache_manager_strings_mut_allows_intern() {
        let mut mgr = CacheManager::new();
        let idx = mgr.strings_mut().intern("test").unwrap();
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_cache_manager_bytecode_mut_allows_cache() {
        let mut mgr = CacheManager::new();
        let hash = SourceHash::compute_str("source");
        let chunk = create_test_chunk(1.0);
        mgr.bytecode_mut().cache(&hash, chunk).unwrap();
        assert!(mgr.bytecode().contains(&hash));
    }

    #[test]
    fn test_cache_manager_get_inline_cache_creates_new() {
        let mut mgr = CacheManager::new();
        assert_eq!(mgr.inline_cache_count(), 0);
        let ic = mgr.get_inline_cache("prop_x");
        assert!(ic.state().is_uninitialized());
        assert_eq!(mgr.inline_cache_count(), 1);
    }

    #[test]
    fn test_cache_manager_get_inline_cache_returns_existing() {
        let mut mgr = CacheManager::new();
        mgr.get_inline_cache("prop_y");
        mgr.get_inline_cache("prop_y");
        assert_eq!(mgr.inline_cache_count(), 1);
    }

    #[test]
    fn test_cache_manager_inline_cache_count_tracks_entries() {
        let mut mgr = CacheManager::new();
        mgr.get_inline_cache("a");
        mgr.get_inline_cache("b");
        mgr.get_inline_cache("c");
        assert_eq!(mgr.inline_cache_count(), 3);
    }

    #[test]
    fn test_cache_manager_invalidate_all_inline_caches() {
        let mut mgr = CacheManager::new();
        let ic = mgr.get_inline_cache("prop");
        *ic.state_mut() = ICState::Monomorphic { shape_id: 1, offset: 0 };
        assert!(mgr.get_inline_cache("prop").state().is_monomorphic());

        mgr.invalidate_all_inline_caches();
        assert!(mgr.get_inline_cache("prop").state().is_uninitialized());
    }

    #[test]
    fn test_cache_manager_reset_all_stats() {
        let mut mgr = CacheManager::new();
        // 产生一些查找统计
        mgr.bytecode_mut().lookup(&SourceHash::compute_str("x"));
        mgr.get_inline_cache("p").lookup_or_update(1, |_| Some(0));

        mgr.reset_all_stats();
        // 验证不 panic 且缓存数据仍在
        let report = mgr.print_stats_report();
        assert!(!report.is_empty());
    }

    #[test]
    fn test_cache_manager_print_stats_report_returns_non_empty() {
        let mgr = CacheManager::new();
        let report = mgr.print_stats_report();
        assert!(!report.is_empty());
        assert!(report.contains("Cache Statistics"));
    }

    #[test]
    fn test_cache_manager_string_pool_capacity_mut() {
        let mut mgr = CacheManager::new();
        mgr.string_pool_capacity_mut(512);
        assert_eq!(mgr.strings().max_capacity(), 512);
    }

    #[test]
    fn test_cache_manager_bytecode_cache_capacity_mut() {
        let mut mgr = CacheManager::new();
        mgr.bytecode_cache_capacity_mut(128);
        assert_eq!(mgr.bytecode().max_capacity(), 128);
    }

    // =========================================================================
    // CacheGlobalStats API 测试
    // =========================================================================

    #[test]
    fn test_cache_global_stats_is_healthy_empty() {
        let mgr = CacheManager::new();
        let stats = mgr.global_stats();
        // 空缓存（无查找）应判定为健康
        assert!(stats.is_healthy());
    }

    #[test]
    fn test_cache_global_stats_is_healthy_with_hits() {
        let mut mgr = CacheManager::new();
        // intern 一次，再查找一次（命中）
        let _ = mgr.strings_mut().intern("key");
        let _ = mgr.strings_mut().intern("key");
        let stats = mgr.global_stats();
        assert!(stats.is_healthy());
    }

    #[test]
    fn test_cache_global_stats_estimated_memory_usage_empty() {
        let mgr = CacheManager::new();
        let stats = mgr.global_stats();
        // 空缓存内存占用应为 0
        assert_eq!(stats.estimated_memory_usage(), 0);
    }

    #[test]
    fn test_cache_global_stats_estimated_memory_usage_non_empty() {
        let mut mgr = CacheManager::new();
        let _ = mgr.strings_mut().intern("hello");
        let stats = mgr.global_stats();
        assert!(stats.estimated_memory_usage() > 0);
    }
}
