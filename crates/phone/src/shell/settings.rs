//! App settings (Expo `app/settings.tsx`, board 120), pushed from the
//! account sheet: the network in hand and the way to another (Switch
//! network, which keeps this phone's key), this phone and where its key is
//! kept, the appearance, the app's version. The Expo dev build's Relay
//! section is not here: a phone build is never one.
//!
//! It reads the session (the node, the network), the chain (the network's
//! founding time), the account (this phone's key) and what the key store
//! said as the page opened; it writes nothing.

use super::look::{self, Palette};
use super::{Root, about};
use ducktape_host::a11y::Control as _;
use ducktape_host::contract::{KeyLevel, KeyStorage};
use ducktape_host::session::Shared;
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, Div, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Window, div, px, svg,
};

/// The page: its Back header, then the sections, scrolled.
pub fn page(
    shared: &Shared,
    storage: Option<&Result<KeyStorage, String>>,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Div {
    let back = cx.listener(|root, _, _, cx| root.pop_page(cx));
    let session = shared.session.read(cx).get();
    let network = match session.connected_rpc.is_empty() {
        true => row(
            "settings/network",
            "icons/server.svg",
            "Not connected",
            None,
            palette,
        ),
        false => {
            let host = session
                .connected_rpc
                .strip_prefix("https://")
                .or_else(|| session.connected_rpc.strip_prefix("http://"))
                .unwrap_or(&session.connected_rpc);
            let founded = shared.chain.read(cx).node.as_ref().map(|node| node.time);
            let caption = match founded {
                Some(founded) => format!("{host} · founded {}", day(founded)),
                None => host.to_owned(),
            };
            row(
                "settings/network",
                "icons/server.svg",
                session.network.clone(),
                Some(caption_mono(caption, palette)),
                palette,
            )
        }
    };
    let switch = row(
        "settings/switch",
        "icons/refresh-cw.svg",
        "Switch network…",
        Some(caption(
            "Keeps this phone's key; join the other network by code.",
            palette,
        )),
        palette,
    )
    .control(Role::Button, "Switch network…")
    .active(|style| style.opacity(look::PRESSED))
    .child(
        svg()
            .path("icons/chevron-right.svg")
            .size(px(look::ICON_ROW))
            .flex_none()
            .text_color(palette.faint()),
    )
    .on_click(cx.listener(|root, _, window, cx| root.show_switch(window, cx)));
    let account = shared.account.read(cx);
    let key = noded_client::hex::hex_decode(&account.get().signer_key)
        .ok()
        .filter(|key| !key.is_empty())
        .map(|key| {
            format!(
                "key {} · fingerprint {}",
                design::short_hex(&account.get().signer_key),
                noded_client::join::fingerprint(&key)
            )
        });
    let phone = row(
        "settings/phone",
        "icons/smartphone.svg",
        crate::os::phone(),
        key.map(|key| caption(key, palette)),
        palette,
    );
    let (kept, ok) = match storage {
        Some(Ok(storage)) => (
            format!("{} · keyring {}", words(storage), account.keyring()),
            ok(storage),
        ),
        Some(Err(unread)) => (
            format!("{} · its level unread: {unread}", crate::os::KEY_STORE),
            false,
        ),
        None => (crate::os::KEY_STORE.to_owned(), false),
    };
    let mut storage = row(
        "settings/key-storage",
        "icons/lock.svg",
        "Key storage",
        Some(caption(kept, palette)),
        palette,
    );
    if ok {
        storage = storage.child(badge("OK", palette));
    }
    let appearance = row(
        "settings/appearance",
        "icons/sun-moon.svg",
        "Follows the system",
        Some(caption(
            format!("Light and dark come from {}'s setting.", crate::os::SYSTEM),
            palette,
        )),
        palette,
    );
    let version = row(
        "settings/about",
        "icons/info.svg",
        "ducktape mobile",
        Some(caption_mono(env!("CARGO_PKG_VERSION"), palette)),
        palette,
    );
    let body = div()
        .id("settings")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .child(about::section("Network", None, palette))
        .child(network)
        .child(switch)
        .child(about::section("This phone", None, palette))
        .child(phone)
        .child(storage)
        .child(about::section("Appearance", None, palette))
        .child(appearance)
        .child(about::section("About", None, palette))
        .child(version);
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(header("settings/back", Some("App settings"), back, palette))
        .child(body)
}

/// The Expo `Header` of a page that is not a root: the back arrow, the
/// title 17 medium when there is one, a hairline under.
pub fn header(
    id: &'static str,
    title: Option<&'static str>,
    back: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    palette: Palette,
) -> Div {
    let back = div()
        .id(id)
        .control(Role::Button, "Back")
        .size(px(look::HIT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .active(|style| style.opacity(look::PRESSED))
        .child(
            svg()
                .path("icons/arrow-left.svg")
                .size(px(look::ICON_HIT))
                .text_color(palette.foreground()),
        )
        .on_click(back);
    div()
        .h(px(look::HEADER))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(look::gap::XXS))
        .px(px(look::gap::SM))
        .border_b_1()
        .border_color(palette.border())
        .bg(palette.background())
        .child(back)
        .children(title.map(|title| {
            div().flex_1().min_w_0().px(px(look::gap::SM)).child(
                look::text(look::TITLE, FontWeight::MEDIUM, palette.foreground())
                    .truncate()
                    .child(title),
            )
        }))
}

/// A list row (Expo `Row`): the 24 lead column with its 18 muted icon, the
/// title over its caption, then whatever the caller puts at the right; 56
/// with a caption (48 without), 6 above and below, the gutter at the sides,
/// a hairline under.
fn row(
    id: &'static str,
    icon: &'static str,
    title: impl Into<SharedString>,
    caption: Option<Div>,
    palette: Palette,
) -> Stateful<Div> {
    let tall = caption.is_some();
    let words = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(1.))
        .child(
            look::text(look::BODY, FontWeight::NORMAL, palette.foreground())
                .truncate()
                .child(title.into()),
        )
        .children(caption);
    div()
        .id(id)
        .min_h(px(if tall { look::ROW_TALL } else { ROW }))
        .flex()
        .items_center()
        .gap(px(look::gap::LG))
        .py(px(look::gap::XS))
        .px(px(look::GUTTER))
        .border_b_1()
        .border_color(palette.border())
        .child(
            div().w(px(24.)).flex_none().flex().justify_center().child(
                svg()
                    .path(icon)
                    .size(px(look::ICON_ROW))
                    .text_color(palette.muted()),
            ),
        )
        .child(words)
}

/// A row without a caption (`size.row`).
const ROW: f32 = 48.;

/// A row's caption: 12 muted, one line.
fn caption(text: impl Into<SharedString>, palette: Palette) -> Div {
    look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
        .truncate()
        .child(text.into())
}

/// A caption in the data face: an address, a version.
fn caption_mono(text: impl Into<SharedString>, palette: Palette) -> Div {
    caption(text, palette).font_family(ducktape_host::fonts::FAMILY_MONO)
}

/// The Expo `Badge` in its ok tint: 20 tall, tiny medium in `success` on
/// `success_soft`.
fn badge(label: &'static str, palette: Palette) -> AnyElement {
    div()
        .h(px(20.))
        .flex_none()
        .px(px(look::gap::XS))
        .flex()
        .items_center()
        .bg(palette.success_soft())
        .child(look::text(look::TINY, FontWeight::MEDIUM, palette.success()).child(label))
        .into_any_element()
}

/// Where the key is kept, in the Expo build's words (mobile
/// `src/session/key-storage.ts`): only what the store reported.
fn words(storage: &KeyStorage) -> String {
    let android = |level: &str| match storage.sealed {
        true => format!("Sealed by Android Keystore, {level}"),
        false => format!("Android Keystore, {level}"),
    };
    match storage.level {
        KeyLevel::SecureEnclave => "Sealed by the Secure Enclave, hardware-backed".to_owned(),
        KeyLevel::Keychain => "iOS Keychain, this device only, software-backed".to_owned(),
        KeyLevel::KeystoreStrongBox => android("hardware-backed (StrongBox)"),
        KeyLevel::KeystoreTee => android("hardware-backed (TEE)"),
        KeyLevel::KeystoreSoftware => android("software-backed"),
        // the desktop's two: no phone's store answers them
        KeyLevel::OsKeyring => "The system keyring".to_owned(),
        KeyLevel::File => "A key file".to_owned(),
    }
}

/// The OK badge: only a key the store says lives in (or is sealed by) a
/// TEE, StrongBox or the Secure Enclave (Expo `storageOk`).
fn ok(storage: &KeyStorage) -> bool {
    matches!(
        storage.level,
        KeyLevel::KeystoreTee | KeyLevel::KeystoreStrongBox | KeyLevel::SecureEnclave
    )
}

/// Unix ms `ms` as the local calendar day it falls on, `16 Oct 2026`
/// (Expo `src/design/format.ts` `day`).
fn day(ms: u64) -> String {
    let seconds = i64::try_from(ms / 1000).unwrap_or(i64::MAX);
    let local = seconds.saturating_add(ducktape_host::runtime::local_offset(seconds));
    ducktape_host::runtime::day(u64::try_from(local).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store's report reads as the Expo build's words, the badge only on
    /// hardware it named.
    #[test]
    fn key_storage_reads_as_the_expo_builds_words() {
        let kept = |level, sealed| KeyStorage { sealed, level };
        let keychain = kept(KeyLevel::Keychain, false);
        assert_eq!(
            words(&keychain),
            "iOS Keychain, this device only, software-backed"
        );
        assert!(!ok(&keychain));
        let tee = kept(KeyLevel::KeystoreTee, true);
        assert_eq!(
            words(&tee),
            "Sealed by Android Keystore, hardware-backed (TEE)"
        );
        assert!(ok(&tee));
    }
}
