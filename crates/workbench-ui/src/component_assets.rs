//! gpui-component's default icons.
//!
//! On the desktop this is gpui-kit-assets' own bundle. On the web it is the
//! same set embedded by `build.rs`: gpui-kit-assets' wasm build would fetch
//! each icon over HTTP from next to the page, where the web product does not
//! serve them.

#[cfg(not(target_family = "wasm"))]
pub(crate) use gpui_component_assets::Assets as ComponentAssets;

#[cfg(target_family = "wasm")]
pub(crate) use web::ComponentAssets;

#[cfg(target_family = "wasm")]
mod web {
    use std::borrow::Cow;

    use gpui::SharedString;

    include!(concat!(env!("OUT_DIR"), "/component_icons.rs"));

    pub(crate) struct ComponentAssets;

    impl ComponentAssets {
        /// Same signature as gpui-kit-assets' `Assets::new`; nothing is fetched.
        pub(crate) fn new(_endpoint: &str) -> Self {
            Self
        }
    }

    impl gpui::AssetSource for ComponentAssets {
        fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
            Ok(ICONS
                .iter()
                .find(|(icon, _)| *icon == path)
                .map(|(_, bytes)| Cow::Borrowed(*bytes)))
        }

        fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
            Ok(ICONS
                .iter()
                .filter(|(icon, _)| icon.starts_with(path))
                .map(|(icon, _)| (*icon).into())
                .collect())
        }
    }
}
