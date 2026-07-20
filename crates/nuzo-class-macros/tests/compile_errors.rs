use std::path::PathBuf;

fn ensure_cargo_env() {
    if std::env::var_os("CARGO").is_some() {
        return;
    }

    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".cargo")));
    let Some(cargo_home) = cargo_home else {
        return;
    };

    let cargo_exe = cargo_home.join("bin").join("cargo.exe");
    if cargo_exe.exists() {
        std::env::set_var("CARGO", cargo_exe);
    }
}

#[cfg_attr(
    target_os = "windows",
    ignore = "trybuild subprocess creation is unstable in the Windows sandbox"
)]
#[test]
fn ui() {
    ensure_cargo_env();
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
