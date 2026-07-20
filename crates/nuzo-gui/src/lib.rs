//! # Nuzo GUI — egui 标准库封装
//!
//! 提供易语言式简洁 GUI API，中英双语支持。

#![allow(clippy::result_large_err)]

pub mod context;
pub mod layout;
pub mod theme;
pub mod widgets;

use nuzo_helpers::BuiltinRegistry;

/// Register all GUI builtins into the registry (widgets + layout + theme).
pub fn register_all(registry: &mut BuiltinRegistry) {
    widgets::register_widgets(registry);
    layout::register_layout(registry);
    theme::register_theme(registry);
}
