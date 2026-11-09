//! About dialog overlay: logo + product name + version + copyright + links.
//!
//! A plain overlay div keeps the dialog independent of gpui-component's
//! Root-hosted Modal.

use gpui::{
    ClickEvent, Context, Div, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::{Icon, Sizable as _, Size};
use workbench_api as wb;

use crate::dialog::{self, DialogFocus};
use crate::{SharedCore, col, lock};

/// Renders the About overlay for the current product profile.
pub(crate) fn overlay(
    core: &SharedCore,
    focus: &DialogFocus,
    cx: &mut Context<crate::RootView>,
) -> gpui::AnyElement {
    // Snapshot everything the dialog needs while holding the lock: the
    // profile, the product name and the handful of translated labels.
    let snap = {
        let guard = lock(core);
        AboutSnap {
            profile: guard.profile().clone(),
            product_name: guard.product_name().to_string(),
            version_label: guard.host.tr("Version"),
            home_label: guard.host.tr("Home"),
            support_label: guard.host.tr("Support"),
            close_label: guard.host.tr("Close"),
        }
    };
    render_about(&snap, core, focus, cx).into_any_element()
}

/// Everything the About overlay renders, snapshotted under the runtime lock.
struct AboutSnap {
    profile: workbench_api::ProductProfile,
    product_name: String,
    version_label: String,
    home_label: String,
    support_label: String,
    close_label: String,
}

fn render_about(
    snap: &AboutSnap,
    core: &SharedCore,
    focus: &DialogFocus,
    cx: &mut Context<crate::RootView>,
) -> gpui::Stateful<Div> {
    let profile = &snap.profile;
    let version = format!("{} {}", snap.version_label, profile.display_version());

    let (overlay, card) = dialog::platform_shell("about", 360.0, core, focus, cx);
    let mut card = card.p(px(24.0)).gap(px(12.0));

    // Header: logo (when the product ships one) + name + version.
    let mut header = div().flex().items_center().gap(px(10.0));
    if let Some(logo) = &profile.logo {
        header = header.child(
            Icon::empty()
                .path(logo.clone())
                .with_size(Size::Size(px(32.0)))
                .text_color(col(wb::Color::LIGHT_GRAY)),
        );
    }
    header = header
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(SharedString::from(snap.product_name.clone())),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(col(wb::Color::GRAY))
                        .child(SharedString::from(version)),
                ));
    card = card.child(header);

    // Optional body lines: copyright and license text.
    for line in [&profile.copyright, &profile.license_text]
        .into_iter()
        .flatten()
    {
        card = card.child(
            div()
                .text_size(px(12.0))
                .text_color(col(wb::Color::GRAY))
                .child(SharedString::from(line.clone())),
        );
    }

    // Footer: optional links + close button.
    let mut footer = div().flex().items_center().justify_end().gap(px(8.0));
    for (url, label) in [
        (&profile.about_url, snap.home_label.as_str()),
        (&profile.support_url, snap.support_label.as_str()),
    ] {
        if let Some(url) = url {
            footer = footer.child(link_label(label, url, cx));
        }
    }
    footer = footer.child(dialog::close_button("about", snap.close_label.clone(), core, cx));
    card = card.child(footer);

    overlay.child(card)
}

/// A clickable text link: opens the URL in the system browser via gpui's
/// platform `open_url` (ShellExecute on Windows, `open` on macOS, xdg-open
/// on Linux).
fn link_label(label: &str, url: &str, cx: &mut Context<crate::RootView>) -> gpui::Stateful<Div> {
    let url = url.to_string();
    div()
        .id(format!("about-link-{url}"))
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(col(wb::Color::ACCENT))
        .hover(|s| s.underline())
        .child(SharedString::from(format!("{label} ({url})")))
        .on_click(cx.listener(
            move |_: &mut crate::RootView, _: &ClickEvent, _: &mut Window, cx: &mut Context<'_, crate::RootView>| {
                cx.open_url(&url);
            },
        ))
}
