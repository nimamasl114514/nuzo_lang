//! Widget rendering builtins (button, label, input, etc.)
//!
//! All widgets access the current egui Ui via `crate::context::with_ui()`.
//! Input state (for text fields) is persisted across frames using thread-local HashMaps.

use std::cell::RefCell;

use nuzo_core::Value;
use nuzo_helpers::BuiltinRegistry;
use nuzo_values::{HeapObject, NIL, NuzoError, ValueExt};

/// Register multiple widget builtins via a compact declarative form.
///
/// Each entry is `"name" => fn, arity` separated by `;`. The macro expands to a
/// sequence of `registry.register(name, fn, arity)` calls preserving order.
macro_rules! define_widgets {
    ($reg:expr, $($name:literal => $fn:ident, $arity:expr);* $(;)?) => {
        $($reg.register($name, $fn, $arity);)*
    };
}

// Thread-local input state (persists across frames)
thread_local! {
    static INPUT_STATE: RefCell<std::collections::HashMap<String, String>> = RefCell::new(std::collections::HashMap::new());
}

/// Extract an array of strings from a nuzo Value (HeapObject::Array).
fn extract_string_array(v: &Value) -> Result<Vec<String>, NuzoError> {
    let obj =
        v.as_heap_object_opt().ok_or_else(|| NuzoError::type_mismatch("array", v.type_name()))?;
    match obj.as_ref() {
        HeapObject::Array(arr) => {
            Ok(arr.iter().map(|item| item.as_string_opt().unwrap_or_default()).collect())
        }
        _ => Err(NuzoError::type_mismatch("array", v.type_name())),
    }
}

/// `heading(text)` — large heading text.
pub fn gui_heading(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    let text = args[0].as_string_opt().unwrap_or_default();
    crate::context::with_ui(|ui| {
        ui.heading(text);
    });
    Ok(NIL)
}

/// `label(text)` — normal text label.
pub fn gui_label(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    let text = args[0].as_string_opt().unwrap_or_default();
    crate::context::with_ui(|ui| {
        ui.label(text);
    });
    Ok(NIL)
}

/// `separator()` — horizontal separator line.
pub fn gui_separator(_args: &[Value]) -> Result<Value, NuzoError> {
    crate::context::with_ui(|ui| {
        ui.separator();
    });
    Ok(NIL)
}

/// `button(text, color?)` → bool — clickable button, returns true if clicked.
pub fn gui_button(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    let text = args[0].as_string_opt().unwrap_or_default();
    let clicked = crate::context::with_ui(|ui| {
        let mut btn = egui::Button::new(text);
        if args.len() >= 2
            && !args[1].is_nil()
            && let Some(color_str) = args[1].as_string_opt()
            && let Some(color) = crate::theme::parse_hex_color(&color_str)
        {
            btn = btn.fill(color);
        }
        ui.add(btn).clicked()
    });
    Ok(Value::from_bool(clicked))
}

/// `checkbox(text, checked)` → bool — toggle checkbox.
pub fn gui_checkbox(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() < 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let text = args[0].as_string_opt().unwrap_or_default();
    let mut checked = if args[1].is_bool() { args[1].as_bool() } else { false };
    crate::context::with_ui(|ui| {
        ui.checkbox(&mut checked, text);
    });
    Ok(Value::from_bool(checked))
}

/// `radio(labels, idx)` → number — radio button group.
pub fn gui_radio(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() < 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let labels = extract_string_array(&args[0])?;
    let mut idx = args[1].try_number().unwrap_or(0.0) as usize;
    crate::context::with_ui(|ui| {
        for (i, label) in labels.iter().enumerate() {
            let is_selected = i == idx;
            if ui.radio(is_selected, label.as_str()).clicked() {
                idx = i;
            }
        }
    });
    Ok(Value::from_number(idx as f64))
}

/// `input(label, id?)` → string — single-line text input.
///
/// The optional `id` parameter is the state persistence key, decoupling the
/// display label from the input identity. When `id` is omitted, `label` is
/// used as the key (backward-compatible but fragile — multiple inputs with
/// the same label will share state).
///
/// # Recommendation
///
/// Always pass an explicit `id` when rendering multiple inputs with the same
/// or similar labels, to avoid state collisions.
///
/// # Example
///
/// ```nuzo
/// // Two inputs with the same label but independent state:
/// input("Name:", "player1_name")
/// input("Name:", "player2_name")
/// ```
pub fn gui_input(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    let label = args[0].as_string_opt().unwrap_or_default();
    // P2-3 fix: decouple display label from state key. Use explicit `id`
    // (second arg) if provided; otherwise fall back to `label` for backward
    // compatibility. This prevents state collisions when multiple inputs
    // share the same display label.
    let state_key = if args.len() >= 2 {
        args[1].as_string_opt().unwrap_or_default().to_string()
    } else {
        label.to_string()
    };
    let result = crate::context::with_ui(|ui| {
        INPUT_STATE.with(|s| {
            let mut state = s.borrow_mut();
            let text = state.entry(state_key).or_insert_with(|| label.to_string());
            ui.text_edit_singleline(text);
            text.clone()
        })
    });
    Ok(Value::from_string(&result))
}

/// `textarea(label, id?)` → string — multi-line text input.
///
/// See [`gui_input`] for the `id` parameter semantics.
pub fn gui_textarea(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    let label = args[0].as_string_opt().unwrap_or_default();
    // P2-3 fix: same decoupling as gui_input.
    let state_key = if args.len() >= 2 {
        args[1].as_string_opt().unwrap_or_default().to_string()
    } else {
        label.to_string()
    };
    let result = crate::context::with_ui(|ui| {
        INPUT_STATE.with(|s| {
            let mut state = s.borrow_mut();
            let text = state.entry(state_key).or_insert_with(|| label.to_string());
            ui.text_edit_multiline(text);
            text.clone()
        })
    });
    Ok(Value::from_string(&result))
}

/// `slider(min, max, val)` → number — numeric slider.
pub fn gui_slider(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() < 3 {
        return Err(NuzoError::invalid_argument_count(3, args.len()));
    }
    let min = args[0].try_number().unwrap_or(0.0);
    let max = args[1].try_number().unwrap_or(100.0);
    let mut val = args[2].try_number().unwrap_or(min);
    crate::context::with_ui(|ui| {
        ui.add(egui::Slider::new(&mut val, min..=max));
    });
    Ok(Value::from_number(val))
}

/// `progress(val, max)` — progress bar.
pub fn gui_progress(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    let fraction = if args.len() >= 2 {
        let val = args[0].try_number().unwrap_or(0.0);
        let max = args[1].try_number().unwrap_or(1.0);
        if max == 0.0 { 1.0 } else { (val / max).clamp(0.0, 1.0) }
    } else {
        args[0].try_number().unwrap_or(0.0).clamp(0.0, 1.0)
    } as f32;
    crate::context::with_ui(|ui| {
        ui.add(egui::ProgressBar::new(fraction));
    });
    Ok(NIL)
}

/// `tooltip(text)` — info icon with hover tooltip.
pub fn gui_tooltip(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    let text = args[0].as_string_opt().unwrap_or_default();
    crate::context::with_ui(|ui| {
        ui.label("ℹ").on_hover_text(text);
    });
    Ok(NIL)
}

// Menu widgets (v1 stub — requires callback mechanism for full impl)
/// `menu(label)` — start a menu. v1: stub.
pub fn gui_menu(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    let label = args[0].as_string_opt().unwrap_or_default();
    crate::context::with_ui(|ui| {
        ui.menu_button(label, |_ui| {
            // v1: content placeholder — full impl needs callback injection
        });
    });
    Ok(NIL)
}

/// `menu_item(label)` → bool — a menu item. v1: stub, always returns false.
pub fn gui_menu_item(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    // v1: menu_item outside menu context has no effect
    let _label = args[0].as_string_opt().unwrap_or_default();
    Ok(Value::from_bool(false))
}

/// Register all widget builtins into the registry (中英双语).
pub fn register_widgets(registry: &mut BuiltinRegistry) {
    define_widgets!(registry,
        "标题" => gui_heading, 1;
        "heading" => gui_heading, 1;
        "标签" => gui_label, 1;
        "label" => gui_label, 1;
        "分隔线" => gui_separator, 0;
        "separator" => gui_separator, 0;

        "按钮" => gui_button, 1;
        "button" => gui_button, 1;
        "复选框" => gui_checkbox, 2;
        "checkbox" => gui_checkbox, 2;
        "单选组" => gui_radio, 2;
        "radio" => gui_radio, 2;

        "输入框" => gui_input, 1;
        "input" => gui_input, 1;
        "多行框" => gui_textarea, 1;
        "textarea" => gui_textarea, 1;
        "滑动条" => gui_slider, 3;
        "slider" => gui_slider, 3;

        "进度条" => gui_progress, 1;
        "progress" => gui_progress, 1;
        "提示" => gui_tooltip, 1;
        "tooltip" => gui_tooltip, 1;

        // Menu (v1 stub)
        "菜单栏" => gui_menu, 1;
        "menu" => gui_menu, 1;
        "菜单项" => gui_menu_item, 1;
        "menu_item" => gui_menu_item, 1
    );
}
