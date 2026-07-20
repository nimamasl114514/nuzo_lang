//! # 硬编码常量管理框架
//!
//! 本模块提供 [`define_constants!`] 宏，用于集中管理 Nuzo 运行时的所有硬编码常量。
//!
//! ## 设计目标
//!
//! - **编译期零开销**：真正的 `const` 常量，无运行时查找开销
//! - **运行时可查询**：自动注册到全局注册表，支持自省（introspection）
//! - **环境变量覆盖**：支持通过 `NUZO_*` 环境变量在运行时覆盖默认值
//! - **JSON 导出**：支持将所有常量导出为 JSON 格式
//! - **校验规则**：支持注册自定义校验逻辑
//!
//! ## 子模块
//!
//! | 模块 | 功能 | Feature |
//! |------|------|---------|
//! | [`types`] | `ConstantInfo` 元数据类型 | 始终可用 |
//! | [`registry`] | 全局常量注册表 | 始终可用 |
//! | [`env`] | 环境变量覆盖机制 | `env-override` |
//! | [`export`] | JSON 导出 | `json-export` |
//! | [`validate`] | 校验规则引擎 | `env-override` |
//!
//! ## 示例
//!
//! ```no_run
//! use nuzo_proc_core::define_constants;
//!
//! define_constants! {
//!     /// 默认最大栈大小
//!     pub DEFAULT_MAX_STACK_SIZE: usize = 65536;
//!
//!     /// GC 默认阈值（10MB）
//!     pub GC_DEFAULT_THRESHOLD: usize = 10 * 1024 * 1024;
//! }
//! ```
//!
//! 展开后会生成：
//!
//! - `pub const DEFAULT_MAX_STACK_SIZE: usize = 65536;`
//! - `pub const GC_DEFAULT_THRESHOLD: usize = 10 * 1024 * 1024;`
//! - 一个 `__register_constants()` 函数，将常量元数据注册到全局注册表

// 子模块声明
pub mod registry;
pub mod types;

#[cfg(feature = "env-override")]
pub mod env;

#[cfg(feature = "json-export")]
pub mod export;

#[cfg(feature = "env-override")]
pub mod validate;

// 重新导出常用类型，便于外部使用
pub use types::ConstantInfo;

/// 定义编译期常量并注册到全局注册表。
///
/// # 语法
///
/// ```ignore
/// define_constants! {
///     $(#[$meta:meta])*
///     $vis:vis $name:ident : $ty:ty = $value:expr;
///     ...
/// }
/// ```
///
/// # 展开结果
///
/// 对每个常量定义，宏会生成：
///
/// 1. `$(#[$meta])* $vis const $name: $ty = $value;`
///    —— 真正的编译期常量，零开销访问
///
/// 2. 一个 `__register_constants()` 函数，调用
///    `$crate::hardcode::registry::register(...)` 注册常量元数据
///    （名称、类型、值字符串、模块路径）
///
/// 3. 一个 `#[ctor::ctor]` 标记的 `__auto_register_constants()` 函数，
///    在程序启动时自动调用 `__register_constants()`，无需手动注册
///
/// # 示例
///
/// ```no_run
/// use nuzo_proc_core::define_constants;
///
/// define_constants! {
///     /// 默认最大栈大小
///     pub DEFAULT_MAX_STACK_SIZE: usize = 65536;
///
///     /// GC 默认阈值
///     pub GC_DEFAULT_THRESHOLD: usize = 10 * 1024 * 1024;
///
///     /// 默认源文件名
///     pub DEFAULT_SOURCE_FILE: &str = "<source>";
/// }
/// ```
///
/// # 自动注册
///
/// 宏使用 `ctor` crate 在程序启动时自动调用 `__register_constants()`，
/// 将常量元数据注册到全局注册表。用户无需手动调用任何初始化函数。
///
/// 注册后可通过 [`registry`](crate::hardcode::registry) 模块查询：
///
/// ```no_run
/// # use nuzo_proc_core::define_constants;
/// # define_constants! { pub FOO: usize = 42; }
/// use nuzo_proc_core::hardcode::registry;
///
/// // 程序启动后，注册表自动包含所有 define_constants! 定义的常量
/// if let Some(info) = registry::get("FOO") {
///     println!("{} = {}", info.name, info.value_str);
/// }
/// ```
#[macro_export]
macro_rules! define_constants {
    // 入口规则：处理多个常量定义
    (
        $(
            $(#[$meta:meta])*
            $vis:vis $name:ident : $ty:ty = $value:expr;
        )*
    ) => {
        // 1. 生成 const 定义（保留文档注释与可见性）
        $(
            $(#[$meta])*
            $vis const $name: $ty = $value;
        )*

        // 2. 生成注册函数
        // 注意：使用 $crate 确保宏在外部 crate 中也能正确解析路径
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::needless_pass_by_value)]
        pub fn __register_constants() {
            $(
                $crate::hardcode::registry::register(
                    $crate::hardcode::types::ConstantInfo {
                        name: stringify!($name),
                        type_name: stringify!($ty),
                        value_str: stringify!($value),
                        doc: "",
                        module_path: module_path!(),
                    }
                );
            )*
        }

        // 3. 自动注册：使用 ctor 在程序启动时调用 __register_constants()
        //    通过 nuzo_proc_core::ctor 路径引用 ctor crate（nuzo_proc_core 重导出了 ctor）
        //    这样调用方 crate 无需显式依赖 ctor
        //    注意：属性路径不能使用 $crate，必须使用字面量 crate 名
        //
        //    wasm32 target：ctor 链接器段不被支持，跳过自动注册。
        //    const 定义仍可直接访问，仅 registry 在 wasm32 下为空。
        //    如需在 wasm32 下查询常量元数据，需显式调用 __register_constants()。
        #[cfg(not(target_arch = "wasm32"))]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        #[nuzo_proc_core::ctor::ctor]
        fn __auto_register_constants() {
            __register_constants();
        }
    };
}
