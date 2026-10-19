//! Regression: the product and platform-standard asset sources must resolve
//! the icon paths the ribbon asks for. Guards the asset-chain wiring that
//! feeds gpui's `AssetRegistry` (paths missing from `list("")` render as
//! invisible SVGs).

use workbench_api::AssetSource;

#[test]
fn product_assets_resolve_ribbon_icon_paths() {
    let ex = wb_example::example_assets();
    for path in [
        "icons/scan.svg",
        "icons/add.svg",
        "icons/mode.svg",
        "icons/output.svg",
        "icons/explorer.svg",
    ] {
        assert!(ex.load(path).is_some(), "example asset missing: {path}");
    }
}

#[test]
fn standard_assets_resolve_platform_icon_paths() {
    let std = workbench_core::assets::standard_assets();
    for path in ["wb-std/new.svg", "wb-std/open.svg", "wb-std/save.svg"] {
        assert!(std.load(path).is_some(), "standard asset missing: {path}");
    }
}

#[test]
fn builder_asset_source_chains_both() {
    let builder = workbench_core::WorkbenchAppBuilder::new("probe.assets", "probe")
        .with_assets(wb_example::example_assets());
    let source = builder.asset_source();
    for path in ["icons/scan.svg", "wb-std/new.svg"] {
        assert!(source.load(path).is_some(), "chained asset missing: {path}");
    }
}
