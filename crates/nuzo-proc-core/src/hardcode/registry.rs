//! # 全局常量注册表
//!
//! 提供 [`register`] / [`get`] / [`all`] / [`by_module`] / [`by_type`] 等函数，
//! 用于在运行时查询由 `define_constants!` 宏注册的所有常量元数据。
//!
//! ## 设计要点
//!
//! - **按值查询**：`get` 返回 `Option<ConstantInfo>`（`ConstantInfo: Copy`，
//!   全 `&'static str` 字段，拷贝开销可忽略），从根上避免了把 `Mutex` guard 内
//!   的引用延长到锁外的悬垂引用风险。
//! - **插入顺序保留**：`all()` 按 `define_constants!` 中的声明顺序返回，
//!   便于生成稳定的 JSON 导出与文档。
//! - **线程安全**：内部使用 `Mutex<RegistryInner>` + `Lazy`，初始化无竞争。
//! - **测试隔离**：`clear()` 仅在 `test-utils` feature 启用时可用，
//!   避免生产代码误清空注册表。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use super::types::ConstantInfo;

/// 注册函数类型：由 `define_constants!` 宏生成的 `__register_*` 函数签名。
pub type RegisterFn = fn();

/// 注册表内部状态。
struct RegistryInner {
    /// name -> ConstantInfo 的映射
    constants: HashMap<&'static str, ConstantInfo>,
    /// 按注册顺序排列的常量名（用于稳定迭代顺序）
    order: Vec<&'static str>,
}

impl RegistryInner {
    fn new() -> Self {
        Self { constants: HashMap::new(), order: Vec::new() }
    }
}

/// 全局注册表（线程安全单例）。
static REGISTRY: LazyLock<Mutex<RegistryInner>> =
    LazyLock::new(|| Mutex::new(RegistryInner::new()));

/// 获取注册表锁，poisoned 时恢复（不 panic）
///
/// Mutex poisoned 表示持有锁的线程 panic，但锁本身仍可获取。
/// 采用 poison recovery 模式：取出内部数据继续使用，避免因一个线程
/// panic 导致整个进程不可用。注册表是只读查询场景居多，poison 后
/// 数据可能不一致但仍可提供 best-effort 服务。
fn lock_registry() -> std::sync::MutexGuard<'static, RegistryInner> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// 注册一个常量到全局注册表。
///
/// 由 `define_constants!` 宏生成的 `__register_*` 函数调用。
/// 重复注册同名常量会被忽略（保留首次注册的信息）。
///
/// # 线程安全
///
/// 内部加锁，可在任意线程调用。
pub fn register(info: ConstantInfo) {
    let mut guard = lock_registry();
    if guard.constants.contains_key(info.name) {
        // 重复注册：保留首次信息，忽略后续
        return;
    }
    guard.order.push(info.name);
    guard.constants.insert(info.name, info);
}

/// 注册一个常量组（由 `RegisterFn` 指定的注册函数）。
///
/// 典型用法：`register_group(nuzo_core::constants::__register_constants);`
pub fn register_group(register_fn: RegisterFn) {
    register_fn();
}

/// 初始化多个常量组。
///
/// 通常在程序启动时调用一次，注册所有已知模块的常量。
pub fn init(groups: &[RegisterFn]) {
    for g in groups {
        register_group(*g);
    }
}

/// 按名称查询常量元数据。
///
/// 返回常量元数据的**拷贝**（`ConstantInfo: Copy`，全 `&'static str` 字段，
/// 拷贝开销可忽略）。按值返回避免了把 `Mutex` guard 内的引用延长到锁外，
/// 从根上消除了悬垂引用风险（即使 `clear()` 被调用也安全）。
pub fn get(name: &str) -> Option<ConstantInfo> {
    let guard = lock_registry();
    guard.constants.get(name).copied()
}

/// 返回所有已注册常量（按注册顺序）。
pub fn all() -> Vec<ConstantInfo> {
    let guard = lock_registry();
    guard.order.iter().filter_map(|name| guard.constants.get(name).copied()).collect()
}

/// 返回已注册常量的数量。
pub fn count() -> usize {
    let guard = lock_registry();
    guard.constants.len()
}

/// 判断指定名称的常量是否已注册。
pub fn exists(name: &str) -> bool {
    let guard = lock_registry();
    guard.constants.contains_key(name)
}

/// 按模块路径过滤常量。
///
/// `module_path` 应为 `module_path!()` 的结果，如 `"nuzo_core::constants"`。
pub fn by_module(module_path: &str) -> Vec<ConstantInfo> {
    let guard = lock_registry();
    guard
        .order
        .iter()
        .filter_map(|name| {
            guard.constants.get(name).filter(|info| info.module_path == module_path).copied()
        })
        .collect()
}

/// 按类型名过滤常量。
///
/// `type_name` 应为 Rust 类型名，如 `"usize"`、`"f64"`、`"&str"`。
pub fn by_type(type_name: &str) -> Vec<ConstantInfo> {
    let guard = lock_registry();
    guard
        .order
        .iter()
        .filter_map(|name| {
            guard.constants.get(name).filter(|info| info.type_name == type_name).copied()
        })
        .collect()
}

/// 清空注册表（仅测试用）。
///
/// # 安全性
///
/// 此函数会清空所有已注册常量，仅应在测试代码中调用。
/// 生产代码调用会导致 `get` / `all` 等函数返回空结果。
#[cfg(feature = "test-utils")]
pub fn clear() {
    let mut guard = lock_registry();
    guard.constants.clear();
    guard.order.clear();
}
