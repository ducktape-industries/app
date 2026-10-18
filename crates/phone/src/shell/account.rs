//! The Account tab (board `24-account-b`, the owner's pick of 2026-10-18):
//! one scrolled page of what the shell owns, nothing pushed over it but
//! the Account program. Its big title is the account's name by its avatar,
//! "account N · network" under it; then the Account program's row (keys,
//! agents and invites: only that program reads them), Add a device (while
//! this phone holds an account); This phone (the device, its key, where
//! the key is kept); Network (the one in hand, Switch network…);
//! Appearance (System, Light or Dark at a press, and debug mode's
//! switch); the app's version at the foot. Add a device and Switch network
//! open as sheets over it (`Sheet::AddDevice`, `Sheet::Switch`). The Expo
//! dev build's Relay section is not here: a phone build is never one.
//!
//! It reads the session (the node, the network), the chain (the network's
//! founding time), the account (its name, this phone's key), the prefs and
//! what the key store said for the keyring in hand; it writes two prefs,
//! the appearance and debug mode (the Expo build had neither: it followed
//! the system).

use super::look::{self, Palette};
use super::{ACCOUNT_PROGRAM, Root, about, home, join};
use ducktape_host::a11y::Control as _;
use ducktape_host::backend::Appearance;
use ducktape_host::contract::{KeyLevel, KeyStorage};
use ducktape_host::session::Shared;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div, px, svg,
};

/// The tab, `storage` what the key store said for the keyring in hand: its
/// big title, then the rows and sections, scrolled.
pub fn tab(
    shared: &Shared,
    storage: Option<&Result<KeyStorage, String>>,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Div {
    let session = shared.session.read(cx).get().clone();
    // Expo `you.tsx`: the account's name, else "No account yet"; the
    // avatar draws the name's initial, or none
    let me = home::me(shared, cx);
    let (face, name, under) = match me.clone() {
        Some((number, name)) => (
            name.clone(),
            name,
            format!("account {number} · {}", session.network),
        ),
        None => (
            String::new(),
            "No account yet".to_owned(),
            session.network.clone(),
        ),
    };
    // the tab roots' 56 header (Expo `Header` with `root`), the avatar in
    // its hit at the left
    let header = div()
        .h(px(look::HEADER))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(look::gap::XXS))
        .px(px(look::gap::SM))
        .child(
            div()
                .size(px(look::HIT))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(home::avatar(&face, 40., palette)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(px(look::gap::SM))
                .flex()
                .flex_col()
                .justify_center()
                .child(
                    look::text(look::BIG_TITLE, FontWeight::MEDIUM, palette.foreground())
                        .truncate()
                        .child(name),
                )
                .child(
                    caption(home::caption(shared, under, cx), palette)
                        .font_family(ducktape_host::fonts::FAMILY_MONO)
                        .truncate(),
                ),
        );
    let opens = |row: Stateful<Div>, title: &'static str| {
        row.control(Role::Button, title)
            .active(|style| style.opacity(look::PRESSED))
            .child(chevron(palette))
    };
    let program = home::row_of(shared, ACCOUNT_PROGRAM, cx).map(|_| {
        let said = "Keys, agents and invites, in the Account program.";
        let keys = row(
            "account/program",
            "icons/key-round.svg",
            "Keys and agents",
            Some(caption(said, palette)),
            palette,
        );
        opens(keys, "Keys and agents").on_click(cx.listener(|root, _, _, cx| {
            root.open(ACCOUNT_PROGRAM, cx);
        }))
    });
    // Expo's row under the Keys list, in the `programs` view here, which
    // the shell does not draw into: the desktop's account menu holds it too
    let add = me.map(|_| {
        let said = "Approve a new device by the code it shows.";
        let add = row(
            "account/add-device",
            "icons/smartphone.svg",
            "Add a device",
            Some(caption(said, palette)),
            palette,
        );
        opens(add, "Add a device")
            .on_click(cx.listener(|root, _, window, cx| root.show_approve(window, cx)))
    });
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
        "account/phone",
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
        "account/key-storage",
        "icons/lock.svg",
        "Key storage",
        Some(caption(kept, palette)),
        palette,
    );
    if ok {
        storage = storage.child(badge("OK", palette));
    }
    let network = match session.connected_rpc.is_empty() {
        true => row(
            "account/network",
            "icons/server.svg",
            "Not connected",
            None,
            palette,
        ),
        false => {
            let host = join::host_of(&session.connected_rpc);
            let founded = shared.chain.read(cx).node.as_ref().map(|node| node.time);
            let said = match founded {
                Some(founded) => format!("{host} · founded {}", day(founded)),
                None => host.to_owned(),
            };
            row(
                "account/network",
                "icons/server.svg",
                session.network.clone(),
                Some(caption(said, palette).font_family(ducktape_host::fonts::FAMILY_MONO)),
                palette,
            )
        }
    };
    let switch = row(
        "account/switch",
        "icons/refresh-cw.svg",
        "Switch network…",
        Some(caption(
            "Keeps this phone's key; join the other network by code.",
            palette,
        )),
        palette,
    );
    let switch = opens(switch, "Switch network…")
        .on_click(cx.listener(|root, _, window, cx| root.show_switch(window, cx)));
    let on = shared.prefs.read(cx).get().debug;
    let debug = row(
        "account/debug",
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
    .active(|style| style.opacity(look::PRESSED))
    .child(toggle(on, palette))
    .on_click(cx.listener(move |root, _, _, cx| {
        root.shared
            .prefs
            .update(cx, |prefs, cx| prefs.set_debug(!on, cx));
    }));
    // the boards' foot: the version alone, centred, in the data face
    let version = look::text(look::CAPTION, FontWeight::NORMAL, palette.faint())
        .id("account/version")
        .debug_selector(|| "account/version".to_owned())
        .font_family(ducktape_host::fonts::FAMILY_MONO)
        .text_center()
        .pt(px(20.))
        .pb(px(look::gap::XL))
        .px(px(look::GUTTER))
        .child(format!("ducktape mobile {}", env!("CARGO_PKG_VERSION")));
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(header)
        .child(
            div()
                .id("account")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(program)
                .children(add)
                .child(about::section("This phone", None, palette))
                .child(phone)
                .child(storage)
                .child(about::section("Network", None, palette))
                .child(network)
                .child(switch)
                .child(about::section("Appearance", None, palette))
                .child(appearance(shared, palette, cx))
                .child(debug)
                .child(version),
        )
}

/// The appearance, inline (the boards' segmented control): System, Light
/// and Dark side by side in one 36 box, the one in force filled. A press
/// saves it, and the app, every program and the system bars take it at
/// once (the shell's prefs observer puts it on the theme).
fn appearance(shared: &Shared, palette: Palette, cx: &mut Context<Root>) -> Div {
    let now = shared.prefs.read(cx).get().appearance;
    let choices = [
        (Appearance::System, "appearance/system", "System"),
        (Appearance::Light, "appearance/light", "Light"),
        (Appearance::Dark, "appearance/dark", "Dark"),
    ]
    .into_iter()
    .enumerate()
    .map(|(at, (mode, id, title))| {
        let chosen = mode == now;
        let (ink, weight) = match chosen {
            true => (palette.primary_foreground(), FontWeight::MEDIUM),
            false => (palette.muted(), FontWeight::NORMAL),
        };
        let mut choice = div()
            .id(id)
            .control(Role::RadioButton, title)
            .aria_toggled(chosen.into())
            .debug_selector(move || id.to_owned())
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .active(|style| style.opacity(look::PRESSED))
            .child(look::text(look::SECONDARY, weight, ink).child(title))
            .on_click(cx.listener(move |root, _, _, cx| {
                root.shared
                    .prefs
                    .update(cx, |prefs, cx| prefs.set_appearance(mode, cx));
            }));
        if at > 0 {
            choice = choice.border_l_1().border_color(palette.border_strong());
        }
        if chosen {
            choice = choice.bg(palette.primary());
        }
        choice
    });
    div()
        .pt(px(look::gap::XXS))
        .pb(px(look::gap::LG))
        .px(px(look::GUTTER))
        .border_b_1()
        .border_color(palette.border())
        .child(
            div()
                .h(px(look::SMALL_BUTTON))
                .flex()
                .border_1()
                .border_color(palette.border_strong())
                .children(choices),
        )
        .child(
            look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted())
                .mt(px(look::gap::SM))
                .child(format!(
                    "Light and dark come from {}'s setting. Every program follows it too.",
                    crate::os::SYSTEM
                )),
        )
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

/// A row's chevron (Expo `Row`'s `chevron`): the row opens something.
fn chevron(palette: Palette) -> gpui_kit::Svg {
    svg()
        .path("icons/chevron-right.svg")
        .size(px(look::ICON_ROW))
        .flex_none()
        .text_color(palette.faint())
}

/// A list row (Expo `Row`), named `id` to the AX door: the 24 lead column
/// with its 18 muted icon, the title over its caption, then whatever the
/// caller puts at the right; 56 with a caption (48 without), 6 above and
/// below, the gutter at the sides, a hairline under.
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
        .debug_selector(move || id.to_owned())
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

/// A row's caption: 12 muted, wrapping as the boards' do (where the key
/// is kept runs to two lines).
fn caption(text: impl Into<SharedString>, palette: Palette) -> Div {
    look::text(look::CAPTION, FontWeight::NORMAL, palette.muted()).child(text.into())
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
