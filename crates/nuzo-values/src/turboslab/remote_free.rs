//! 远程释放节点：无锁 CAS 链表（Treiber stack）

use std::ptr::null_mut;
use std::sync::atomic::AtomicPtr;

/// 远程释放节点（无锁 CAS 链表 / Treiber 栈节点）
///
/// 用于跨 CPU 释放对象时构建 lock-free 链表，挂在目标 slab 的
/// `remote_free_head` 上。`generation` 预留用于 ABA 防护（当前实现通过
/// 每次分配全新节点 + 弹回时立即回收节点内存，避免经典 ABA）。
pub struct RemoteFreeNode {
    pub object_idx: u32,
    pub next: AtomicPtr<RemoteFreeNode>,
    pub generation: u64,
}

impl RemoteFreeNode {
    /// 创建一个新的远程释放节点，`next` 初始化为 null，`generation` 为 0。
    pub fn new(object_idx: u32) -> Box<Self> {
        Box::new(Self { object_idx, next: AtomicPtr::new(null_mut()), generation: 0 })
    }
}

// SAFETY: RemoteFreeNode contains only object_idx (u32, Copy), next
// (AtomicPtr, thread-safe), and generation (u64, Copy). AtomicPtr provides
// interior mutability with proper synchronization, so RemoteFreeNode can be
// safely sent and shared across threads.
unsafe impl Send for RemoteFreeNode {}
// SAFETY: All mutable access to next is through atomic operations (load/store)
// which provide proper synchronization. object_idx and generation are never
// mutated after construction. Concurrent reads and atomic updates are safe.
unsafe impl Sync for RemoteFreeNode {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turboslab::slab::{SizeClass, TurboSlab};
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_remote_free_concurrent() {
        let sc = SizeClass::new(64, 256);
        let slab = Arc::new(TurboSlab::new(&sc, 0));

        let mut handles = Vec::new();
        for thread_id in 0..4 {
            let slab_clone = Arc::clone(&slab);
            handles.push(thread::spawn(move || {
                for i in 0..64 {
                    let idx = (thread_id * 64 + i) as u32;
                    let node = RemoteFreeNode::new(idx);
                    let node_ptr = Box::into_raw(node);
                    unsafe { slab_clone.push_remote_free(node_ptr) };
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let mut count = 0usize;
        let mut seen = std::collections::HashSet::new();
        while let Some(node_ptr) = unsafe { slab.pop_remote_free() } {
            let node = unsafe { Box::from_raw(node_ptr) };
            count += 1;
            assert!(seen.insert(node.object_idx), "duplicate object_idx {}", node.object_idx);
        }
        assert_eq!(count, 256, "all remotely freed nodes should be recoverable");
    }

    #[test]
    fn test_concurrent_remote_free() {
        let sc = SizeClass::new(64, 256);
        let slab = Arc::new(TurboSlab::new(&sc, 0));

        let mut handles = Vec::new();
        for thread_id in 0..4 {
            let slab_clone = Arc::clone(&slab);
            handles.push(thread::spawn(move || {
                for i in 0..64 {
                    let idx = (thread_id * 64 + i) as u32;
                    let node = RemoteFreeNode::new(idx);
                    let node_ptr = Box::into_raw(node);
                    unsafe { slab_clone.push_remote_free(node_ptr) };
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let mut count = 0usize;
        let mut seen = std::collections::HashSet::new();
        while let Some(node_ptr) = unsafe { slab.pop_remote_free() } {
            let node = unsafe { Box::from_raw(node_ptr) };
            count += 1;
            assert!(seen.insert(node.object_idx), "duplicate object_idx {}", node.object_idx);
        }
        assert_eq!(count, 256, "all remotely freed nodes should be recoverable");
    }
}
