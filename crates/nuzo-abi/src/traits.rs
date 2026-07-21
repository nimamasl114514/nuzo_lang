//! 共享 trait 定义。
//!
//! - [`Tracer`] — GC 追踪器的抽象接口，解耦 nuzo-abi 与 nuzo-vm 的 Gc 类型。
//! - [`NuzoTrace`] — 可被 GC 追踪的类型统一接口。

/// GC 追踪器的抽象接口，解耦 nuzo-abi 与 nuzo-vm 的 Gc 类型。
///
/// nuzo-vm 中的 `Gc` 类型实现此 trait，使得 `NuzoTrace::trace()` 可以
/// 在不依赖 nuzo-vm 的情况下被定义，避免循环依赖。
pub trait Tracer {
    /// 标记给定索引对应的堆对象为可达。
    fn mark_index(&mut self, idx: u32);
}

/// 可被 GC 追踪的类型统一接口。
///
/// 任何持有堆引用的类型都应实现此 trait，以便 GC 在标记阶段
/// 遍历其所有引用的堆对象。
///
/// # 为什么用 `dyn Tracer` 而非 `&mut Gc`？
///
/// nuzo-abi（L2）不能依赖 nuzo-vm（L6），否则形成循环：
/// `nuzo-vm → nuzo-abi → nuzo-vm`。
/// 使用 trait 对象将 Gc 的具体类型延迟到调用侧绑定。
pub trait NuzoTrace {
    /// 遍历 `self` 持有的所有堆引用，通过 `tracer` 标记为可达。
    fn trace(&self, tracer: &mut dyn Tracer);
}
