//! HList — 类型安全的异构列表
//!
//! 由 HNil（空列表）和 HCons<Head, Tail>（头尾递归）组成，长度在编译期可知。

use crate::heap::TraceRef;

mod sealed {
    pub trait Sealed {}
}

/// 异构列表 trait，由 HNil 和 HCons 实现。
pub trait HList: sealed::Sealed {
    /// 编译期已知长度
    const LEN: usize;
    /// 运行时长度（== Self::LEN）
    fn len(&self) -> usize {
        Self::LEN
    }
    /// 是否为空
    fn is_empty(&self) -> bool {
        Self::LEN == 0
    }
}

/// 空异构列表
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HNil;

/// 非空异构列表：head 元素 + tail 子列表
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HCons<H, T> {
    pub head: H,
    pub tail: T,
}

impl sealed::Sealed for HNil {}
impl<H, T: HList> sealed::Sealed for HCons<H, T> {}

impl HList for HNil {
    const LEN: usize = 0;
}

impl<H, T: HList> HList for HCons<H, T> {
    const LEN: usize = 1 + T::LEN;
}

impl<H, T> HCons<H, T> {
    /// 构造一个非空节点
    pub const fn new(head: H, tail: T) -> Self {
        Self { head, tail }
    }
    /// 取 head 引用
    pub fn head(&self) -> &H {
        &self.head
    }
    /// 取 tail 引用
    pub fn tail(&self) -> &T {
        &self.tail
    }
    /// 拆解为 (head, tail)
    pub fn into_head_tail(self) -> (H, T) {
        (self.head, self.tail)
    }
}

/// prepend：在任何 HList 前面加一个元素
pub trait HListPrepend<H>: HList {
    fn prepend(self, head: H) -> HCons<H, Self>
    where
        Self: Sized;
}

impl<H, T: HList> HListPrepend<H> for T {
    fn prepend(self, head: H) -> HCons<H, Self> {
        HCons::new(head, self)
    }
}

// 注意：避免 impl<T: HList> T { fn prepend } 这种 blanket impl，因为会和未来 trait 冲突。
// 用独立的 HListPrepend trait 更安全。

/// hlist! 宏：构造异构列表
#[macro_export]
macro_rules! hlist {
    () => { $crate::generic::hlist::HNil };
    ($head:expr $(, $tail:expr)* $(,)?) => {
        $crate::generic::hlist::HCons::new($head, $crate::hlist!($($tail),*))
    };
}

// ============================================================================
// GC TraceRef 集成
// ============================================================================

impl TraceRef for HNil {
    fn trace_ref(&self, _marker: &mut dyn FnMut(u32)) {}
}

impl<H: TraceRef, T: TraceRef + HList> TraceRef for HCons<H, T> {
    fn trace_ref(&self, marker: &mut dyn FnMut(u32)) {
        self.head.trace_ref(marker);
        self.tail.trace_ref(marker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hnil_len() {
        assert_eq!(HNil.len(), 0);
        assert!(HNil.is_empty());
    }

    #[test]
    fn test_hcons_len() {
        let h: HCons<i32, HCons<&str, HNil>> = hlist!(1, "a");
        assert_eq!(h.len(), 2);
        assert!(!h.is_empty());
    }
}
