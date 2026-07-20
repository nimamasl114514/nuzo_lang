//! nuzo_gui — Launch a nuzo script as an egui GUI application.
//!
//! Usage: nuzo_gui <script.nuzo>
//!
//! The script must define a `render()` function that is called every frame.

use std::cell::RefCell;
use std::env;
use std::path::PathBuf;
use std::rc::Rc;

use nuzo_run::Engine;
use nuzo_vm::VM;

use nuzo_gui::context;

/// eframe App that owns the VM and renders it every frame.
///
/// # Concurrency (S6 fix)
///
/// `VM` is `!Send` (it holds thread_local state and non-Send resources).
/// egui is single-threaded, so the VM is wrapped in `Rc<RefCell<VM>>` — not
/// `Arc<Mutex<VM>>` — to make the single-threaded contract explicit at the
/// type level. `Rc<RefCell>` is `!Send`, which prevents the App from being
/// moved across threads, matching egui's threading model exactly.
///
/// The previous `Arc<Mutex<VM>>` + `#[allow(clippy::arc_with_non_send_sync)]`
/// falsely implied thread-safety while suppressing the lint that flagged the
/// violation. Switching to `Rc<RefCell>` removes the lint suppression and
/// the false promise.
struct NuzoGuiApp {
    vm: Rc<RefCell<VM>>,
}

impl NuzoGuiApp {
    fn new(vm: VM) -> Self {
        Self { vm: Rc::new(RefCell::new(vm)) }
    }
}

impl eframe::App for NuzoGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut vm = self.vm.borrow_mut();

            // `set_ui` returns a guard that clears the GUI_CTX pointer on
            // drop — even if the VM call panics. This is the S5 fix: the
            // pointer cannot leak across frames. The guard holds an
            // exclusive borrow of `ui`, so all UI access during the VM call
            // must go through `with_ui()` (the intended pattern).
            let _ui_guard = context::set_ui(ui);

            let result = vm.call_global_function("render", &[]);
            if let Err(e) = result {
                // Route error rendering through `with_ui` to respect the
                // guard's exclusive borrow of `ui` (cannot touch `ui`
                // directly while the guard is alive).
                context::with_ui(|ui| {
                    ui.colored_label(egui::Color32::RED, format!("render error: {}", e));
                });
            }
            // `_ui_guard` drops here, clearing GUI_CTX. `vm` (RefMut) also
            // drops, releasing the RefCell borrow.
        });

        ctx.request_repaint();
    }
}

/// Configure egui fonts to support CJK characters.
fn setup_cjk_fonts(cc: &eframe::CreationContext<'_>) {
    let mut fonts = egui::FontDefinitions::default();

    // Try to load system CJK fonts (Windows: MSYH, macOS: PingFang, Linux: NotoSansCJK)
    let cjk_font_data: Option<Vec<u8>> = {
        let candidates: &[&str] = &[
            r"C:\Windows\Fonts\msyh.ttc",
            "/System/Library/Fonts/PingFang.ttc",
            "/Library/Fonts/Arial Unicode.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        ];

        let mut found = None;
        for path in candidates {
            if let Ok(data) = std::fs::read(path) {
                found = Some(data);
                break;
            }
        }
        found
    };

    if let Some(data) = cjk_font_data {
        fonts.font_data.insert("cjk".to_owned(), egui::FontData::from_owned(data));

        // Add CJK font as fallback for proportional and monospace
        fonts.families.entry(egui::FontFamily::Proportional).or_default().push("cjk".to_owned());
        fonts.families.entry(egui::FontFamily::Monospace).or_default().push("cjk".to_owned());
    }

    cc.egui_ctx.set_fonts(fonts);
}

fn main() -> eframe::Result {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: nuzo_gui <script.nuzo>");
        std::process::exit(1);
    }

    let script_path = PathBuf::from(&args[1]);
    let source = match std::fs::read_to_string(&script_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {}: {}", script_path.display(), e);
            std::process::exit(1);
        }
    };

    // Build engine and register GUI builtins before any session is created
    let mut engine =
        Engine::builder().with_default_config().build().expect("Failed to build engine");
    engine.with_registry(|registry| {
        nuzo_gui::register_all(registry);
    });

    let mut session = engine.new_session();
    session.set_module_path(&script_path);
    if let Err(e) = session.run(&source) {
        eprintln!("Script error: {}", e);
        std::process::exit(1);
    }

    let vm = session.into_vm();

    let app = NuzoGuiApp::new(vm);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_title("Nuzo GUI"),
        ..Default::default()
    };

    eframe::run_native(
        "Nuzo GUI",
        options,
        Box::new(|cc| {
            setup_cjk_fonts(cc);
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S6 regression: `NuzoGuiApp.vm` must be `Rc<RefCell<VM>>` (single-threaded),
    /// NOT `Arc<Mutex<VM>>` (which falsely implies `Send`/`Sync` and requires
    /// `#[allow(clippy::arc_with_non_send_sync)]`).
    ///
    /// This is a compile-time assertion: if someone reverts to
    /// `Arc<Mutex<VM>>`, this test fails to compile because
    /// `Arc<Mutex<VM>>: Deref<Target = Mutex<VM>>`, not `RefCell<VM>`.
    ///
    /// egui is single-threaded; `Rc<RefCell>` is the correct wrapper for
    /// `!Send` types in a single-threaded context.
    #[test]
    fn test_gui_vm_main_thread_only() {
        // Compile-time bound: the `vm` field's type must deref to
        // `RefCell<VM>`. Only `Rc<RefCell<VM>>` (and `&RefCell<VM>`) satisfy
        // this; `Arc<Mutex<VM>>` does not.
        fn assert_rc_refcell<T>(_t: &T)
        where
            T: std::ops::Deref<Target = std::cell::RefCell<nuzo_vm::VM>>,
        {
        }

        // The closure body references `app.vm`, forcing the compiler to
        // verify the field type satisfies the bound above. If `vm` is
        // `Arc<Mutex<VM>>`, compilation fails here.
        let assert_fn: fn(&NuzoGuiApp) = |app| assert_rc_refcell(&app.vm);
        // Suppress "unused variable" while keeping the compile-time check.
        let _ = assert_fn;
    }
}
