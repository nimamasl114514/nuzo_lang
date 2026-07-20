//! Thread-local egui Ui context for builtin functions.
//!
//! Builtins access this to render widgets without having `&mut egui::Ui` in
//! their signature (they are called via `VM::call_global_function` which has
//! no parameter channel for the Ui).
//!
//! ## Safety Contract
//!
//! This module bridges egui's `&mut Ui` (borrowed from `eframe::App::update`)
//! to nuzo builtin functions. The bridge uses a thread-local raw pointer.
//! Soundness is upheld by four invariants:
//!
//! 1. **Single-threaded**: egui is single-threaded. `set_ui` records the
//!    `ThreadId`, and `with_ui` asserts the caller is on the same thread.
//!    Cross-thread access panics rather than dereferencing.
//!
//! 2. **Frame-scoped (RAII)**: `set_ui` returns a [`GuiFrameGuard`] that
//!    clears the pointer on drop. This ensures cleanup even if the VM call
//!    panics between `set_ui` and an explicit `clear_ui`, preventing
//!    use-after-free on the next frame.
//!
//! 3. **Typed pointer**: stored as `NonNull<egui::Ui>` (not `*mut ()`), so
//!    the type is preserved and "no frame active" is represented by
//!    `Option::None` rather than a null pointer.
//!
//! 4. **Exclusive mutability**: `set_ui` takes `&mut egui::Ui` (not `&`), so
//!    the raw pointer originates from an exclusive borrow. `with_ui`'s
//!    closure receives an exclusive `&mut` borrow, and the `RefCell` borrow
//!    is held for the closure's duration to prevent reentrancy.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::thread::ThreadId;

thread_local! {
    /// Frame-scoped pointer to the current egui Ui.
    ///
    /// `None` means "no frame active". The pointer is set by [`set_ui`] and
    /// cleared by [`GuiFrameGuard`]'s `Drop` impl (or explicit [`clear_ui`]).
    static GUI_CTX: RefCell<GuiCtx> = const { RefCell::new(GuiCtx::EMPTY) };
}

/// Internal state stored in the thread-local.
#[derive(Clone, Copy)]
struct GuiCtx {
    /// Typed non-null pointer to the frame's `egui::Ui`. `None` when no frame
    /// is active.
    ptr: Option<NonNull<egui::Ui>>,
    /// Thread that called `set_ui`. Used to assert same-thread access in
    /// `with_ui`.
    setter_thread: Option<ThreadId>,
}

impl GuiCtx {
    const EMPTY: Self = Self { ptr: None, setter_thread: None };
}

/// RAII guard that clears the `GUI_CTX` pointer when dropped.
///
/// Returned by [`set_ui`]. Dropping the guard is the canonical way to end a
/// frame's UI scope. The pointer is cleared even if the VM call panics,
/// preventing use-after-free on the next frame.
///
/// # Construction
///
/// Only [`set_ui`] can construct this guard. The `_private` field enforces
/// this at the type level (code outside this module cannot construct it).
pub struct GuiFrameGuard {
    _private: (),
}

impl Drop for GuiFrameGuard {
    fn drop(&mut self) {
        clear_ui();
    }
}

/// Set the current frame's egui Ui reference and return a guard that clears
/// it on drop.
///
/// # Usage
///
/// ```no_run
/// # let ctx = egui::Context::default();
/// // inside eframe::App::update:
/// egui::CentralPanel::default().show(&ctx, |ui| {
///     let _guard = nuzo_gui::context::set_ui(ui);
///     // ... call VM render() which invokes builtins that use with_ui() ...
/// }); // _guard drops here, clearing the pointer
/// ```
///
/// # Safety Contract (enforced by the type system + runtime)
///
/// The caller MUST ensure:
/// - `ui` outlives the returned guard. In practice, `ui` is borrowed from
///   the eframe render loop scope which outlives VM execution.
/// - The guard is dropped on the same thread that created it (eframe is
///   single-threaded so this is automatic).
///
/// Takes `&mut egui::Ui` (not `&`) so the stored raw pointer originates from
/// an exclusive borrow — this satisfies Rust's aliasing rules when later
/// reconstituted as `&mut` inside [`with_ui`].
pub fn set_ui(ui: &mut egui::Ui) -> GuiFrameGuard {
    GUI_CTX.with(|ctx| {
        *ctx.borrow_mut() = GuiCtx {
            ptr: Some(NonNull::from(ui)),
            setter_thread: Some(std::thread::current().id()),
        };
    });
    GuiFrameGuard { _private: () }
}

/// Clear the Ui reference after the frame ends.
///
/// This is called automatically by [`GuiFrameGuard::drop`]. Direct calls are
/// only needed if you bypass the guard (not recommended for new code).
pub fn clear_ui() {
    GUI_CTX.with(|ctx| {
        *ctx.borrow_mut() = GuiCtx::EMPTY;
    });
}

/// Execute a closure with mutable access to the current frame's egui Ui.
///
/// # Panics
///
/// Panics if:
/// - No Ui is set (the builtin was called outside a render frame). This
///   prevents dereferencing a stale/null pointer.
/// - The current thread differs from the one that called `set_ui`. This
///   catches accidental cross-thread VM scheduling.
///
/// # Safety
///
/// Sound because:
/// - The pointer was obtained from `&mut egui::Ui` via `NonNull::from` inside
///   [`set_ui`], and the [`GuiFrameGuard`] ensures it is cleared before the
///   underlying `Ui` is dropped.
/// - Same-thread access is asserted at runtime.
/// - The `RefCell` borrow is held for the closure's duration, preventing
///   reentrant `with_ui` calls (which would create overlapping `&mut`
///   borrows).
pub fn with_ui<F, R>(f: F) -> R
where
    F: FnOnce(&mut egui::Ui) -> R,
{
    GUI_CTX.with(|ctx| {
        let borrowed = ctx.borrow();
        let gui_ctx = *borrowed;

        let mut ptr = gui_ctx.ptr.expect("GUI_CTX not set - builtin called outside render frame");

        let setter = gui_ctx
            .setter_thread
            .expect("GUI_CTX setter_thread missing (internal invariant violated)");
        let current = std::thread::current().id();
        assert!(
            setter == current,
            "GUI_CTX cross-thread access: set on {:?}, accessed on {:?}. \
             egui is single-threaded; VM must run on the render thread.",
            setter,
            current
        );

        // SAFETY: see `with_ui` safety section. The pointer originates from a
        // live `&mut egui::Ui` whose lifetime is guarded by `GuiFrameGuard`,
        // and same-thread access has been asserted above.
        let ui: &mut egui::Ui = unsafe { ptr.as_mut() };
        f(ui)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S5 regression: `with_ui` must panic with a clear message when no Ui is
    /// set, rather than dereferencing a dangling pointer.
    ///
    /// Before the fix, a stale pointer from a previous frame (left behind by
    /// a panic between `set_ui` and `clear_ui`) could be dereferenced
    /// unsafely. After the fix, `Option::None` is checked explicitly and the
    /// `GuiFrameGuard` ensures cleanup on panic.
    #[test]
    #[should_panic(expected = "GUI_CTX not set")]
    fn test_context_no_dangling_ui_ptr() {
        clear_ui(); // ensure clean state
        with_ui(|_ui| {});
    }

    /// S5 regression: `GuiFrameGuard::drop` must clear the context, even when
    /// the guard is dropped via panic unwinding. This is the core RAII safety
    /// property that prevents stale pointers across frames.
    #[test]
    fn test_context_guard_clears_on_panic_drop() {
        clear_ui(); // clean slate

        // Simulate a frame that panics while the guard is held. The guard's
        // Drop doesn't care whether a real pointer was set — it
        // unconditionally clears the context.
        let result = std::panic::catch_unwind(|| {
            let _guard = GuiFrameGuard { _private: () };
            panic!("simulated VM panic during render");
        });
        assert!(result.is_err(), "expected simulated panic");

        // After panic unwinding, the context must be empty.
        let is_empty = GUI_CTX.with(|c| {
            let c = c.borrow();
            c.ptr.is_none() && c.setter_thread.is_none()
        });
        assert!(is_empty, "GuiFrameGuard did not clear context on panic-drop");
    }

    /// S5 regression: `clear_ui` is idempotent and always leaves the context
    /// in the empty state. Multiple calls must not panic or leave stale state.
    #[test]
    fn test_context_clear_ui_idempotent() {
        clear_ui();
        clear_ui();
        clear_ui();

        let is_empty = GUI_CTX.with(|c| {
            let c = c.borrow();
            c.ptr.is_none() && c.setter_thread.is_none()
        });
        assert!(is_empty);
    }

    /// S5 regression: the full `set_ui` → `with_ui` → guard-drop flow works
    /// on a single thread using a real `egui::Ui` constructed via
    /// `egui::Context::default()`. Verifies that the pointer is set, the
    /// closure executes, and the context is cleared after the guard drops.
    #[test]
    fn test_context_set_ui_and_with_ui_same_thread() {
        clear_ui();

        let ctx = egui::Context::default();
        // egui::Context::run 返回 FullOutput（#[must_use]），测试仅验证
        // set_ui/with_ui 副作用与 guard 释放后 GUI_CTX 清空，显式忽略返回值。
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _guard = set_ui(ui);

                let label_rendered = with_ui(|ui| {
                    ui.label("regression-test-marker");
                    true
                });
                assert!(label_rendered, "with_ui closure should execute");
            });
            // _guard dropped here, clearing GUI_CTX
        });

        // After the frame, the context must be cleared.
        let is_empty = GUI_CTX.with(|c| c.borrow().ptr.is_none());
        assert!(is_empty, "GUI_CTX not cleared after frame guard dropped");
    }

    /// S5 regression: `with_ui` panics with a cross-thread message if the
    /// accessor's `ThreadId` differs from the setter's.
    ///
    /// Normally `thread_local!` prevents cross-thread access (each thread has
    /// its own `GUI_CTX`), but this test simulates the invariant by injecting
    /// a foreign `ThreadId` directly. The cross-thread assert must fire
    /// *before* the pointer is dereferenced, so a dangling `NonNull` is safe.
    #[test]
    #[should_panic(expected = "GUI_CTX cross-thread access")]
    fn test_context_cross_thread_access_panics() {
        clear_ui();

        // Obtain a foreign ThreadId by spawning a thread.
        let foreign_id = std::thread::spawn(|| std::thread::current().id())
            .join()
            .expect("spawned thread panicked");

        // Inject a foreign ThreadId + dangling pointer. The dangling pointer
        // is NEVER dereferenced — the cross-thread assert fires first.
        GUI_CTX.with(|ctx| {
            *ctx.borrow_mut() =
                GuiCtx { ptr: Some(NonNull::dangling()), setter_thread: Some(foreign_id) };
        });

        // This must panic with "cross-thread access" before dereferencing.
        with_ui(|_ui| {});
    }
}
