//! Theme and style configuration builtins for egui.
//! Provides light/dark theme switching, color parsing, and app quit.

use nuzo_core::Value;
use nuzo_values::{NIL, NuzoError, ValueExt};

use std::cell::RefCell;

thread_local! {
    static CURRENT_THEME: RefCell<ThemeMode> = const { RefCell::new(ThemeMode::Light) };
}

#[derive(Clone, Copy, PartialEq)]
enum ThemeMode {
    Light,
    Dark,
}

/// Set the theme to light mode.
pub fn gui_light_theme(_args: &[Value]) -> Result<Value, NuzoError> {
    CURRENT_THEME.with(|t| *t.borrow_mut() = ThemeMode::Light);
    crate::context::with_ui(|ui| {
        ui.ctx().set_visuals(egui::Visuals::light());
    });
    Ok(NIL)
}

/// Set the theme to dark mode.
pub fn gui_dark_theme(_args: &[Value]) -> Result<Value, NuzoError> {
    CURRENT_THEME.with(|t| *t.borrow_mut() = ThemeMode::Dark);
    crate::context::with_ui(|ui| {
        ui.ctx().set_visuals(egui::Visuals::dark());
    });
    Ok(NIL)
}

/// Set the theme by name (zh/en supported).
/// Names: "亮色"/"light", "暗色"/"dark".
pub fn gui_theme(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, 0));
    }
    let name = args[0].as_string_opt().unwrap_or_default();
    crate::context::with_ui(|ui| {
        match name.as_str() {
            "亮色" | "light" => {
                CURRENT_THEME.with(|t| *t.borrow_mut() = ThemeMode::Light);
                ui.ctx().set_visuals(egui::Visuals::light());
            }
            "暗色" | "dark" => {
                CURRENT_THEME.with(|t| *t.borrow_mut() = ThemeMode::Dark);
                ui.ctx().set_visuals(egui::Visuals::dark());
            }
            _ => {} // unknown theme name, silently ignored
        }
    });
    Ok(NIL)
}

/// Quit the application by sending a close command to the viewport.
pub fn gui_quit(_args: &[Value]) -> Result<Value, NuzoError> {
    crate::context::with_ui(|ui| {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    });
    Ok(NIL)
}

/// Parse a hex color string (e.g. "#FF0000" or "#FF0000AA") into an egui Color32.
/// Returns None if the string is not a valid hex color.
///
/// # 实现要点
///
/// 直接对 `s.as_bytes()` 切片并校验全部字节都是 ASCII hex digit。
/// 这避免了原实现对 `s.len()`（UTF-8 字节数）的隐式假设：
/// 非 ASCII 输入（如 `"#颜色"`）下 `s.len() == 6` 可能为真，
/// 但 `&s[0..2]` 在多字节字符边界切片会 panic（byte index not on a char boundary）。
/// 字节级切片 + `from_str_radix` 解析对 ASCII hex 输入与原版等价，
/// 对非 ASCII 输入则安全返回 `None`。
pub fn parse_hex_color(s: &str) -> Option<egui::Color32> {
    let s = s.trim_start_matches('#');
    let bytes = s.as_bytes();

    // 仅接受 6 或 8 字节长度，且全部为 ASCII hex digit。
    // 这同时排除了多字节 UTF-8 字符（其字节 ≥ 0x80，非 ASCII hex digit）。
    let valid_len = bytes.len() == 6 || bytes.len() == 8;
    if !valid_len || !bytes.iter().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }

    let parse_byte = |i: usize| -> Option<u8> {
        // 安全：上面已校验 bytes 长度 ≥ 6，且全是 ASCII hex digit（仅 ASCII 字节），
        // 因此 from_utf8 必成功。
        let hex = std::str::from_utf8(&bytes[i..i + 2]).expect("validated ASCII hex bytes");
        u8::from_str_radix(hex, 16).ok()
    };

    if bytes.len() == 6 {
        let r = parse_byte(0)?;
        let g = parse_byte(2)?;
        let b = parse_byte(4)?;
        Some(egui::Color32::from_rgb(r, g, b))
    } else {
        // bytes.len() == 8
        let r = parse_byte(0)?;
        let g = parse_byte(2)?;
        let b = parse_byte(4)?;
        let a = parse_byte(6)?;
        Some(egui::Color32::from_rgba_unmultiplied(r, g, b, a))
    }
}

/// Register all theme, style, and quit builtins into the registry.
pub fn register_theme(registry: &mut nuzo_helpers::BuiltinRegistry) {
    registry.register("主题", gui_theme, 1);
    registry.register("theme", gui_theme, 1);
    registry.register("亮色主题", gui_light_theme, 0);
    registry.register("light_theme", gui_light_theme, 0);
    registry.register("暗色主题", gui_dark_theme, 0);
    registry.register("dark_theme", gui_dark_theme, 0);
    registry.register("退出", gui_quit, 0);
    registry.register("quit", gui_quit, 0);
}
