//! 高级泛型基础设施
//!
//! 提供 4 类工具：
//! - HList: 异构列表（hlist）
//! - Functor / Applicative / Monad: 函数式高阶 trait（functional）
//! - AnyMap: TypeId 索引的 type-erased 容器（any_map）
//! - GenericArray: const generics 定长数组封装（const_array）

pub mod any_map;
pub mod const_array;
pub mod functional;
pub mod hlist;

pub use any_map::AnyMap;
pub use const_array::GenericArray;
pub use functional::{Applicative, Functor, Monad};
pub use hlist::{HCons, HList, HListPrepend, HNil};
