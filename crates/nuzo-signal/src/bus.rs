//! # 事件总线模块（作用域限定发布-订阅中心）
//!
//! 本模块实现了**作用域限定的信号注册表**，提供类型安全的信号查找和生命周期管理。
//!
//! ## 架构定位
//!
//! `SignalBus` 是观察者模式中的**中介者（Mediator）**或**服务定位器（Service Locator）**：
//! - 解耦信号的创建者和使用者
//! - 按作用域隔离不同子系统的信号，避免名称冲突
//! - 保证每个信号名称的唯一性和类型一致性
//!
//! ## 设计模式组合
//!
//! ### 1. 发布-订阅（Publish-Subscribe）
//! 信号的生产者（发射方）和消费者（槽位方）通过总线解耦：
//! ```text
//! 生产者 ──▶ [SignalBus] ◀── 消费者
//!              │
//!         信号注册表
//!         (SignalKey → Signal)
//! ```
//!
//! ### 2. 类型安全的服务定位
//! 通过 `SignalKey<Args>` 将信号名称、作用域和参数类型绑定为一个编译期可验证的整体，
//! 在**运行时**保证类型安全：尝试用错误的类型参数查找信号会返回明确的错误。
//!
//! ### 3. 作用域限定模式
//! 每条总线通过 `BusScope` 标识所属子系统，避免不同模块间的信号名冲突。
//! 不再提供全局单例，由调用方按需创建和持有总线实例。
//!
//! ## 线程安全性模型
//!
//! ```text
//! +------------------------------------------------------+
//! |                   SignalBus                            |
//! |                                                       |
//! |  +----------------------+  +----------------------+     |
//! |  | signals: RwLock     |  | names: RwLock       |     |
//! |  | <HashMap>           |  | <Vec<&str>>         |     |
//! |  +----------+-----------+  +----------+-----------+     |
//! |             |                        |                  |
//! |  +----------v-----------+  +---------v-----------+      |
//! |  | 并发策略             |  | 并发策略              |     |
//! |  |                      |  |                       |     |
//! |  | register(): 写锁     |  | register(): 写锁      |     |
//! |  | get(): 读锁          |  | list_signals(): 读锁   |     |
//! |  | clear(): 写锁        |  | clear(): 写锁         |     |
//! |  +----------------------+  +----------------------+     |
//! |                                                       |
//! |  注意: 锁顺序规则:                                    |
//! |  必须先获取 signals 锁，再获取 names 锁               |
//! |  (禁止反向顺序，防止死锁)                              |
//! +------------------------------------------------------+
//! ```
//!
//! ## 使用场景
//!
//! ### 典型工作流
//! ```ignore
//! use nuzo_signal::{SignalBus, BusScope, SignalKey, Signal};
//!
//! // 1. 创建作用域限定的总线
//! let bus = SignalBus::scoped(BusScope::Gc);
//!
//! // 2. 初始化阶段：注册信号（使用类型化 SignalKey）
//! let key: SignalKey<GcWillCollectInfo> = SignalKey::new("gc:will_collect", BusScope::Gc);
//! let signal = Signal::<GcWillCollectInfo>::named("gc:will_collect");
//! bus.register(&key, &signal)?;
//!
//! // 3. 运行时：通过类型化键查找信号
//! let signal = bus.get(&key)?;
//! signal.connect(|event| handle_gc_event(event));
//!
//! // 4. 运行时：生产者发射信号
//! let signal = bus.get(&key)?;
//! signal.emit(&gc_event);
//! ```
//!
//! ## 性能特征
//!
//! | 操作     | 时间复杂度 | 锁竞争 | 内存分配 |
//! |----------|-----------|--------|----------|
//! | register | O(1)*     | 写锁   | 1 次     |
//! | get      | O(1)      | 读锁   | 0 次     |
//! | list     | O(n)      | 读锁   | 1 次     |
//! | clear    | O(n)      | 写锁   | 0 次     |
//!
//! > *amortized（HashMap 均摊）

use nuzo_core::{XxHashMap, xx_hash_map_new};
use std::any::{Any, TypeId};
use std::sync::RwLock;

use crate::error::SignalError;
use crate::signal::Signal;
use crate::types::BusScope;

/// 类型擦除的信号存储类型（简化 clippy type_complexity 警告）
type SignalRegistry = RwLock<XxHashMap<(&'static str, TypeId), Box<dyn Any + Send + Sync>>>;

/// 作用域限定的事件总线（信号注册表）
///
/// # 核心职责
///
/// ## 1. 信号注册（register）
/// 将信号实例存储到注册表中，通过 `SignalKey<Args>` 建立类型安全的名称映射。
/// 注册后，任何代码都可以通过相同的键查找并使用该信号。
///
/// ## 2. 信号查找（get）
/// 根据 `SignalKey<Args>` 从注册表中检索信号。
/// 返回的是信号的克隆句柄（共享底层状态），可直接用于 connect/emit。
///
/// ## 3. 生命周期管理（clear）
/// 提供清理所有已注册信号的能力，主要用于测试和模块卸载。
///
/// # 内部数据结构
///
/// ## signals HashMap
/// 存储实际的信号对象，使用 `Box<dyn Any + Send + Sync>` 进行类型擦除：
/// - **为什么用 Any？** 因为 HashMap 需要统一的值类型
/// - **如何恢复类型？** 通过 `downcast_ref::<Signal<Args>>()` 在运行时检查类型
/// - **安全性？** 如果类型不匹配，返回 `TypeMismatch` 错误而非 panic
///
/// ## names Vec
/// 维护所有已注册信号名称的列表，用于快速列举：
/// - **为什么单独维护？** 避免 HashMap keys() 的迭代开销
/// - **一致性保证？** 在 register 时同步更新，clear 时同步清空
///
/// # 死锁预防
///
/// 本类持有两把锁（signals 和 names），必须严格遵守加锁顺序：
/// 1. **先 signals 后 names**（register, clear）
/// 2. **只读其中一把**（get 只读 signals，list 只读 names）
///
/// **禁止**在任何地方以相反顺序获取这两把锁！
pub struct SignalBus {
    /// 总线作用域标识
    scope: BusScope,

    /// 信号注册表（name+TypeId → 信号实例）
    ///
    /// 使用 RwLock 保护，支持：
    /// - 多个并发 reader（get 操作）
    /// - 独占 writer（register/clear 操作）
    signals: SignalRegistry,

    /// 信号名称列表（用于快速列举）
    ///
    /// 与 signals HashMap 保持同步：
    /// - register 时追加（如果名称不存在）
    /// - clear 时清空
    names: RwLock<Vec<&'static str>>,
}

impl SignalBus {
    /// 创建指定作用域的事件总线实例
    ///
    /// # 参数
    /// `scope`: 总线的作用域标识，用于隔离不同子系统的信号
    ///
    /// # 返回值
    /// 指定作用域的空信号注册表（无已注册信号）
    ///
    /// # 使用场景
    /// 各子系统应创建自己作用域的总线，避免信号名冲突：
    /// - `BusScope::Gc` — 垃圾回收信号
    /// - `BusScope::Compiler` — 编译器信号
    /// - `BusScope::Builtin` — 内置函数信号
    /// - `BusScope::Custom("plugin:x")` — 插件自定义信号
    ///
    /// # 示例
    /// ```ignore
    /// let gc_bus = SignalBus::scoped(BusScope::Gc);
    /// let compiler_bus = SignalBus::scoped(BusScope::Compiler);
    /// ```
    pub fn scoped(scope: BusScope) -> Self {
        Self { scope, signals: RwLock::new(xx_hash_map_new()), names: RwLock::new(Vec::new()) }
    }

    /// 获取总线的作用域标识
    ///
    /// # 返回值
    /// 创建时指定的 `BusScope`
    pub fn scope(&self) -> BusScope {
        self.scope
    }

    /// 创建新的事件总线实例（已废弃，使用 `scoped` 替代）
    ///
    /// # 废弃原因
    /// 无作用域限定的总线容易导致信号名冲突，应使用 `SignalBus::scoped(BusScope::X)`
    /// 明确指定作用域。
    #[deprecated(since = "0.2.0", note = "使用 SignalBus::scoped(BusScope::X) 替代")]
    #[allow(clippy::new_without_default)] // new() 已废弃，无需添加 Default impl
    pub fn new() -> Self {
        Self::scoped(BusScope::Custom("legacy"))
    }

    /// 通过类型化信号键注册信号到总线
    ///
    /// # 参数
    /// - `key`: 类型化信号键，携带名称和作用域信息
    /// - `signal`: 要注册的信号引用（不会被消费，内部会 clone）
    ///
    /// # 返回值
    /// - `Ok(())`: 注册成功
    /// - `Err(SignalError::AlreadyRegistered)`: 相同名称+类型的信号已存在
    /// - `Err(SignalError::LockPoisoned)`: 内部锁被污染
    ///
    /// # 类型安全机制
    /// 使用 `(key.name(), TypeId::of::<Args>())` 作为复合键：
    /// - 同名不同类型：允许（不同的 TypeId）
    /// - 同名同类型：拒绝（AlreadyRegistered）
    ///
    /// # Clone 行为
    /// 内部调用 `signal.clone_handle()` 创建共享状态的副本存储。
    /// 原始信号仍可正常使用，与总线中的版本完全同步。
    ///
    /// # 示例
    /// ```ignore
    /// let key: SignalKey<GcWillCollectInfo> = SignalKey::new("gc:will_collect", BusScope::Gc);
    /// let signal = Signal::<GcWillCollectInfo>::named("gc:will_collect");
    /// bus.register(&key, &signal)?;
    /// ```
    pub fn register<Args: 'static + Send + Sync>(
        &self,
        key: &crate::types::SignalKey<Args>,
        signal: &Signal<Args>,
    ) -> Result<(), SignalError> {
        let internal_key = (key.name(), TypeId::of::<Args>());

        // 获取写锁以修改 HashMap
        let mut signals =
            self.signals.write().map_err(|_| SignalError::LockPoisoned { name: key.name() })?;

        // 检查是否已注册（防止重复）
        if signals.contains_key(&internal_key) {
            return Err(SignalError::AlreadyRegistered { name: key.name() });
        }

        // 插入克隆后的信号（clone_handle 共享底层状态）
        signals.insert(internal_key, Box::new(signal.clone_handle()));

        // 同步更新名称列表（如果名称不存在）
        let mut names =
            self.names.write().map_err(|_| SignalError::LockPoisoned { name: key.name() })?;
        if !names.contains(&key.name()) {
            names.push(key.name());
        }

        Ok(())
    }

    /// 通过类型化信号键查找信号（已废弃，仅接受 signal 参数）
    ///
    /// # 废弃原因
    /// 缺少类型化的 `SignalKey` 参数，无法在编译期绑定名称与类型。
    /// 使用 `bus.register(&SignalKey, &signal)` 替代。
    #[deprecated(since = "0.2.0", note = "使用 bus.register(&SignalKey, &signal) 替代")]
    pub fn register_legacy<Args: 'static + Send + Sync>(
        &self,
        signal: &Signal<Args>,
    ) -> Result<(), SignalError> {
        let key = (signal.name(), TypeId::of::<Args>());

        let mut signals =
            self.signals.write().map_err(|_| SignalError::LockPoisoned { name: signal.name() })?;

        if signals.contains_key(&key) {
            return Err(SignalError::AlreadyRegistered { name: signal.name() });
        }

        signals.insert(key, Box::new(signal.clone_handle()));

        let mut names =
            self.names.write().map_err(|_| SignalError::LockPoisoned { name: signal.name() })?;
        if !names.contains(&signal.name()) {
            names.push(signal.name());
        }

        Ok(())
    }

    /// 通过类型化信号键查找信号
    ///
    /// # 参数
    /// `key`: 类型化信号键，携带名称和泛型参数
    ///
    /// # 返回值
    /// - `Ok(Signal<Args>)`: 找到的信号句柄（可立即用于 connect/emit）
    /// - `Err(SignalError::NotFound)`: 该名称的信号不存在
    /// - `Err(SignalError::TypeMismatch)`: 名称存在但类型不匹配
    /// - `Err(SignalError::LockPoisoned)`: 内部锁被污染
    ///
    /// # 两阶段查找算法
    /// 为了提供精确的错误信息，采用两阶段查找：
    ///
    /// ## 阶段 1：按复合键查找
    /// 尝试 `(key.name(), TypeId::of::<Args>())` 精确匹配
    /// - 成功：执行 downcast 并返回
    /// - 失败：进入阶段 2
    ///
    /// ## 阶段 2：按名称模糊匹配
    /// 检查该名称是否存在于 names 列表中：
    /// - 存在：说明类型参数错误 → 返回 `TypeMismatch`
    /// - 不存在：说明信号未注册 → 返回 `NotFound`
    ///
    /// # 示例
    /// ```ignore
    /// let key: SignalKey<GcWillCollectInfo> = SignalKey::new("gc:will_collect", BusScope::Gc);
    /// let signal = bus.get(&key)?;
    /// signal.emit(&event);
    /// ```
    pub fn get<Args: 'static + Send + Sync>(
        &self,
        key: &crate::types::SignalKey<Args>,
    ) -> Result<Signal<Args>, SignalError> {
        let internal_key = (key.name(), TypeId::of::<Args>());

        // 获取读锁进行查找
        let signals =
            self.signals.read().map_err(|_| SignalError::LockPoisoned { name: key.name() })?;

        match signals.get(&internal_key) {
            Some(boxed) => {
                // 找到了：尝试 downcast 到具体类型
                boxed
                    .downcast_ref::<Signal<Args>>()
                    .cloned() // 克隆 Signal（浅拷贝）
                    .ok_or_else(|| SignalError::TypeMismatch {
                        name: key.name(),
                        expected: std::any::type_name::<Args>(),
                        actual: "unknown",
                    })
            }
            None => {
                // 未找到：区分"未注册"和"类型错误"
                let names = self
                    .names
                    .read()
                    .map_err(|_| SignalError::LockPoisoned { name: key.name() })?;

                if names.contains(&key.name()) {
                    // 名称存在但类型不匹配
                    Err(SignalError::TypeMismatch {
                        name: key.name(),
                        expected: std::any::type_name::<Args>(),
                        actual: "different type",
                    })
                } else {
                    // 名称不存在（确实未注册）
                    Err(SignalError::NotFound { name: key.name() })
                }
            }
        }
    }

    /// 列举所有已注册的信号名称
    ///
    /// # 返回值
    /// 包含所有已注册信号名称的向量（按注册顺序排列）
    ///
    /// # 使用场景
    /// - **调试/诊断**：打印系统中所有可用的信号
    /// - **动态发现**：插件系统根据可用信号自动连接
    /// - **文档生成**：提取信号清单用于 API 文档
    /// - **健康检查**：验证预期的信号是否已正确注册
    ///
    /// # 性能注意
    /// 此方法会克隆整个名称列表（Vec 的 clone 是 O(n) 的深拷贝指针数组）。
    /// 对于大量信号（>1000），可能需要考虑缓存或迭代器接口。
    ///
    /// # 示例
    /// ```ignore
    /// let signals = bus.list_signals();
    /// for name in &signals {
    ///     println!("- {}", name);
    /// }
    /// // 输出:
    /// // - gc:will_collect
    /// // - gc:did_collect
    /// // - compiler:started
    /// ```
    pub fn list_signals(&self) -> Vec<&'static str> {
        // 必须拷贝：数据在 RwLock 后面，无法返回引用切片。
        // &'static str 是 Copy 类型，用 iter().copied().collect() 比 .clone() 更显式。
        self.names.read().map(|guard| guard.iter().copied().collect()).unwrap_or_default()
    }

    /// 清空所有已注册的信号
    ///
    /// # 效果
    /// 移除所有信号和名称记录，将总线重置为初始空状态。
    /// 已有的 Signal 句柄（之前通过 get 获得的）仍然有效，
    /// 但它们不再出现在总线的查找结果中。
    ///
    /// # 使用场景
    /// - **测试 teardown**：每个测试用例结束后清理总线状态
    /// - **模块卸载**：移除某个功能模块的所有信号
    /// - **热重载**：重新加载配置前清空旧信号
    ///
    /// # 注意事项
    /// 此操作不可逆。所有信号都需要重新注册。
    /// 不影响已有连接：已经连接到信号上的槽位不会受影响。
    ///
    /// # 线程安全
    /// 分别获取两把写锁，顺序为 signals -> names（符合死锁预防规则）。
    /// 在持锁期间，所有其他操作都会被阻塞。
    pub fn clear(&self) {
        // 清空信号注册表
        if let Ok(mut signals) = self.signals.write() {
            signals.clear();
        }
        // 清空名称列表
        if let Ok(mut names) = self.names.write() {
            names.clear();
        }
    }
}
