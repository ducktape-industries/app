//! App settings (Expo `app/settings.tsx`, board 120), pushed from the
//! Account tab: the network in hand and the way to another (Switch
//! network, which keeps this phone's key), this phone and where its key is
//! kept, the appearance, the app's version. The Expo dev build's Relay
//! section is not here: a phone build is never one.
//!
//! It reads the session (the node, the network), the chain (the network's
//! founding time), the account (this phone's key), the prefs and what the
//! key store said as the page opened; it writes two prefs, debug mode's
//! switch here and the appearance on its own page (the Expo build had
//! neither: it followed the system).

use super::look::{self, Palette};
use super::{Page, Root, about, join};
use ducktape_host::a11y::Control as _;
use ducktape_host::backend::Appearance;
use ducktape_host::contract::{KeyLevel, KeyStorage};
use ducktape_host::session::Shared;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div, px, svg,
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
            let host = join::host_of(&session.connected_rpc);
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
    .child(chevron(palette))
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
    let chosen = shared.prefs.read(cx).get().appearance;
    let appearance = row(
        "settings/appearance",
        "icons/sun-moon.svg",
        "Light and dark",
        Some(caption(chosen_words(chosen), palette)),
        palette,
    )
    .control(Role::Button, "Light and dark")
    .debug_selector(|| "settings/appearance".to_owned())
    .active(|style| style.opacity(look::PRESSED))
    .child(chevron(palette))
    .on_click(cx.listener(|root, _, _, cx| root.push_page(Page::Appearance, cx)));
    let on = shared.prefs.read(cx).get().debug;
    let debug = row(
        "settings/debug",
        "icons/gauge.svg",
        "Debug mode",
        Some(caption(
            "What each program costs, on the Open tab.",
            palette,
        )),
        palette,
    )
    .control(Role::Switch, "Debug mode")
    .aria_toggled(on.into())
    .debug_selector(|| "settings/debug".to_owned())
    .active(|style| style.opacity(look::PRESSED))
    .child(toggle(on, palette))
    .on_click(cx.listener(move |root, _, _, cx| {
        root.shared
            .prefs
            .update(cx, |prefs, cx| prefs.set_debug(!on, cx));
    }));
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
        .child(about::section("Debug", None, palette))
        .child(debug)
        .child(about::section("About", None, palette))
        .child(version);
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            look::header("settings/back", back, palette).child(
                div().flex_1().min_w_0().px(px(look::gap::SM)).child(
                    look::text(look::TITLE, FontWeight::MEDIUM, palette.foreground())
                        .truncate()
                        .child("App settings"),
                ),
            ),
        )
        .child(body)
}

/// Appearance (pushed from App settings' Light and dark row, as iOS
/// Settings' choice pages are): System, Light and Dark, the one in force
/// checked. A press saves it, and the app, every program and the system
/// bars take it at once (the shell's prefs observer puts it on the theme).
pub fn appearance(shared: &Shared, palette: Palette, cx: &mut Context<Root>) -> Div {
    let back = cx.listener(|root, _, _, cx| root.pop_page(cx));
    let now = shared.prefs.read(cx).get().appearance;
    let system = format!("Light and dark come from {}'s setting.", crate::os::SYSTEM);
    let choices = [
        (
            Appearance::System,
            "appearance/system",
            "icons/sun-moon.svg",
            "System",
        ),
        (
            Appearance::Light,
            "appearance/light",
            "icons/sun.svg",
            "Light",
        ),
        (
            Appearance::Dark,
            "appearance/dark",
            "icons/moon.svg",
            "Dark",
        ),
    ]
    .map(|(mode, id, icon, title)| {
        let words = (mode == Appearance::System).then(|| caption(system.clone(), palette));
        let chosen = mode == now;
        let mut choice = row(id, icon, title, words, palette)
            .control(Role::RadioButton, title)
            .aria_toggled(chosen.into())
            .debug_selector(move || id.to_owned())
            .active(|style| style.opacity(look::PRESSED))
            .on_click(cx.listener(move |root, _, _, cx| {
                root.shared
                    .prefs
                    .update(cx, |prefs, cx| prefs.set_appearance(mode, cx));
            }));
        if chosen {
            choice = choice.child(
                svg()
                    .path("icons/check.svg")
                    .size(px(look::ICON_ROW))
                    .flex_none()
                    .text_color(palette.foreground()),
            );
        }
        choice
    });
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            look::header("appearance/back", back, palette).child(
                div().flex_1().min_w_0().px(px(look::gap::SM)).child(
                    look::text(look::TITLE, FontWeight::MEDIUM, palette.foreground())
                        .truncate()
                        .child("Appearance"),
                ),
            ),
        )
        .child(
            div()
                .id("appearance")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(about::section("Light and dark", None, palette))
                .children(choices)
                .child(
                    look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted())
                        .px(px(look::GUTTER))
                        .py(px(look::gap::LG))
                        .child("Every program follows it too."),
                ),
        )
}

/// The appearance chosen, as App settings' row says it.
fn chosen_words(appearance: Appearance) -> String {
    match appearance {
        Appearance::System => format!("System: as {} is set", crate::os::SYSTEM),
        Appearance::Light => "Always light".to_owned(),
        Appearance::Dark => "Always dark".to_owned(),
    }
}

/// The Expo `Switch` (kit `Toggle.tsx`): a 40 × 24 pill, `primary` when
/// on and `border_strong` off, its 18 knob in `background` at the left or
/// the right; the row around it is the press.
fn toggle(on: bool, palette: Palette) -> Div {
    let ground = match on {
        true => palette.primary(),
        false => palette.border_strong(),
    };
    div()
        .relative()
        .w(px(40.))
        .h(px(24.))
        .flex_none()
        .rounded(px(12.))
        .bg(ground)
        .child(
            div()
                .absolute()
                .top(px(3.))
                .left(px(if on { 19. } else { 3. }))
                .size(px(18.))
                .rounded(px(9.))
                .bg(palette.background()),
        )
}

/// A row's chevron (Expo `Row`'s `chevron`): the row pushes a page.
pub(super) fn chevron(palette: Palette) -> gpui_kit::Svg {
    svg()
        .path("icons/chevron-right.svg")
        .size(px(look::ICON_ROW))
        .flex_none()
        .text_color(palette.faint())
}

/// A list row (Expo `Row`): the 24 lead column with its 18 muted icon, the
/// title over its caption, then whatever the caller puts at the right; 56
/// with a caption (48 without), 6 above and below, the gutter at the sides,
/// a hairline under.
pub(super) fn row(
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
        .min_h(px(if tall { look::ROW_TALL } else { look::ROW }))
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

/// A row's caption: 12 muted, one line.
pub(super) fn caption(text: impl Into<SharedString>, palette: Palette) -> Div {
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
        .h(px(look::BADGE))
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
