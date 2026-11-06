//! Web builds embed gpui-component's default icon set.
//!
//! gpui-kit-assets embeds those icons on the desktop, but its wasm build
//! fetches each one at run time from `{endpoint}/assets/icons/…`, which needs
//! an absolute URL and the SVGs served next to the page; neither holds for
//! the web product. Embedding the same set (about 130 KB of SVG) makes the
//! icons available at once, as on the desktop. See `src/component_assets.rs`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let mut code = String::from("pub(crate) static ICONS: &[(&str, &[u8])] = &[\n");
    let wasm = env::var("CARGO_CFG_TARGET_FAMILY")
        .is_ok_and(|families| families.split(',').any(|family| family == "wasm"));
    if wasm {
        // gpui-kit-assets publishes its icon directory through its `links`
        // metadata; the default list sits in its crate root.
        let icons = PathBuf::from(
            env::var("DEP_GPUI_KIT_DEFAULT_ICONS_ICONS_DIR")
                .expect("gpui-kit-assets publishes its icon directory"),
        );
        let root = icons
            .parent()
            .and_then(Path::parent)
            .expect("the icon directory is <crate>/assets/icons");
        let list = root.join("default-icons.txt");
        println!("cargo:rerun-if-changed={}", list.display());
        let list = fs::read_to_string(&list).expect("gpui-kit-assets' default icon list");
        for path in list.lines().filter(|line| !line.is_empty()) {
            let file = root.join("assets").join(path);
            writeln!(code, "    ({path:?}, include_bytes!({:?})),", file.display().to_string())
                .unwrap();
        }
    }
    code.push_str("];\n");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    fs::write(out.join("component_icons.rs"), code).expect("write the icon table");
}
