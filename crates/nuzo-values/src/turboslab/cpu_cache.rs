//! Per-CPU 缓存：LIFO 空闲栈

use super::RemoteFreeNode;
use super::TurboSlab;

/// Per-CPU 缓存：LIFO 空闲栈
pub struct TurboCpuCache {
    local_stack: Vec<u32>,
    partial_slab: Option<*mut TurboSlab>,
    remote_pending: Vec<RemoteFreeNode>,
}

impl TurboCpuCache {
    /// 创建空的 CPU 缓存。
    pub fn new() -> Self {
        Self { local_stack: Vec::new(), partial_slab: None, remote_pending: Vec::new() }
    }
    /// 将对象索引压入本地 LIFO 栈。
    pub fn push_local(&mut self, idx: u32) {
        self.local_stack.push(idx);
    }

    /// 从本地 LIFO 栈弹出对象索引。
    pub fn pop_local(&mut self) -> Option<u32> {
        self.local_stack.pop()
    }

    /// 设置当前关联的 partial slab。
    pub fn set_partial(&mut self, slab: *mut TurboSlab) {
        self.partial_slab = Some(slab);
    }

    /// 获取当前关联的 partial slab。
    pub fn get_partial(&self) -> Option<*mut TurboSlab> {
        self.partial_slab
    }

    /// 清空当前关联的 partial slab。
    pub fn clear_partial(&mut self) {
        self.partial_slab = None;
    }

    /// 返回本地 LIFO 栈的可变引用，用于批量填充空闲索引。
    pub fn local_stack_mut(&mut self) -> &mut Vec<u32> {
        &mut self.local_stack
    }

    /// 将远程释放节点加入待批处理队列。
    pub fn push_remote_pending(&mut self, node: RemoteFreeNode) {
        self.remote_pending.push(node);
    }

    ///  draining 并返回远程释放节点迭代器。
    pub fn drain_remote_pending(&mut self) -> impl Iterator<Item = RemoteFreeNode> {
        self.remote_pending.drain(..)
    }
}

impl Default for TurboCpuCache {
    fn default() -> Self {
        Self::new()
    }
}

/// 根据当前线程 ID 计算 CPU 索引（0..8）。
///
/// 使用 `std::thread::current().id()` 的 Debug 字符串进行稳定哈希，再对 8 取模。
/// 该方法仅依赖 std，不依赖平台相关 API（如 CPU 亲和性、NUMA 节点等），因此可
/// 在所有支持 std 的平台上工作。同一工作线程内调用结果稳定。
pub fn cpu_idx_from_thread() -> usize {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::thread;

    let mut hasher = DefaultHasher::new();
    format!("{:?}", thread::current().id()).hash(&mut hasher);
    (hasher.finish() % 8) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_cache_local_stack_lifo() {
        let mut cache = TurboCpuCache::new();
        assert_eq!(cache.pop_local(), None, "空栈应返回 None");

        cache.push_local(10);
        cache.push_local(20);
        cache.push_local(30);

        assert_eq!(cache.pop_local(), Some(30));
        assert_eq!(cache.pop_local(), Some(20));
        assert_eq!(cache.pop_local(), Some(10));
        assert_eq!(cache.pop_local(), None, "弹空后应再次返回 None");
    }

    #[test]
    fn test_cpu_cache_cpu_idx_deterministic() {
        let idx1 = cpu_idx_from_thread();
        let idx2 = cpu_idx_from_thread();
        assert_eq!(idx1, idx2, "同一线程两次计算的 CPU 索引应一致");
        assert!(idx1 < 8, "CPU 索引必须在 [0, 8) 范围内");
    }
}
