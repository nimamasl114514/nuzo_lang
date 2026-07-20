//! Layout management (rows, columns, panels, etc.)
//!
//! ## v1 Status (P2-4 fix)
//!
//! egui's `ui.horizontal(|ui| { ... })` is closure-based, but nuzo builtins
//! cannot inject closure bodies today. Therefore v1 implements:
//!
//! - `row()` / `col()`: **no-op** (default vertical layout is sufficient).
//!   These do NOT push to `LAYOUT_STACK` — the previous implementation pushed
//!   on every call without a corresponding pop, causing unbounded stack growth
//!   from script bugs (e.g., calling `row()` in a loop). The audit flagged
//!   this as P2-4.
//! - `collapse(title)`: uses egui `CollapsingHeader` with placeholder content.
//!   Self-contained — does not touch `LAYOUT_STACK`.
//! - `end()`: **no-op** in v1. Reserved for v2 callback-based layout, where
//!   it will pop the layout stack to close the current container. Scripts
//!   written for v1 can already pair `row()`/`end()` for forward compatibility.
//!
//! `LAYOUT_STACK` and `UiStackItem` are retained (empty) for v2. When v2
//! adds callback support, `row()`/`col()` will push and `end()` will pop,
//! bounded by [`MAX_LAYOUT_STACK_DEPTH`] to prevent runaway nesting.

use std::cell::RefCell;

use nuzo_core::Value;
use nuzo_helpers::BuiltinRegistry;
use nuzo_values::{NIL, NuzoError, ValueExt};

// Thread-local layout stack (reserved for v2)
/// Maximum nesting depth for layout containers. Reserved for v2
/// callback-based layout to prevent unbounded `LAYOUT_STACK` growth.
///
/// In v1 this is unused (the stack is always empty) but kept here so v2 can
/// enforce the bound from day one.
#[allow(dead_code)] // Used by v2 callback-based layout and by tests
const MAX_LAYOUT_STACK_DEPTH: usize = 64;

/// Layout type pushed onto the stack (v2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Used by v2 callback-based layout (not yet implemented)
enum LayoutType {
    Horizontal,
    Vertical,
    Collapsing,
}

/// Stack item saved when entering a nested layout container (v2).
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used by v2 callback-based layout (not yet implemented)
struct UiStackItem {
    layout_type: LayoutType,
}

thread_local! {
    /// Stack of nested layout containers.
    ///
    /// **v1**: always empty. `row()`/`col()` are no-ops and do not push.
    /// **v2**: will be pushed by `row()`/`col()` and popped by `end()`,
    /// bounded by [`MAX_LAYOUT_STACK_DEPTH`].
    static LAYOUT_STACK: RefCell<Vec<UiStackItem>> = const { RefCell::new(Vec::new()) };
}

/// `row()` — start a horizontal layout row.
///
/// **v1: no-op.** egui's `ui.horizontal()` is closure-based; we cannot
/// inject nuzo-side widget calls into the closure body yet. Default vertical
/// layout handles most use cases without this builtin.
///
/// **v2 plan**: will push a `Horizontal` item onto `LAYOUT_STACK` and create
/// a scoped `ui.horizontal()` closure. Must be paired with [`gui_end`].
pub fn gui_row(_args: &[Value]) -> Result<Value, NuzoError> {
    // v1: intentionally no-op. Do NOT push to LAYOUT_STACK — the previous
    // implementation pushed without a matching pop, causing unbounded growth
    // (audit P2-4). v2 will add push/pop with MAX_LAYOUT_STACK_DEPTH bound.
    Ok(NIL)
}

/// `col()` — start a vertical layout column.
///
/// **v1: no-op** (same reasoning as [`gui_row`]).
///
/// **v2 plan**: will push a `Vertical` item and create a scoped closure.
/// Must be paired with [`gui_end`].
pub fn gui_col(_args: &[Value]) -> Result<Value, NuzoError> {
    // v1: intentionally no-op. See `gui_row` for rationale.
    Ok(NIL)
}

/// `end()` — end the current layout container.
///
/// **v1: no-op.** Since `row()`/`col()` do not push in v1, there is nothing
/// to pop. Provided for forward compatibility — scripts that pair
/// `row()`/`end()` will work unchanged in v2.
///
/// **v2 plan**: will pop the most recent item from `LAYOUT_STACK`. If the
/// stack is empty, this is a no-op (returns nil).
pub fn gui_end(_args: &[Value]) -> Result<Value, NuzoError> {
    // v1: no-op. v2 will pop LAYOUT_STACK (with empty-stack guard).
    Ok(NIL)
}

/// `collapse(title)` — create a collapsible section.
///
/// Uses egui `CollapsingHeader`. In v1 the body is a placeholder label
/// "(content)"; full content injection requires callback support (v2).
///
/// This function is self-contained: it does not push to `LAYOUT_STACK`
/// (the collapse body is rendered immediately within the `collapsing`
/// closure, not via a separate `end()` call).
pub fn gui_collapse(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    let title = args[0].as_string_opt().unwrap_or_default();

    // No LAYOUT_STACK push — collapse is self-contained in v1.
    crate::context::with_ui(|ui| {
        ui.collapsing(title, |ui| {
            ui.label("(content)");
        });
    });

    Ok(NIL)
}

/// Register all layout builtins into the registry.
pub fn register_layout(registry: &mut BuiltinRegistry) {
    registry.register("行", gui_row, 0);
    registry.register("row", gui_row, 0);
    registry.register("列", gui_col, 0);
    registry.register("col", gui_col, 0);
    registry.register("结束", gui_end, 0);
    registry.register("end", gui_end, 0);
    registry.register("折叠", gui_collapse, 1);
    registry.register("collapse", gui_collapse, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P2-4 regression: `gui_row`/`gui_col` must NOT cause unbounded growth
    /// of `LAYOUT_STACK`. In v1 they are no-ops; the stack remains empty
    /// even after many calls.
    ///
    /// Before the fix, each call pushed an item without a matching pop,
    /// so a loop calling `row()` would grow the stack without bound.
    #[test]
    fn test_layout_stack_does_not_grow() {
        // Clear any leftover state from previous tests.
        LAYOUT_STACK.with(|s| s.borrow_mut().clear());

        // Call gui_row and gui_col many times (simulating a script loop).
        for _ in 0..1000 {
            let _ = gui_row(&[]);
            let _ = gui_col(&[]);
        }

        // Stack must still be empty (v1: no push).
        let len = LAYOUT_STACK.with(|s| s.borrow().len());
        assert_eq!(len, 0, "LAYOUT_STACK grew unboundedly — v1 row/col must be no-op");
    }

    /// P2-4 regression: `gui_end` is a safe no-op even when the stack is empty.
    /// It must not panic or corrupt state.
    #[test]
    fn test_layout_end_is_safe_noop() {
        LAYOUT_STACK.with(|s| s.borrow_mut().clear());

        // Call end() many times with an empty stack — must not panic.
        for _ in 0..100 {
            let result = gui_end(&[]);
            assert!(result.is_ok(), "gui_end should always succeed");
        }

        let len = LAYOUT_STACK.with(|s| s.borrow().len());
        assert_eq!(len, 0, "gui_end must not grow the stack");
    }

    /// P2-4 regression: `MAX_LAYOUT_STACK_DEPTH` is defined and reasonable,
    /// so v2 will have a bound from day one.
    #[test]
    fn test_layout_max_depth_constant() {
        // Sanity check: the constant exists and is a sensible bound.
        // Compile-time assertions — if these fail, the crate won't build.
        const _: () = assert!(MAX_LAYOUT_STACK_DEPTH > 0);
        const _: () =
            assert!(MAX_LAYOUT_STACK_DEPTH <= 256, "MAX_LAYOUT_STACK_DEPTH unreasonably large");
    }
}
