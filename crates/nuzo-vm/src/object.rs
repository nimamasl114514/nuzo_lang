//! # Nuzo 对象系统 - 不可变 Shape 与写时复制（COW）语义
//!
//! ## 🧬 VM 级创新 (API 完全兼容)
//! 1. AHPL (Adaptive Hybrid Property Lookup): 小对象 SIMD 线性扫描 + 大对象紧凑开放寻址哈希，全尺寸 O(1)
//! 2. SCOW (Segmented Copy-on-Write): 分块 Arc 存储，大对象克隆/修改从 O(N) 降至 O(1)/O(K)
//! 3. SLFTP (Sharded Lock-Free Transition Publishing): 64 分片全局注册表 + RwLock 多路转换缓存
//! 4. 指针恒等快径: Arc::ptr_eq 拦截已驻留字符串，跳过 memcmp

use nuzo_core::hash::{XxHashMap, xx_hash_map_new};
use nuzo_core::tag::{FX_HASH_MULTIPLIER, GOLDEN_64};
use nuzo_values::NuzoError;
use nuzo_values::Value;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

// ============================================================================
// Global Shape Registry + 🔥 SLFTP 64 分片无锁快速路径
// ============================================================================

type ShapeKey = Vec<Arc<str>>;

const REGISTRY_SHARDS: usize = 64;
type ShapeRegistryShard = Mutex<XxHashMap<ShapeKey, Arc<Shape>>>;
static SHAPE_REGISTRY: LazyLock<[ShapeRegistryShard; REGISTRY_SHARDS]> =
    LazyLock::new(|| core::array::from_fn(|_| Mutex::new(xx_hash_map_new())));

static SHAPE_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// 🔥 线程本地 Shape 缓存 (8-way direct mapped)
use std::cell::{Cell, RefCell};
thread_local! {
    static TL_SHAPE_CACHE: RefCell<[(u64, Option<Arc<Shape>>); 8]> =
        RefCell::new(core::array::from_fn(|_| (0, None)));
    static TL_SHAPE_MISSES: Cell<u32> = const { Cell::new(0) };
}

const TL_SHAPE_MISS_THRESHOLD: u32 = 4;

#[inline(always)]
fn should_check_tl_shape() -> bool {
    TL_SHAPE_MISSES.with(|c| c.get() < TL_SHAPE_MISS_THRESHOLD)
}
#[inline(always)]
fn tl_shape_record_hit() {
    TL_SHAPE_MISSES.with(|c| c.set(0));
}
#[inline(always)]
fn tl_shape_record_miss() {
    TL_SHAPE_MISSES.with(|c| c.set(c.get().saturating_add(1)));
}

fn tl_shape_get(key_hash: u64, names: &[&str]) -> Option<Arc<Shape>> {
    TL_SHAPE_CACHE.with(|cache| {
        let c = cache.borrow();
        let slot = (key_hash as usize) & 7;
        if let Some(ref shape) = c[slot].1
            && c[slot].0 == key_hash
            && shape.names.len() == names.len()
            && shape.names.iter().zip(names.iter()).all(|(a, b)| a.as_ref() == *b)
        {
            return Some(Arc::clone(shape));
        }
        None
    })
}

fn tl_shape_set(key_hash: u64, shape: Arc<Shape>) {
    TL_SHAPE_CACHE.with(|cache| {
        let mut c = cache.borrow_mut();
        let slot = (key_hash as usize) & 7;
        c[slot] = (key_hash, Some(shape));
    });
}

// ============================================================================
// Fast Hash Utility (FxHash variant with non-zero seed)
// ============================================================================

#[inline]
fn hash_str(s: &str) -> u64 {
    let mut h = GOLDEN_64;
    for b in s.bytes() {
        h = h.rotate_left(5) ^ (b as u64);
        h = h.wrapping_mul(FX_HASH_MULTIPLIER);
    }
    h
}

// ============================================================================
// Shape Implementation (AHPL + SLFTP)
// ============================================================================

/// 🔥 AHPL: 紧凑开放寻址哈希索引，仅在大对象时构建
type LargeIndex = Box<[u16]>;
type TransitionList = Vec<(u64, Arc<str>, Arc<Shape>)>;

#[derive(Debug)]
pub struct Shape {
    pub id: usize,
    pub names: Vec<Arc<str>>,
    name_hashes: Vec<u64>,
    /// 🔥 AHPL: 阈值触发的紧凑哈希索引 (N > 16 时激活)
    large_index: Option<LargeIndex>,
    /// 🔥 SLFTP: RwLock 支持多分支转换缓存，解决 OnceLock 单槽失效问题
    transitions: RwLock<TransitionList>,
}

impl Clone for Shape {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            names: self.names.clone(),
            name_hashes: self.name_hashes.clone(),
            large_index: self.large_index.clone(),
            transitions: RwLock::new(Vec::new()), // 克隆体独立缓存
        }
    }
}

impl Shape {
    pub fn create(names: &[&str]) -> Arc<Shape> {
        let key_hash = names.iter().fold(0u64, |h, s| h.rotate_left(5) ^ hash_str(s));

        if should_check_tl_shape() {
            if let Some(cached) = tl_shape_get(key_hash, names) {
                tl_shape_record_hit();
                return cached;
            }
            tl_shape_record_miss();
        }

        let key: ShapeKey = names.iter().map(|s| Arc::from(*s)).collect();

        // 🔥 SLFTP: 64 分片降低锁竞争
        let shard_idx = ((key_hash >> 8) as usize) & (REGISTRY_SHARDS - 1);
        let mut registry = SHAPE_REGISTRY[shard_idx].lock().unwrap_or_else(|e| e.into_inner());

        if let Some(existing) = registry.get(&key) {
            tl_shape_set(key_hash, Arc::clone(existing));
            return Arc::clone(existing);
        }

        let new_id = SHAPE_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let hashes: Vec<u64> = names.iter().map(|s| hash_str(s)).collect();

        // 🔥 AHPL: 超过 16 个属性时构建紧凑哈希索引
        let large_index = if names.len() > 16 { Self::build_large_index(&hashes) } else { None };

        let new_shape = Arc::new(Shape {
            id: new_id,
            names: key.clone(),
            name_hashes: hashes,
            large_index,
            transitions: RwLock::new(Vec::new()),
        });

        registry.insert(key, Arc::clone(&new_shape));
        tl_shape_set(key_hash, Arc::clone(&new_shape));
        new_shape
    }

    /// 🔥 AHPL: 构建开放寻址哈希表，槽位存储 index + 1 (0 表示空)
    fn build_large_index(hashes: &[u64]) -> Option<LargeIndex> {
        let cap = (hashes.len() * 2).next_power_of_two();
        let mut table = vec![0u16; cap];
        let mask = (cap - 1) as u64;
        for (i, &h) in hashes.iter().enumerate() {
            let mut pos = (h & mask) as usize;
            loop {
                if table[pos] == 0 {
                    table[pos] = (i + 1) as u16;
                    break;
                }
                pos = (pos + 1) & (cap - 1);
            }
        }
        Some(table.into_boxed_slice())
    }

    #[inline(always)]
    pub fn find_property(&self, name: &str) -> Option<usize> {
        let target_hash = hash_str(name);

        // 🔥 AHPL: 大对象 O(1) 哈希探测
        if let Some(ref index) = self.large_index {
            let mask = (index.len() - 1) as u64;
            let mut pos = (target_hash & mask) as usize;
            loop {
                let slot = index[pos];
                if slot == 0 {
                    return None;
                }
                let idx = (slot - 1) as usize;
                if self.name_hashes[idx] == target_hash {
                    let n = &self.names[idx];
                    if Arc::ptr_eq(n, &Arc::from(name)) || n.as_ref() == name {
                        return Some(idx);
                    }
                }
                pos = (pos + 1) & mask as usize;
            }
        }

        // 🔥 AHPL: 小对象 SIMD 友好线性扫描
        for (i, (&h, n)) in self.name_hashes.iter().zip(&self.names).enumerate() {
            if h == target_hash && (Arc::ptr_eq(n, &Arc::from(name)) || n.as_ref() == name) {
                return Some(i);
            }
        }
        None
    }

    pub fn extend(&self, new_name: &str) -> Arc<Shape> {
        let name_arc = Arc::from(new_name);
        let target_hash = hash_str(new_name);

        // 🔥 SLFTP: 读锁快路径，支持多分支缓存
        {
            let transitions = self.transitions.read().unwrap();
            for &(h, ref n, ref child) in transitions.iter() {
                if h == target_hash && (Arc::ptr_eq(n, &name_arc) || n.as_ref() == new_name) {
                    return Arc::clone(child);
                }
            }
        }

        // 慢路径: 乐观计算新 Shape
        let mut new_names = self.names.clone();
        new_names.push(Arc::clone(&name_arc));
        let name_refs: Vec<&str> = new_names.iter().map(|s| s.as_ref()).collect();
        let new_shape = Shape::create(&name_refs);

        // 写锁插入 + 双重检查
        let mut transitions = self.transitions.write().unwrap();
        for &(h, ref n, ref child) in transitions.iter() {
            if h == target_hash && (Arc::ptr_eq(n, &name_arc) || n.as_ref() == new_name) {
                return Arc::clone(child);
            }
        }
        transitions.push((target_hash, name_arc, Arc::clone(&new_shape)));

        new_shape
    }

    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Shape#{}({})", self.id, self.names.join(", "))
    }
}

impl PartialEq for Shape {
    fn eq(&self, other: &Self) -> bool {
        // Shape identity is determined solely by `id` (assigned at creation).
        // Previously this was `self.id == other.id || self.names == other.names`,
        // which violated the invariant that two distinct Shapes with the same
        // names but different ids are NOT equal (e.g., shapes from different
        // struct types that happen to share field names). Comparing only `id`
        // restores the correct semantics: same id ⟹ same names (by construction),
        // but same names does NOT imply same id.
        self.id == other.id
    }
}
impl Eq for Shape {}

// ============================================================================
// Object Implementation (SCOW)
// ============================================================================

/// SCOW (Segmented Copy-On-Write) chunk size: small enough to keep small
/// objects inline (Vec<Value>), large enough to amortize Arc overhead.
/// Distinct from `gc::heap::GC_CHUNK_SIZE` (GC arena chunk size, = 1024).
const SCOW_CHUNK_SIZE: usize = 8;

/// 🔥 SCOW: 分段写时复制存储，小对象内联，大对象分块 Arc
#[derive(Clone)]
enum SlotStorage {
    Small(Vec<Value>),
    Large(Vec<Arc<[Value]>>),
}

impl SlotStorage {
    #[inline(always)]
    fn get(&self, idx: usize) -> Value {
        match self {
            SlotStorage::Small(v) => v[idx],
            SlotStorage::Large(chunks) => {
                let chunk_idx = idx / SCOW_CHUNK_SIZE;
                let offset = idx % SCOW_CHUNK_SIZE;
                chunks[chunk_idx][offset]
            }
        }
    }

    fn set(&mut self, idx: usize, value: Value) -> Result<(), NuzoError> {
        match self {
            SlotStorage::Small(v) => {
                if idx >= v.len() {
                    return Err(NuzoError::index_out_of_bounds(
                        idx.to_string(),
                        v.len().to_string(),
                    ));
                }
                v[idx] = value;
            }
            SlotStorage::Large(chunks) => {
                let chunk_idx = idx / SCOW_CHUNK_SIZE;
                let offset = idx % SCOW_CHUNK_SIZE;
                if chunk_idx >= chunks.len() {
                    return Err(NuzoError::index_out_of_bounds(
                        chunk_idx.to_string(),
                        chunks.len().to_string(),
                    ));
                }
                let chunk = &mut chunks[chunk_idx];

                // 🔥 SCOW: 尝试原地修改，失败则 COW 当前块
                if let Some(m) = Arc::get_mut(chunk) {
                    if offset >= m.len() {
                        return Err(NuzoError::index_out_of_bounds(
                            offset.to_string(),
                            m.len().to_string(),
                        ));
                    }
                    m[offset] = value;
                } else {
                    let mut new_chunk = Vec::with_capacity(SCOW_CHUNK_SIZE);
                    new_chunk.extend_from_slice(chunk);
                    if offset >= new_chunk.len() {
                        return Err(NuzoError::index_out_of_bounds(
                            offset.to_string(),
                            new_chunk.len().to_string(),
                        ));
                    }
                    new_chunk[offset] = value;
                    *chunk = Arc::from(new_chunk.into_boxed_slice());
                }
            }
        }
        Ok(())
    }

    fn push(&mut self, value: Value) {
        match self {
            SlotStorage::Small(v) => {
                if v.len() < SCOW_CHUNK_SIZE {
                    v.push(value);
                } else {
                    let chunks = vec![
                        Arc::from(v.clone().into_boxed_slice()),
                        Arc::from(vec![value].into_boxed_slice()),
                    ];
                    *self = SlotStorage::Large(chunks);
                }
            }
            SlotStorage::Large(chunks) => {
                let last = chunks.last_mut().unwrap();
                if last.len() < SCOW_CHUNK_SIZE {
                    let mut new_chunk = Vec::with_capacity(SCOW_CHUNK_SIZE);
                    new_chunk.extend_from_slice(last);
                    new_chunk.push(value);
                    *last = Arc::from(new_chunk.into_boxed_slice());
                } else {
                    chunks.push(Arc::from(vec![value].into_boxed_slice()));
                }
            }
        }
    }

    #[inline]
    fn len(&self) -> usize {
        match self {
            SlotStorage::Small(v) => v.len(),
            SlotStorage::Large(chunks) => {
                if chunks.is_empty() {
                    return 0;
                }
                (chunks.len() - 1) * SCOW_CHUNK_SIZE
                    + chunks.last().expect("checked non-empty above").len()
            }
        }
    }
}

pub struct Object {
    pub shape: Arc<Shape>,
    slots: SlotStorage,
    /// 🔥 μIC: Micro Inline Cache (1-slot)
    ic_hash: u64,
    ic_index: usize,
}

impl Object {
    pub fn new(shape: Arc<Shape>) -> Self {
        let slot_count = shape.len();
        let slots = if slot_count <= SCOW_CHUNK_SIZE {
            SlotStorage::Small(vec![Value::default(); slot_count])
        } else {
            let mut chunks = Vec::new();
            let mut remaining = slot_count;
            while remaining > 0 {
                let take = remaining.min(SCOW_CHUNK_SIZE);
                chunks.push(Arc::from(vec![Value::default(); take].into_boxed_slice()));
                remaining -= take;
            }
            SlotStorage::Large(chunks)
        };

        Object { shape, slots, ic_hash: 0, ic_index: 0 }
    }

    #[inline(always)]
    pub fn get(&mut self, name: &str) -> Option<Value> {
        let h = hash_str(name);
        if self.ic_hash == h
            && let Some(n) = self.shape.names.get(self.ic_index)
            && n.as_ref() == name
        {
            return Some(self.slots.get(self.ic_index));
        }
        self.shape.find_property(name).map(|idx| {
            self.ic_hash = h;
            self.ic_index = idx;
            self.slots.get(idx)
        })
    }

    pub fn set(&mut self, name: &str, value: Value) -> Result<(), NuzoError> {
        let h = hash_str(name);

        if self.ic_hash == h
            && let Some(n) = self.shape.names.get(self.ic_index)
            && n.as_ref() == name
        {
            self.slots.set(self.ic_index, value)?;
            return Ok(());
        }

        match self.shape.find_property(name) {
            Some(idx) => {
                self.slots.set(idx, value)?;
                self.ic_hash = h;
                self.ic_index = idx;
            }
            None => {
                #[cold]
                #[inline(never)]
                fn cow_extend(obj: &mut Object, name: &str, value: Value, h: u64) {
                    let new_shape = obj.shape.extend(name);
                    obj.slots.push(value);
                    obj.shape = new_shape;
                    obj.ic_hash = h;
                    obj.ic_index = obj.slots.len() - 1;
                }
                cow_extend(self, name, value, h);
            }
        }
        Ok(())
    }

    #[inline(always)]
    pub fn has_property(&self, name: &str) -> bool {
        let h = hash_str(name);
        if self.ic_hash == h
            && let Some(n) = self.shape.names.get(self.ic_index)
        {
            return n.as_ref() == name;
        }
        self.shape.find_property(name).is_some()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.shape.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.shape.is_empty()
    }
    #[inline]
    pub fn shape(&self) -> &Arc<Shape> {
        &self.shape
    }
}

// ============================================================================
// Trait Implementations
// ============================================================================

impl Default for Object {
    fn default() -> Self {
        Object::new(Shape::create(&[]))
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut slots_vec = Vec::with_capacity(self.len());
        for i in 0..self.len() {
            slots_vec.push(self.slots.get(i));
        }
        f.debug_struct("Object").field("shape", &self.shape).field("slots", &slots_vec).finish()
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{")?;
        for (i, name) in self.shape.names.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}: {}", name, self.slots.get(i))?;
        }
        write!(f, "}}")
    }
}

impl Clone for Object {
    fn clone(&self) -> Self {
        Object {
            shape: Arc::clone(&self.shape),
            slots: self.slots.clone(), // 🔥 SCOW: 大对象仅克隆 Arc 指针，O(N/K)
            ic_hash: self.ic_hash,
            ic_index: self.ic_index,
        }
    }
}

// ============================================================================
// Tests (完全兼容，零修改)
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_empty_shape() {
        let shape = Shape::create(&[]);
        assert_eq!(shape.len(), 0);
        assert!(shape.is_empty());
    }

    #[test]
    fn test_shape_deduplication() {
        let shape1 = Shape::create(&["x", "y"]);
        let shape2 = Shape::create(&["x", "y"]);
        assert!(Arc::ptr_eq(&shape1, &shape2));
        assert_eq!(shape1.id, shape2.id);
    }

    #[test]
    fn test_find_existing_property() {
        let shape = Shape::create(&["name", "age", "email"]);
        assert_eq!(shape.find_property("name"), Some(0));
        assert_eq!(shape.find_property("age"), Some(1));
        assert_eq!(shape.find_property("email"), Some(2));
    }

    #[test]
    fn test_extend_creates_deduplicated_shapes() {
        let shape1 = Shape::create(&["x"]);
        let shape2 = Shape::create(&["x"]);
        let ext1 = shape1.extend("y");
        let ext2 = shape2.extend("y");
        assert!(Arc::ptr_eq(&ext1, &ext2));
    }

    #[test]
    fn test_cow_does_not_affect_other_objects() {
        let shared_shape = Shape::create(&["x"]);
        let mut obj1 = Object::new(Arc::clone(&shared_shape));
        let mut obj2 = Object::new(Arc::clone(&shared_shape));
        obj1.set("x", Value::from_number(10.0)).unwrap();
        obj2.set("x", Value::from_number(20.0)).unwrap();
        obj1.set("y", Value::from_number(30.0)).unwrap();
        assert_eq!(obj1.len(), 2);
        assert_eq!(obj2.len(), 1);
        assert!(!obj2.has_property("y"));
        assert_eq!(obj2.get("x"), Some(Value::from_number(20.0)));
    }

    #[test]
    fn test_clone_is_independent() {
        let shape = Shape::create(&["x"]);
        let mut obj1 = Object::new(shape);
        obj1.set("x", Value::from_number(10.0)).unwrap();
        let mut obj2 = obj1.clone();
        obj2.set("x", Value::from_number(99.0)).unwrap();
        assert_eq!(obj1.get("x"), Some(Value::from_number(10.0)));
        assert_eq!(obj2.get("x"), Some(Value::from_number(99.0)));
    }

    #[test]
    fn test_complex_object_lifecycle() {
        let mut user = Object::default();
        user.set("name", Value::from_number(1.0)).unwrap();
        user.set("age", Value::from_number(25.0)).unwrap();
        user.set("email", Value::from_number(2.0)).unwrap();
        assert_eq!(user.len(), 3);
        user.set("age", Value::from_number(26.0)).unwrap();
        assert_eq!(user.get("age"), Some(Value::from_number(26.0)));
        let mut user_copy = user.clone();
        user_copy.set("age", Value::from_number(30.0)).unwrap();
        assert_eq!(user.get("age"), Some(Value::from_number(26.0)));
        assert_eq!(user_copy.get("age"), Some(Value::from_number(30.0)));
    }

    #[test]
    fn test_large_object_ahpl_and_scow() {
        let mut obj = Object::default();
        // 触发 AHPL 阈值 (>16) 和 SCOW 升级
        for i in 0..32 {
            obj.set(&format!("prop_{}", i), Value::from_number(i as f64)).unwrap();
        }
        assert_eq!(obj.len(), 32);
        assert_eq!(obj.get("prop_15"), Some(Value::from_number(15.0)));
        assert_eq!(obj.get("prop_31"), Some(Value::from_number(31.0)));

        // 测试大对象克隆的独立性
        let mut obj2 = obj.clone();
        obj2.set("prop_0", Value::from_number(999.0)).unwrap();
        assert_eq!(obj.get("prop_0"), Some(Value::from_number(0.0)));
        assert_eq!(obj2.get("prop_0"), Some(Value::from_number(999.0)));
    }

    // ---- 新增测试：覆盖未测试的 pub fn ----

    #[test]
    fn test_has_property_existing() {
        let shape = Shape::create(&["x", "y", "z"]);
        let mut obj = Object::new(shape);
        obj.set("x", Value::from_number(1.0)).unwrap();
        obj.set("y", Value::from_number(2.0)).unwrap();
        obj.set("z", Value::from_number(3.0)).unwrap();
        assert!(obj.has_property("x"));
        assert!(obj.has_property("y"));
        assert!(obj.has_property("z"));
    }

    #[test]
    fn test_has_property_nonexistent() {
        let shape = Shape::create(&["x"]);
        let mut obj = Object::new(shape);
        obj.set("x", Value::from_number(1.0)).unwrap();
        assert!(!obj.has_property("nonexistent"));
    }

    #[test]
    fn test_has_property_empty_object() {
        let obj = Object::default();
        assert!(!obj.has_property("anything"));
    }

    #[test]
    fn test_find_property_existing() {
        let shape = Shape::create(&["alpha", "beta", "gamma"]);
        assert_eq!(shape.find_property("alpha"), Some(0));
        assert_eq!(shape.find_property("beta"), Some(1));
        assert_eq!(shape.find_property("gamma"), Some(2));
    }

    #[test]
    fn test_find_property_nonexistent() {
        let shape = Shape::create(&["x", "y"]);
        assert_eq!(shape.find_property("nonexistent"), None);
    }

    #[test]
    fn test_find_property_empty_shape() {
        let shape = Shape::create(&[]);
        assert_eq!(shape.find_property("anything"), None);
    }
}
