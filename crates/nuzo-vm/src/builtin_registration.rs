//! # Builtin 函数注册 — VM 内置函数绑定
//!
//! 负责将 nuzo_helpers::BuiltinRegistry 中的内置函数绑定到 VM 的执行上下文。
//! 通过 register_builtins() 统一注册所有 domain 的 builtin。
//!
//! ## 公开 API
//!
//! - `VM::register_builtins(&mut self)` — 注册入口
//! - `VM::register_builtins_from(&mut self, &BuiltinRegistry)` — 注册外部 builtin（如 GUI）

use super::VM;
use nuzo_core::Value;
use nuzo_helpers::BuiltinRegistry;
use nuzo_values::HeapObject;
use nuzo_values::heap::BuiltinFnPtr;

impl VM {
    /// 注册所有内置函数到全局作用域。
    pub(super) fn register_builtins(&mut self) {
        let registry = BuiltinRegistry::new();
        self.register_builtins_from(&registry);
    }

    /// 注册外部 BuiltinRegistry 中的所有函数到全局作用域。
    ///
    /// 用于 nuzo_gui 等外部 crate 将自己的 builtin 函数注入 VM，
    /// 无需修改 nuzo_helpers 或 nuzo_vm 的代码。
    ///
    /// # 示例
    ///
    /// ```rust,ignore
    /// let mut registry = BuiltinRegistry::new();
    /// nuzo_gui::register_all(&mut registry);
    /// vm.register_builtins_from(&registry);
    /// ```
    pub fn register_builtins_from(&mut self, registry: &BuiltinRegistry) {
        for name in registry.names() {
            if let Some(func) = registry.get(name) {
                let arity: usize = registry.get_arity(name).unwrap_or(0);
                let builtin = HeapObject::BuiltinFn {
                    name: name.to_string(),
                    arity,
                    func: func as BuiltinFnPtr,
                };
                let idx = self.gc.alloc_with_size(builtin, 0);
                let value = Value::from_gc_index(idx);
                self.cx.global_scope.define(name, value);
            }
        }
    }
}
