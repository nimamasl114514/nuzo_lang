//! # NuzoDict -- Swiss-table 风格的字符串键字典
//!
//! 本模块实现了一个**两级策略**的字典数据结构，针对不同规模优化：
//!
//! - **SmallDict** (<= 8 条目): 线性扫描，零分配开销
//! - **LargeDict** (> 8 条目): 开放寻址 + ctrl-byte 元数据（类似 rustc-hash / Swiss-table）
//!
//! ## 键表示方式
//!
//! 键使用 `u32` 索引表示，指向全局字符串池中的驻留字符串（NaN-tagged string values）。
//! 这实现了 **O(1) 恒等比较**而无需克隆字符串数据。
//!
//! ## 哈希算法与冲突解决
//!
//! ### 哈希函数: Golden-Ratio Mixing
//! ```text
//! nuzo_mix(pool_index) = (pool_index * GOLDEN_64) >> 32
//! ```
//!
//! 使用黄金比例常数 `0x9E37_79B9_7F4A_7C15` 进行乘法哈希，
//! 对连续的 u32 池索引产生良好分布的 32 位哈希值。
//!
//! ### 冲突解决: 开放寻址 + 探测序列
//!
//! LargeDict 使用**线性探测 (Linear Probing)** 处理冲突：
//! ```text
//! bucket = hash & bucket_mask
//! while occupied:
//!     bucket = (bucket + 1) & bucket_mask   // 环形探测
//! ```
//!
//! ## 两级升级机制 (Tiered Upgrade)
//!
//! ```text
//! NuzoDict::Small(SmallDict)  --[>8条目]-->  NuzoDict::Large(LargeDict)
//!     线性扫描 O(n)                              开放寻址 O(1)
//!     零堆分配                                    ctrl-byte 元数据
//! ```
//!
//! 当 SmallDict 的条目数超过 `SMALL_CAP`(8) 时，自动升级为 LargeDict。
//! 升级过程将所有条目重新插入到新分配的 LargeDict 中。
//!
//! ## 内联缓存 (Inline Cache, IC) 支持
//!
//! NuzoDict 提供了 IC 友好的接口：
//! - [`shape_id()`]: 基于结构生成形状 ID，用于检测字典布局变化
//! - [`get_by_slot()`] / [`set_by_slot()`]: 通过槽位号直接访问（跳过哈希计算）
//! - [`get_with_slot()`] / [`insert_with_slot()`]: 返回槽位号供 VM 缓存

use std::fmt;

use crate::value::{Value, ValueExt};

// ============================================================================
// 哈希函数 (Hash Function)
// ============================================================================

use crate::constants::GOLDEN_64;

/// 黄金比例哈希混合函数 -- 用于字符串池索引。
///
/// 从顺序 u32 键产生分布良好的 32 位哈希值，
/// 对 LargeDict 的桶分配质量至关重要。
///
/// # 算法
///
/// ```text
/// nuzo_mix(key) = (key as u64 * GOLDEN_64) >> 32
/// ```
///
/// # 分布质量
///
/// 对于连续的 16 个键 (0..=16)，期望填充至少 11 个不同的桶（>=68%）。
#[inline]
pub fn nuzo_mix(pool_index: u32) -> u32 {
    let wide = (pool_index as u64).wrapping_mul(GOLDEN_64);
    (wide >> 32) as u32
}

// ============================================================================
// LargeDict 内部常量
// ============================================================================

/// SmallDict -> LargeDict 升级的阈值
const SMALL_CAP: usize = 8;

/// LargeDict 默认初始容量。
///
/// 新建的 LargeDict 默认分配 16 个桶，与 Swiss-table 实践一致，
/// 足以容纳约 14 个条目（7/8 负载因子）而无需扩容。
const SMALL_DICT_DEFAULT_CAPACITY: usize = 16;

/// ctrl byte 值：表示空槽位
const CTRL_EMPTY: u8 = 0xFF;
/// ctrl byte 值：表示已删除槽位（墓碑标记）
const CTRL_DELETED: u8 = 0x80;

// ============================================================================
// NuzoEntry -- Single key-value pair in a dictionary
// ============================================================================

/// A single entry in a NuzoDict, holding a string-pool key index and a Value.
#[derive(Debug, Clone, Copy)]
pub struct NuzoEntry {
    /// Index into the global string pool (from Value::string_index())
    pub key_index: u32,
    /// Pre-computed hash of key_index for fast lookup
    pub key_hash: u32,
    /// The stored value
    pub value: Value,
}

impl NuzoEntry {
    /// Create a new entry, computing the hash automatically.
    pub fn new(key_index: u32, value: Value) -> Self {
        Self { key_index, key_hash: nuzo_mix(key_index), value }
    }
}

// ============================================================================
// SmallDict -- Linear-scan dictionary for small counts
// ============================================================================

/// Linear-scan dictionary optimized for <= 8 entries.
///
/// Zero hash overhead, cache-friendly iteration, ideal for typical
/// object literals and small configuration dicts.
#[derive(Debug, Clone)]
pub struct SmallDict {
    entries: Vec<NuzoEntry>,
}

impl SmallDict {
    pub fn new() -> Self {
        Self { entries: Vec::with_capacity(SMALL_CAP) }
    }

    pub fn get(&self, key_index: u32) -> Option<Value> {
        for entry in &self.entries {
            if entry.key_index == key_index {
                return Some(entry.value);
            }
        }
        None
    }

    pub fn insert(&mut self, key_index: u32, value: Value) {
        for entry in &mut self.entries {
            if entry.key_index == key_index {
                entry.value = value;
                return;
            }
        }
        self.entries.push(NuzoEntry::new(key_index, value));
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, Value)> + '_ {
        self.entries.iter().map(|e| (e.key_index, e.value))
    }

    pub fn values(&self) -> impl Iterator<Item = Value> + '_ {
        self.entries.iter().map(|e| e.value)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Value> + '_ {
        self.entries.iter_mut().map(|e| &mut e.value)
    }

    pub fn contains_value(&self, target: &Value) -> bool {
        self.entries.iter().any(|e| e.value.value_equals(target))
    }

    pub fn into_entries(self) -> Vec<NuzoEntry> {
        self.entries
    }
}

impl Default for SmallDict {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// LargeDict -- Open-addressing hash table with ctrl-byte metadata
// ============================================================================

/// Open-addressing hash table using ctrl-byte metadata (Swiss-table style).
///
/// Automatically grows when load factor exceeds 7/8. Provides O(1) amortized
/// lookup for larger dictionaries.
#[derive(Debug, Clone)]
pub struct LargeDict {
    /// Control byte array (metadata for each slot: empty/deleted/h2 partial hash)
    ctrl: Vec<u8>,
    /// Slot array (parallel to ctrl, same length)
    entries: Vec<NuzoEntry>,
    /// Number of active (non-empty, non-deleted) entries
    len: usize,
    /// Bucket mask for modular arithmetic: `bucket = hash & bucket_mask`
    bucket_mask: u32,
}

impl Default for LargeDict {
    fn default() -> Self {
        Self::new()
    }
}

impl LargeDict {
    pub fn new() -> Self {
        Self::with_capacity(SMALL_DICT_DEFAULT_CAPACITY)
    }

    pub fn with_capacity(cap: usize) -> Self {
        let cap = cap.next_power_of_two().max(SMALL_DICT_DEFAULT_CAPACITY);
        Self {
            ctrl: vec![CTRL_EMPTY; cap],
            entries: (0..cap)
                .map(|_| NuzoEntry { key_index: 0, key_hash: 0, value: Value::default() })
                .collect(),
            len: 0,
            bucket_mask: (cap - 1) as u32,
        }
    }

    /// Build a LargeDict from pre-existing entries (used during SmallDict -> LargeDict upgrade).
    pub fn from_entries(entries: Vec<NuzoEntry>) -> Self {
        let cap = (entries.len() * 2).next_power_of_two().max(SMALL_DICT_DEFAULT_CAPACITY);
        let mut dict = Self {
            ctrl: vec![CTRL_EMPTY; cap],
            entries: (0..cap)
                .map(|_| NuzoEntry { key_index: 0, key_hash: 0, value: Value::default() })
                .collect(),
            len: 0,
            bucket_mask: (cap - 1) as u32,
        };
        for entry in entries {
            dict.insert_entry(entry);
        }
        dict
    }

    /// Insert an entry without checking growth (internal helper).
    fn insert_entry(&mut self, entry: NuzoEntry) {
        let h2 = h2_from_hash(entry.key_hash);
        let mut bucket = nuzo_bucket(entry.key_hash, self.bucket_mask);
        let cap = self.ctrl.len();

        let mut first_deleted = None;
        for _ in 0..cap {
            match self.ctrl[bucket] {
                CTRL_EMPTY => {
                    let slot = first_deleted.unwrap_or(bucket);
                    self.ctrl[slot] = h2;
                    self.entries[slot] = entry;
                    self.len += 1;
                    return;
                }
                CTRL_DELETED => {
                    if first_deleted.is_none() {
                        first_deleted = Some(bucket);
                    }
                    if self.entries[bucket].key_index == entry.key_index {
                        self.entries[bucket].value = entry.value;
                        return;
                    }
                }
                other if other == h2 && self.entries[bucket].key_index == entry.key_index => {
                    self.entries[bucket].value = entry.value;
                    return;
                }
                _ => {}
            }
            bucket = (bucket + 1) & self.bucket_mask as usize;
        }

        if let Some(slot) = first_deleted {
            self.ctrl[slot] = h2;
            self.entries[slot] = entry;
            self.len += 1;
        }
    }

    /// Find the slot for a given key, if present.
    fn find_slot(&self, key_index: u32, key_hash: u32) -> Option<usize> {
        let h2 = h2_from_hash(key_hash);
        let mut bucket = nuzo_bucket(key_hash, self.bucket_mask);
        let cap = self.ctrl.len();

        for _ in 0..cap {
            match self.ctrl[bucket] {
                CTRL_EMPTY => return None,
                CTRL_DELETED if self.entries[bucket].key_index == key_index => {
                    return Some(bucket);
                }
                other if other == h2 && self.entries[bucket].key_index == key_index => {
                    return Some(bucket);
                }
                _ => {}
            }
            bucket = (bucket + 1) & self.bucket_mask as usize;
        }
        None
    }

    pub fn get(&self, key_index: u32) -> Option<Value> {
        let key_hash = nuzo_mix(key_index);
        self.find_slot(key_index, key_hash).map(|slot| self.entries[slot].value)
    }

    pub fn insert(&mut self, key_index: u32, value: Value) {
        if self.len * 8 > self.ctrl.len() * 7 {
            self.grow();
        }

        let key_hash = nuzo_mix(key_index);
        let h2 = h2_from_hash(key_hash);
        let mut bucket = nuzo_bucket(key_hash, self.bucket_mask);
        let cap = self.ctrl.len();

        let mut first_deleted = None;
        for _ in 0..cap {
            match self.ctrl[bucket] {
                CTRL_EMPTY => {
                    let slot = first_deleted.unwrap_or(bucket);
                    self.ctrl[slot] = h2;
                    self.entries[slot] = NuzoEntry { key_index, key_hash, value };
                    self.len += 1;
                    return;
                }
                CTRL_DELETED => {
                    if first_deleted.is_none() {
                        first_deleted = Some(bucket);
                    }
                    if self.entries[bucket].key_index == key_index {
                        self.entries[bucket].value = value;
                        return;
                    }
                }
                other if other == h2 && self.entries[bucket].key_index == key_index => {
                    self.entries[bucket].value = value;
                    return;
                }
                _ => {}
            }
            bucket = (bucket + 1) & self.bucket_mask as usize;
        }

        if let Some(slot) = first_deleted {
            self.ctrl[slot] = h2;
            self.entries[slot] = NuzoEntry { key_index, key_hash, value };
            self.len += 1;
        }
    }

    /// Double the capacity and re-insert all entries.
    fn grow(&mut self) {
        let old_entries: Vec<NuzoEntry> = self
            .ctrl
            .iter()
            .zip(self.entries.iter())
            .filter(|(c, _)| **c != CTRL_EMPTY && **c != CTRL_DELETED)
            .map(|(_, e)| *e)
            .collect();

        let new_cap = (self.ctrl.len() * 2).max(SMALL_DICT_DEFAULT_CAPACITY);
        self.ctrl = vec![CTRL_EMPTY; new_cap];
        self.entries = (0..new_cap)
            .map(|_| NuzoEntry { key_index: 0, key_hash: 0, value: Value::default() })
            .collect();
        self.len = 0;
        self.bucket_mask = (new_cap - 1) as u32;

        for entry in old_entries {
            self.insert_entry(entry);
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, Value)> + '_ {
        self.ctrl
            .iter()
            .zip(self.entries.iter())
            .filter(|(c, _)| **c != CTRL_EMPTY && **c != CTRL_DELETED)
            .map(|(_, e)| (e.key_index, e.value))
    }

    pub fn values(&self) -> impl Iterator<Item = Value> + '_ {
        self.ctrl
            .iter()
            .zip(self.entries.iter())
            .filter(|(c, _)| **c != CTRL_EMPTY && **c != CTRL_DELETED)
            .map(|(_, e)| e.value)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Value> + '_ {
        self.ctrl
            .iter()
            .zip(self.entries.iter_mut())
            .filter(|(c, _)| **c != CTRL_EMPTY && **c != CTRL_DELETED)
            .map(|(_, e)| &mut e.value)
    }

    pub fn contains_value(&self, target: &Value) -> bool {
        self.values().any(|v| v.value_equals(target))
    }
}

// ============================================================================
// NuzoEnum Dict -- Two-tier dictionary enum
// ============================================================================

/// Two-tier dictionary that automatically upgrades from SmallDict to LargeDict.
///
/// Starts as a `SmallDict` (linear scan, zero alloc overhead) and promotes to
/// `LargeDict` (open addressing, O(1) lookup) when entry count exceeds `SMALL_CAP`.
#[derive(Debug, Clone)]
pub enum NuzoDict {
    /// Linear-scan variant for small dictionaries (<= 8 entries)
    Small(SmallDict),
    /// Open-addressing hash table for large dictionaries (> 8 entries)
    Large(LargeDict),
}

impl NuzoDict {
    pub fn new() -> Self {
        NuzoDict::Small(SmallDict::new())
    }

    /// Return a stable shape ID for this dict.
    ///
    /// The ID is a deterministic hash of all property key indices in slot
    /// iteration order, mixed with the table capacity. Dicts with the same
    /// length but different property names therefore get different IDs, and
    /// a Small->Large upgrade changes the capacity so old PIC entries are
    /// invalidated automatically.
    #[inline]
    pub fn shape_id(&self) -> u32 {
        // Empty dict keeps the historically expected shape ID of 0.
        if self.is_empty() {
            return 0;
        }
        let mut h: u32 = 0x9E37_79B9;
        for (key, _) in self.iter() {
            h = h.wrapping_mul(31).wrapping_add(key);
        }
        let capacity = match self {
            NuzoDict::Small(_) => SMALL_CAP as u32,
            NuzoDict::Large(l) => l.ctrl.len() as u32,
        };
        h = h.wrapping_mul(31).wrapping_add(capacity);
        // Avoid colliding with the empty-dict sentinel 0.
        if h == 0 { 1 } else { h }
    }

    /// Get value by slot index (IC fast path).
    /// Returns None if slot is out of bounds or invalid.
    #[inline]
    pub fn get_by_slot(&self, slot: usize) -> Option<Value> {
        match self {
            NuzoDict::Small(s) => s.entries.get(slot).map(|e| e.value),
            NuzoDict::Large(l) => {
                if slot < l.entries.len()
                    && l.ctrl[slot] != CTRL_EMPTY
                    && l.ctrl[slot] != CTRL_DELETED
                {
                    Some(l.entries[slot].value)
                } else {
                    None
                }
            }
        }
    }

    /// Set value by slot index (IC fast path for existing keys).
    #[inline]
    pub fn set_by_slot(&mut self, slot: usize, value: Value) {
        match self {
            NuzoDict::Small(s) => {
                if slot < s.entries.len() {
                    s.entries[slot].value = value;
                }
            }
            NuzoDict::Large(l) => {
                if slot < l.entries.len() {
                    l.entries[slot].value = value;
                }
            }
        }
    }

    /// Get value and return (value, Some(slot)) for IC caching.
    /// Returns (None, None) if key not found.
    pub fn get_with_slot(&self, key_index: u32) -> (Option<Value>, Option<usize>) {
        match self {
            NuzoDict::Small(s) => {
                for (i, entry) in s.entries.iter().enumerate() {
                    if entry.key_index == key_index {
                        return (Some(entry.value), Some(i));
                    }
                }
                (None, None)
            }
            NuzoDict::Large(l) => {
                let key_hash = nuzo_mix(key_index);
                if let Some(slot) = l.find_slot(key_index, key_hash) {
                    (Some(l.entries[slot].value), Some(slot))
                } else {
                    (None, None)
                }
            }
        }
    }

    /// Insert and return the slot index for IC caching.
    pub fn insert_with_slot(&mut self, key_index: u32, value: Value) -> Option<usize> {
        match self {
            NuzoDict::Small(s) => {
                for (i, entry) in s.entries.iter_mut().enumerate() {
                    if entry.key_index == key_index {
                        entry.value = value;
                        return Some(i);
                    }
                }
                s.entries.push(NuzoEntry::new(key_index, value));
                if s.len() > SMALL_CAP {
                    let entries = s.clone().into_entries();
                    *self = NuzoDict::Large(LargeDict::from_entries(entries));
                }
                match self {
                    NuzoDict::Small(s2) => Some(s2.len() - 1),
                    NuzoDict::Large(_) => None, // Upgraded, slot mapping changed
                }
            }
            NuzoDict::Large(l) => {
                let key_hash = nuzo_mix(key_index);
                if let Some(slot) = l.find_slot(key_index, key_hash) {
                    l.entries[slot].value = value;
                    return Some(slot);
                }
                l.insert(key_index, value);
                l.find_slot(key_index, key_hash)
            }
        }
    }

    pub fn get(&self, key_index: u32) -> Option<Value> {
        match self {
            NuzoDict::Small(s) => s.get(key_index),
            NuzoDict::Large(l) => l.get(key_index),
        }
    }

    pub fn insert(&mut self, key_index: u32, value: Value) {
        match self {
            NuzoDict::Small(s) => {
                if s.len() < SMALL_CAP {
                    s.insert(key_index, value);
                } else {
                    s.insert(key_index, value);
                    let entries = s.clone().into_entries();
                    *self = NuzoDict::Large(LargeDict::from_entries(entries));
                }
            }
            NuzoDict::Large(l) => l.insert(key_index, value),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        match self {
            NuzoDict::Small(s) => s.len(),
            NuzoDict::Large(l) => l.len(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        match self {
            NuzoDict::Small(s) => s.is_empty(),
            NuzoDict::Large(l) => l.is_empty(),
        }
    }

    pub fn iter(&self) -> NuzoDictIter<'_> {
        match self {
            NuzoDict::Small(s) => NuzoDictIter::Small(SmallDictIter { entries: s.entries.iter() }),
            NuzoDict::Large(l) => NuzoDictIter::Large(LargeDictIter {
                ctrl: l.ctrl.iter(),
                entries: l.entries.iter(),
            }),
        }
    }

    pub fn values(&self) -> NuzoDictValues<'_> {
        match self {
            NuzoDict::Small(s) => {
                NuzoDictValues::Small(SmallDictValuesIter { entries: s.entries.iter() })
            }
            NuzoDict::Large(l) => NuzoDictValues::Large(LargeDictValuesIter {
                ctrl: l.ctrl.iter(),
                entries: l.entries.iter(),
            }),
        }
    }

    pub fn values_mut(&mut self) -> Box<dyn Iterator<Item = &mut Value> + '_> {
        match self {
            NuzoDict::Small(s) => Box::new(s.values_mut()),
            NuzoDict::Large(l) => Box::new(l.values_mut()),
        }
    }

    pub fn contains_value(&self, target: &Value) -> bool {
        match self {
            NuzoDict::Small(s) => s.contains_value(target),
            NuzoDict::Large(l) => l.contains_value(target),
        }
    }

    /// Rough memory estimate for GC bookkeeping.
    pub fn size_estimate(&self) -> usize {
        match self {
            NuzoDict::Small(s) => s.len() * 16 + 32,
            NuzoDict::Large(l) => l.ctrl.len() * 17 + 32,
        }
    }
}

impl Default for NuzoDict {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Enum Iterators — 避免堆分配的零成本迭代器
// ============================================================================

/// NuzoDict 键值对迭代器（零堆分配，替代 Box<dyn Iterator>）
pub enum NuzoDictIter<'a> {
    Small(SmallDictIter<'a>),
    Large(LargeDictIter<'a>),
}

impl Iterator for NuzoDictIter<'_> {
    type Item = (u32, Value);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            NuzoDictIter::Small(it) => it.next(),
            NuzoDictIter::Large(it) => it.next(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            NuzoDictIter::Small(it) => it.size_hint(),
            NuzoDictIter::Large(it) => it.size_hint(),
        }
    }
}

/// NuzoDict 值迭代器（零堆分配，替代 Box<dyn Iterator>）
pub enum NuzoDictValues<'a> {
    Small(SmallDictValuesIter<'a>),
    Large(LargeDictValuesIter<'a>),
}

impl Iterator for NuzoDictValues<'_> {
    type Item = Value;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            NuzoDictValues::Small(it) => it.next(),
            NuzoDictValues::Large(it) => it.next(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            NuzoDictValues::Small(it) => it.size_hint(),
            NuzoDictValues::Large(it) => it.size_hint(),
        }
    }
}

// SmallDict 手动迭代器（避免 impl Iterator 类型不可命名问题）

pub struct SmallDictIter<'a> {
    entries: std::slice::Iter<'a, NuzoEntry>,
}

impl Iterator for SmallDictIter<'_> {
    type Item = (u32, Value);
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.entries.next().map(|e| (e.key_index, e.value))
    }
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

pub struct SmallDictValuesIter<'a> {
    entries: std::slice::Iter<'a, NuzoEntry>,
}

impl Iterator for SmallDictValuesIter<'_> {
    type Item = Value;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.entries.next().map(|e| e.value)
    }
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

// LargeDict 手动迭代器

pub struct LargeDictIter<'a> {
    ctrl: std::slice::Iter<'a, u8>,
    entries: std::slice::Iter<'a, NuzoEntry>,
}

impl Iterator for LargeDictIter<'_> {
    type Item = (u32, Value);
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let c = self.ctrl.next()?;
            let e = self.entries.next()?;
            if *c != CTRL_EMPTY && *c != CTRL_DELETED {
                return Some((e.key_index, e.value));
            }
        }
    }
}

pub struct LargeDictValuesIter<'a> {
    ctrl: std::slice::Iter<'a, u8>,
    entries: std::slice::Iter<'a, NuzoEntry>,
}

impl Iterator for LargeDictValuesIter<'_> {
    type Item = Value;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let c = self.ctrl.next()?;
            let e = self.entries.next()?;
            if *c != CTRL_EMPTY && *c != CTRL_DELETED {
                return Some(e.value);
            }
        }
    }
}

impl fmt::Display for NuzoDict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{")?;
        for (i, (key_index, val)) in self.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            let key_str = Value::string_from_index(key_index).unwrap_or_default();
            write!(f, "{}: {}", key_str, val)?;
        }
        write!(f, "}}")
    }
}

// ============================================================================
// Internal helpers
// ============================================================================

#[inline]
fn nuzo_bucket(pre_hash: u32, bucket_mask: u32) -> usize {
    (pre_hash & bucket_mask) as usize
}

#[inline]
fn h2_from_hash(pre_hash: u32) -> u8 {
    (pre_hash & 0x7F) as u8
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Value, ValueExt};

    #[test]
    fn test_nuzo_mix_distribution() {
        let mut buckets = [0usize; 16];
        for i in 0u32..16 {
            let hash = nuzo_mix(i);
            buckets[(hash & 15) as usize] += 1;
        }
        let occupied = buckets.iter().filter(|&&c| c > 0).count();
        assert!(
            occupied >= 11,
            "16 sequential keys should fill at least 11 of 16 buckets, got {}",
            occupied
        );
    }

    #[test]
    fn test_small_dict_insert_get() {
        let mut dict = SmallDict::new();
        let key_a = Value::from_string("alpha").string_index().unwrap();
        let key_b = Value::from_string("beta").string_index().unwrap();

        dict.insert(key_a, Value::from_number(1.0));
        dict.insert(key_b, Value::from_number(2.0));

        assert_eq!(dict.get(key_a), Some(Value::from_number(1.0)));
        assert_eq!(dict.get(key_b), Some(Value::from_number(2.0)));
        assert_eq!(dict.get(9999), None);
        assert_eq!(dict.len(), 2);
    }

    #[test]
    fn test_small_dict_update() {
        let mut dict = SmallDict::new();
        let key = Value::from_string("x").string_index().unwrap();

        dict.insert(key, Value::from_number(1.0));
        dict.insert(key, Value::from_number(42.0));

        assert_eq!(dict.get(key), Some(Value::from_number(42.0)));
        assert_eq!(dict.len(), 1);
    }

    #[test]
    fn test_large_dict_insert_get() {
        let mut dict = LargeDict::new();
        let keys: Vec<u32> = (0..20)
            .map(|i| Value::from_string(&format!("key_{}", i)).string_index().unwrap())
            .collect();

        for (i, &key) in keys.iter().enumerate() {
            dict.insert(key, Value::from_number(i as f64));
        }

        for (i, &key) in keys.iter().enumerate() {
            assert_eq!(dict.get(key), Some(Value::from_number(i as f64)));
        }
        assert_eq!(dict.get(99999), None);
    }

    #[test]
    fn test_nuzo_dict_small_to_large_upgrade() {
        let mut dict = NuzoDict::new();
        let keys: Vec<u32> = (0..10)
            .map(|i| Value::from_string(&format!("k{}", i)).string_index().unwrap())
            .collect();

        for (i, &key) in keys.iter().enumerate() {
            dict.insert(key, Value::from_number(i as f64));
        }

        assert!(matches!(dict, NuzoDict::Large(_)));
        assert_eq!(dict.len(), 10);

        for (i, &key) in keys.iter().enumerate() {
            assert_eq!(dict.get(key), Some(Value::from_number(i as f64)));
        }
    }

    #[test]
    fn test_nuzo_dict_iter() {
        let mut dict = NuzoDict::new();
        let key_a = Value::from_string("a").string_index().unwrap();
        let key_b = Value::from_string("b").string_index().unwrap();

        dict.insert(key_a, Value::from_number(1.0));
        dict.insert(key_b, Value::from_number(2.0));

        let entries: Vec<_> = dict.iter().collect();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_nuzo_dict_contains_value() {
        let mut dict = NuzoDict::new();
        let key = Value::from_string("x").string_index().unwrap();

        dict.insert(key, Value::from_number(42.0));
        assert!(dict.contains_value(&Value::from_number(42.0)));
        assert!(!dict.contains_value(&Value::from_number(99.0)));
    }

    #[test]
    fn test_large_dict_grow() {
        let mut dict = LargeDict::with_capacity(16);
        let keys: Vec<u32> = (0..30)
            .map(|i| Value::from_string(&format!("grow_{}", i)).string_index().unwrap())
            .collect();

        for (i, &key) in keys.iter().enumerate() {
            dict.insert(key, Value::from_number(i as f64));
        }

        for (i, &key) in keys.iter().enumerate() {
            assert_eq!(dict.get(key), Some(Value::from_number(i as f64)));
        }
        assert_eq!(dict.len(), 30);
    }

    // ─── LargeDict::from_entries ───
    #[test]
    fn test_large_dict_from_entries() {
        let entries: Vec<NuzoEntry> = (0..5)
            .map(|i| {
                let key = Value::from_string(&format!("fe_{}", i)).string_index().unwrap();
                NuzoEntry::new(key, Value::from_number(i as f64))
            })
            .collect();
        let dict = LargeDict::from_entries(entries);
        assert_eq!(dict.len(), 5);
        for i in 0..5 {
            let key = Value::from_string(&format!("fe_{}", i)).string_index().unwrap();
            assert_eq!(dict.get(key), Some(Value::from_number(i as f64)));
        }
    }

    #[test]
    fn test_large_dict_from_entries_empty() {
        let dict = LargeDict::from_entries(vec![]);
        assert_eq!(dict.len(), 0);
        assert!(dict.is_empty());
    }

    // ─── LargeDict::values_mut ───
    #[test]
    fn test_large_dict_values_mut() {
        let mut dict = LargeDict::new();
        let k1 = Value::from_string("lvm1").string_index().unwrap();
        let k2 = Value::from_string("lvm2").string_index().unwrap();
        dict.insert(k1, Value::from_number(1.0));
        dict.insert(k2, Value::from_number(2.0));

        for v in dict.values_mut() {
            *v = Value::from_number(99.0);
        }
        assert_eq!(dict.get(k1), Some(Value::from_number(99.0)));
        assert_eq!(dict.get(k2), Some(Value::from_number(99.0)));
    }

    // ─── SmallDict::into_entries ───
    #[test]
    fn test_small_dict_into_entries() {
        let mut sd = SmallDict::new();
        let k1 = Value::from_string("ie1").string_index().unwrap();
        let k2 = Value::from_string("ie2").string_index().unwrap();
        sd.insert(k1, Value::from_number(10.0));
        sd.insert(k2, Value::from_number(20.0));

        let entries = sd.into_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key_index, k1);
        assert_eq!(entries[0].value, Value::from_number(10.0));
        assert_eq!(entries[1].key_index, k2);
    }

    #[test]
    fn test_small_dict_into_entries_empty() {
        let sd = SmallDict::new();
        let entries = sd.into_entries();
        assert!(entries.is_empty());
    }

    // ─── SmallDict::values_mut ───
    #[test]
    fn test_small_dict_values_mut() {
        let mut sd = SmallDict::new();
        let k = Value::from_string("svm").string_index().unwrap();
        sd.insert(k, Value::from_number(1.0));

        for v in sd.values_mut() {
            *v = Value::from_number(42.0);
        }
        assert_eq!(sd.get(k), Some(Value::from_number(42.0)));
    }

    // ─── NuzoDict::shape_id ───
    #[test]
    fn test_shape_id_small_empty() {
        let d = NuzoDict::new();
        let id = d.shape_id();
        // Empty SmallDict: len=0, first_hash=0 -> 0*31+0 = 0
        assert_eq!(id, 0);
    }

    #[test]
    fn test_shape_id_small_with_entries() {
        let mut d = NuzoDict::new();
        let k = Value::from_string("shape1").string_index().unwrap();
        d.insert(k, Value::from_number(1.0));
        let id1 = d.shape_id();

        let mut d2 = NuzoDict::new();
        d2.insert(k, Value::from_number(2.0));
        let id2 = d2.shape_id();

        // Same property names (and order) produce the same shape_id regardless of values.
        assert_eq!(id1, id2);
        assert_ne!(id1, 0, "non-empty dict shape_id should not be 0");
    }

    #[test]
    fn test_shape_id_large() {
        let mut d = NuzoDict::new();
        for i in 0..10 {
            let k = Value::from_string(&format!("sh{}", i)).string_index().unwrap();
            d.insert(k, Value::from_number(i as f64));
        }
        // Should be Large variant now
        let id = d.shape_id();
        // Non-empty dict must have a non-zero shape_id.
        assert_ne!(id, 0);
    }

    #[test]
    fn test_shape_id_changes_with_structure() {
        let mut d1 = NuzoDict::new();
        let k1 = Value::from_string("a").string_index().unwrap();
        d1.insert(k1, Value::from_number(1.0));

        let mut d2 = NuzoDict::new();
        let k2 = Value::from_string("b").string_index().unwrap();
        d2.insert(k2, Value::from_number(1.0));

        // Same length, different property names -> different shape_id.
        let id1 = d1.shape_id();
        let id2 = d2.shape_id();
        assert_ne!(id1, id2, "dicts with different property names must have different shape_id");
        assert_ne!(id1, 0);
        assert_ne!(id2, 0);
    }

    // ─── NuzoDict::get_by_slot ───
    #[test]
    fn test_get_by_slot_small() {
        let mut d = NuzoDict::new();
        let k1 = Value::from_string("gbs1").string_index().unwrap();
        let k2 = Value::from_string("gbs2").string_index().unwrap();
        d.insert(k1, Value::from_number(1.0));
        d.insert(k2, Value::from_number(2.0));

        assert_eq!(d.get_by_slot(0), Some(Value::from_number(1.0)));
        assert_eq!(d.get_by_slot(1), Some(Value::from_number(2.0)));
        assert_eq!(d.get_by_slot(2), None);
    }

    #[test]
    fn test_get_by_slot_large() {
        let mut d = NuzoDict::new();
        for i in 0..10 {
            let k = Value::from_string(&format!("gbsl{}", i)).string_index().unwrap();
            d.insert(k, Value::from_number(i as f64));
        }
        // At least some slots should return values
        let mut found = 0;
        for slot in 0..64 {
            if d.get_by_slot(slot).is_some() {
                found += 1;
            }
        }
        assert_eq!(found, 10);
    }

    #[test]
    fn test_get_by_slot_out_of_bounds() {
        let d = NuzoDict::new();
        assert_eq!(d.get_by_slot(0), None);
        assert_eq!(d.get_by_slot(999), None);
    }

    // ─── NuzoDict::set_by_slot ───
    #[test]
    fn test_set_by_slot_small() {
        let mut d = NuzoDict::new();
        let k = Value::from_string("sbs").string_index().unwrap();
        d.insert(k, Value::from_number(1.0));
        d.set_by_slot(0, Value::from_number(99.0));
        assert_eq!(d.get_by_slot(0), Some(Value::from_number(99.0)));
    }

    #[test]
    fn test_set_by_slot_large() {
        let mut d = NuzoDict::new();
        for i in 0..10 {
            let k = Value::from_string(&format!("sbsl{}", i)).string_index().unwrap();
            d.insert(k, Value::from_number(i as f64));
        }
        // Find a valid slot and set it
        for slot in 0..64 {
            if d.get_by_slot(slot).is_some() {
                d.set_by_slot(slot, Value::from_number(777.0));
                assert_eq!(d.get_by_slot(slot), Some(Value::from_number(777.0)));
                break;
            }
        }
    }

    #[test]
    fn test_set_by_slot_out_of_bounds_no_panic() {
        let mut d = NuzoDict::new();
        d.set_by_slot(999, Value::from_number(1.0));
        // Should not panic, just silently ignore
    }

    // ─── NuzoDict::get_with_slot ───
    #[test]
    fn test_get_with_slot_small_found() {
        let mut d = NuzoDict::new();
        let k1 = Value::from_string("gws1").string_index().unwrap();
        let k2 = Value::from_string("gws2").string_index().unwrap();
        d.insert(k1, Value::from_number(1.0));
        d.insert(k2, Value::from_number(2.0));

        let (val, slot) = d.get_with_slot(k1);
        assert_eq!(val, Some(Value::from_number(1.0)));
        assert_eq!(slot, Some(0));

        let (val2, slot2) = d.get_with_slot(k2);
        assert_eq!(val2, Some(Value::from_number(2.0)));
        assert_eq!(slot2, Some(1));
    }

    #[test]
    fn test_get_with_slot_small_not_found() {
        let mut d = NuzoDict::new();
        let k = Value::from_string("gws_exist").string_index().unwrap();
        d.insert(k, Value::from_number(1.0));

        let missing_key = Value::from_string("gws_missing").string_index().unwrap();
        let (val, slot) = d.get_with_slot(missing_key);
        assert_eq!(val, None);
        assert_eq!(slot, None);
    }

    #[test]
    fn test_get_with_slot_large_found() {
        let mut d = NuzoDict::new();
        let keys: Vec<u32> = (0..10)
            .map(|i| Value::from_string(&format!("gwsl{}", i)).string_index().unwrap())
            .collect();
        for (i, &k) in keys.iter().enumerate() {
            d.insert(k, Value::from_number(i as f64));
        }

        for (i, &k) in keys.iter().enumerate() {
            let (val, slot) = d.get_with_slot(k);
            assert_eq!(val, Some(Value::from_number(i as f64)));
            assert!(slot.is_some());
        }
    }

    #[test]
    fn test_get_with_slot_large_not_found() {
        let mut d = NuzoDict::new();
        for i in 0..10 {
            let k = Value::from_string(&format!("gwsl_nf{}", i)).string_index().unwrap();
            d.insert(k, Value::from_number(i as f64));
        }
        let missing = Value::from_string("gwsl_missing").string_index().unwrap();
        let (val, slot) = d.get_with_slot(missing);
        assert_eq!(val, None);
        assert_eq!(slot, None);
    }

    // ─── NuzoDict::insert_with_slot ───
    #[test]
    fn test_insert_with_slot_small_new_key() {
        let mut d = NuzoDict::new();
        let k = Value::from_string("iws1").string_index().unwrap();
        let slot = d.insert_with_slot(k, Value::from_number(1.0));
        assert!(slot.is_some());
        assert_eq!(d.get(k), Some(Value::from_number(1.0)));
    }

    #[test]
    fn test_insert_with_slot_small_existing_key() {
        let mut d = NuzoDict::new();
        let k = Value::from_string("iws2").string_index().unwrap();
        d.insert(k, Value::from_number(1.0));
        let slot = d.insert_with_slot(k, Value::from_number(99.0));
        assert_eq!(slot, Some(0));
        assert_eq!(d.get(k), Some(Value::from_number(99.0)));
    }

    #[test]
    fn test_insert_with_slot_large_new_key() {
        let mut d = NuzoDict::new();
        // Fill to force upgrade to Large
        for i in 0..10 {
            let k = Value::from_string(&format!("iwsl{}", i)).string_index().unwrap();
            d.insert(k, Value::from_number(i as f64));
        }
        assert!(matches!(d, NuzoDict::Large(_)));

        let new_k = Value::from_string("iwsl_new").string_index().unwrap();
        let slot = d.insert_with_slot(new_k, Value::from_number(42.0));
        assert!(slot.is_some());
        assert_eq!(d.get(new_k), Some(Value::from_number(42.0)));
    }

    #[test]
    fn test_insert_with_slot_large_existing_key() {
        let mut d = NuzoDict::new();
        let keys: Vec<u32> = (0..10)
            .map(|i| Value::from_string(&format!("iwsle{}", i)).string_index().unwrap())
            .collect();
        for &k in &keys {
            d.insert(k, Value::from_number(0.0));
        }

        let slot = d.insert_with_slot(keys[3], Value::from_number(77.0));
        assert!(slot.is_some());
        assert_eq!(d.get(keys[3]), Some(Value::from_number(77.0)));
    }

    #[test]
    fn test_insert_with_slot_triggers_upgrade() {
        let mut d = NuzoDict::new();
        // Insert exactly SMALL_CAP+1 entries to trigger upgrade
        for i in 0..9 {
            let k = Value::from_string(&format!("upg{}", i)).string_index().unwrap();
            let slot = d.insert_with_slot(k, Value::from_number(i as f64));
            if i < 8 {
                assert!(slot.is_some(), "slot should be Some for SmallDict at i={}", i);
            }
        }
        // After 9 inserts, should be Large
        assert!(matches!(d, NuzoDict::Large(_)));
    }

    // ─── NuzoDict::values_mut ───
    #[test]
    fn test_nuzo_dict_values_mut_small() {
        let mut d = NuzoDict::new();
        let k1 = Value::from_string("nvm1").string_index().unwrap();
        let k2 = Value::from_string("nvm2").string_index().unwrap();
        d.insert(k1, Value::from_number(1.0));
        d.insert(k2, Value::from_number(2.0));

        for v in d.values_mut() {
            *v = Value::from_number(0.0);
        }
        assert_eq!(d.get(k1), Some(Value::from_number(0.0)));
        assert_eq!(d.get(k2), Some(Value::from_number(0.0)));
    }

    #[test]
    fn test_nuzo_dict_values_mut_large() {
        let mut d = NuzoDict::new();
        for i in 0..10 {
            let k = Value::from_string(&format!("nvml{}", i)).string_index().unwrap();
            d.insert(k, Value::from_number(i as f64));
        }

        for v in d.values_mut() {
            *v = Value::from_number(42.0);
        }
        for i in 0..10 {
            let k = Value::from_string(&format!("nvml{}", i)).string_index().unwrap();
            assert_eq!(d.get(k), Some(Value::from_number(42.0)));
        }
    }
}
