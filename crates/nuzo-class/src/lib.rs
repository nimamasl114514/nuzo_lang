//! # nuzo_class — 面向 Nuzo Lang 的 Rust 侧类语法糖
//!
//! **层级**: L1（虚拟机层 / 宿主集成层）—— 提供零运行时成本的属性宏，让 Rust 结构体按 Nuzo 类约定生成构造器、getter、setter、实例方法与序列化支持。
//!
//! **主要入口**: [`class`], [`class_impl`], [`constructor`], [`get`], [`set`], [`method`], [`static_method`]
//!
//! `nuzo_class` 本身不包含运行时逻辑，它只是对 [`nuzo_class_macros`]
//! 中过程宏的重新导出。这些宏会在编译期校验类、构造器、getter、
//! setter、实例方法以及静态方法是否满足约定，并原样保留你的实现，
//! 因此是**零运行时成本**的语法糖。
//!
//! 目前支持的属性宏：
//!
//! - `#[class]` — 标记一个 `struct` 为类，并可选地追加 `Debug` /
//!   `Default` / `Clone` 派生。
//! - `#[class_impl]` — 标记一个 `impl` 块，由宏统一处理其中的
//!   `#[constructor]`、`#[get]`、`#[set]`、`#[method]`、`#[static_method]`。
//! - `#[constructor]` — 构造器，返回类型必须是 `Self`。
//! - `#[get]` — getter，第一个参数必须是 `&self`，且必须有返回值。
//! - `#[set]` — setter，第一个参数必须是 `&mut self`，且必须返回 `()`。
//! - `#[method]` — 普通实例方法，第一个参数必须是 `&self` 或 `&mut self`。
//! - `#[static_method]` — 静态方法，不能接收 `self`。
//!
//! # 自动 Serde 支持
//!
//! 当启用 `serde` feature 后，`#[class_impl]` 可自动生成
//! `serde::Serialize` / `serde::Deserialize` 实现：
//!
//! - `#[class_impl(serialize)]` — 收集所有 `#[get]` 方法，自动生成
//!   `Serialize` impl（序列化时调用 getter）。
//! - `#[class_impl(deserialize)]` — 收集 `#[constructor` 的参数和 `#[set]`
//!   方法，自动生成 `Deserialize` impl（反序列化时调用构造器 + setter）。
//!
//! ```rust
//! use nuzo_class::{class, class_impl, constructor, get, method, set, static_method};
//!
//! #[class(debug, default, clone)]
//! struct Person {
//!     name: String,
//!     age: u32,
//! }
//!
//! #[class_impl(serialize, deserialize)]
//! impl Person {
//!     #[constructor]
//!     fn new(name: String, age: u32) -> Self {
//!         Self { name, age }
//!     }
//!
//!     #[get]
//!     fn name(&self) -> &str {
//!         &self.name
//!     }
//!
//!     #[get]
//!     fn age(&self) -> u32 {
//!         self.age
//!     }
//!
//!     #[set]
//!     fn set_age(&mut self, age: u32) {
//!         self.age = age;
//!     }
//!
//!     #[method]
//!     fn greet(&self) -> String {
//!         format!(
//!             "Hello, my name is {} and I am {} years old.",
//!             self.name, self.age
//!         )
//!     }
//!
//!     #[static_method]
//!     fn species() -> &'static str {
//!         "Homo sapiens"
//!     }
//! }
//!
//! fn main() {
//!     let mut person = Person::new("Alice".to_string(), 30);
//!     assert_eq!(person.name(), "Alice");
//!     assert_eq!(person.age(), 30);
//!
//!     person.set_age(31);
//!     assert_eq!(person.age(), 31);
//!
//!     assert!(person.greet().contains("Alice"));
//!     assert_eq!(Person::species(), "Homo sapiens");
//! }
//! ```

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(layer = 1, description = "原型 OOP 类系统", entry_type = "ProcMacro")]
const _NUZO_CRATE_META_ANCHOR: () = ();

pub use nuzo_class_macros::{class, class_impl, constructor, get, method, set, static_method};
