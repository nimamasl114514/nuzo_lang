//! GenericArray — 编译期定长数组封装
//!
//! 基于原生 const generics（Rust 1.51+ 稳定），不引入 typenum/generic-array 外部 crate。

use crate::heap::TraceRef;
use std::ops::{Index, IndexMut};

/// 编译期定长数组
pub struct GenericArray<T, const N: usize> {
    inner: [T; N],
}

impl<T, const N: usize> GenericArray<T, N> {
    pub const fn new(arr: [T; N]) -> Self {
        Self { inner: arr }
    }
    pub const fn len(&self) -> usize {
        N
    }
    pub const fn is_empty(&self) -> bool {
        N == 0
    }
    pub fn as_slice(&self) -> &[T] {
        &self.inner
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.inner
    }
    pub fn into_inner(self) -> [T; N] {
        self.inner
    }
    pub fn from_fn<F: FnMut(usize) -> T>(f: F) -> Self {
        Self { inner: std::array::from_fn(f) }
    }
    pub fn map<U, F: FnMut(T) -> U>(self, f: F) -> GenericArray<U, N> {
        GenericArray { inner: self.inner.map(f) }
    }
}

impl<T: Clone, const N: usize> Clone for GenericArray<T, N> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<T: Copy, const N: usize> Copy for GenericArray<T, N> {}

impl<T: PartialEq, const N: usize> PartialEq for GenericArray<T, N> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T: Eq, const N: usize> Eq for GenericArray<T, N> {}

impl<T: std::fmt::Debug, const N: usize> std::fmt::Debug for GenericArray<T, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("GenericArray").field(&self.inner).finish()
    }
}

impl<T, const N: usize> Index<usize> for GenericArray<T, N> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        &self.inner[i]
    }
}

impl<T, const N: usize> IndexMut<usize> for GenericArray<T, N> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        &mut self.inner[i]
    }
}

impl<T, const N: usize> IntoIterator for GenericArray<T, N> {
    type Item = T;
    type IntoIter = std::array::IntoIter<T, N>;
    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a GenericArray<T, N> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter()
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a mut GenericArray<T, N> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter_mut()
    }
}

impl<T: Default, const N: usize> Default for GenericArray<T, N> {
    fn default() -> Self {
        Self::from_fn(|_| T::default())
    }
}

// ============================================================================
// GC TraceRef 集成
// ============================================================================

impl<T: TraceRef, const N: usize> TraceRef for GenericArray<T, N> {
    fn trace_ref(&self, marker: &mut dyn FnMut(u32)) {
        for item in &self.inner {
            item.trace_ref(marker);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic() {
        let arr = GenericArray::new([1, 2, 3, 4]);
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[2], 3);
    }

    #[test]
    fn test_map() {
        let arr = GenericArray::new([1, 2, 3]);
        let doubled: GenericArray<i32, 3> = arr.map(|x| x * 2);
        assert_eq!(doubled.as_slice(), &[2, 4, 6]);
    }

    #[test]
    fn test_from_fn() {
        let arr: GenericArray<u32, 4> = GenericArray::from_fn(|i| i as u32);
        assert_eq!(arr.as_slice(), &[0, 1, 2, 3]);
    }

    #[test]
    fn test_empty() {
        let arr: GenericArray<u32, 0> = GenericArray::new([]);
        assert!(arr.is_empty());
        assert_eq!(arr.len(), 0);
    }
}
