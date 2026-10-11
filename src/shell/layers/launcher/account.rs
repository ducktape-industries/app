//! The launcher's account screens: name a new account, or add this device
//! to one that exists through a passkey (`passkey_waiting`) or another
//! device (`link_waiting`). See launcher.rs for key vs account.

use super::super::super::entities::Account;
use super::super::super::ink::{self, *};
use super::super::fields::TextField;
use super::{LauncherLayer, LauncherScreen, buttons, closing};
use crate::a11y::Control as _;
use gpui_kit::*;

/// What the device that waits for approval is told: a security instruction,
/// so a `Status` a screen reader hears.
const LINK_WAITING: &str = "On a device already signed in, open the account menu, choose \"Add a device…\" and type the code. Approve there only if it shows the same four-and-four. The code lasts five minutes.";

impl LauncherLayer {
    /// CreateAccount: name the account this key signs for, or add this
    /// device to one that exists.
    pub(super) fn account_step(&self, window: &mut Window, cx: &App) -> AnyElement {
        let account = self.account.read(cx).get();
        if account.passkey_waiting {
            return self.passkey_waiting(window, cx);
        }
        if !account.link_code.is_empty() {
            return self.link_waiting(window, cx);
        }
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let name = self.fields.name.input(
            "create-account-name",
            TextField {
                label: "Account name".into(),
                private: false,
                error: (!account.error.is_empty()).then(|| account.error.clone()),
                size: 22.,
            },
            cx,
        );
        let focused = self.fields.name.focused(window, cx);
        let below = match account.error.is_empty() {
            true => ink::note(
                "account-name-note",
                "The account number is given when it's created. Change the name later in Account.",
                ink.muted,
            )
            .into_any_element(),
            false => alert("create-account-error", account.error.clone(), &ink),
        };
        let busy = account.busy;
        let form = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(field(
                "account-name-label",
                "Name",
                sans(400, 22.)
                    .child(
                        field_box(name, focused, ink.field, 56., &ink).text_size(px(ink::fit(22.))),
                    )
                    .into_any_element(),
                Some(below),
                &ink,
            ))
            .child(
                buttons([
                    ink::button(
                        "create-account",
                        match busy {
                            true => "Creating…",
                            false => "Create account",
                        },
                        Kind::Primary,
                        self.with_field(&self.fields.name, Account::create_submit),
                        Press::busy(busy),
                        &ink,
                    ),
                    ink::button(
                        "passkey-create",
                        "Create with a passkey",
                        Kind::Secondary,
                        self.with_field(&self.fields.name, Account::passkey_create),
                        busy,
                        &ink,
                    ),
                    link_running(
                        "create-account-later",
                        "Not now",
                        self.on_account(Account::create_later),
                        true,
                        &ink,
                    ),
                ])
                .items_center(),
            );
        // The common way in gets its own line; the other two share one.
        let join = closing(
            [
                sans(500, 14.)
                    .child(words(
                        "join-question",
                        format!(
                            "Already have an account on {}?",
                            self.session.read(cx).get().network
                        ),
                    ))
                    .into_any_element(),
                link_running(
                    "link-device",
                    "Add this device from another device",
                    self.on_account(Account::link_start),
                    false,
                    &ink,
                ),
                div()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap(px(5.))
                    .child(ink::note("join-or-use", "Or use", ink.muted))
                    .child(link_running(
                        "passkey-sign-in",
                        "a passkey",
                        self.on_account(Account::passkey_sign_in),
                        true,
                        &ink,
                    ))
                    .child(ink::note("join-or", "or", ink.muted))
                    .child(link_running(
                        "recover",
                        "a recovery key",
                        self.on_account(Account::recover_show),
                        true,
                        &ink,
                    ))
                    .into_any_element(),
            ],
            &ink,
        )
        .id("join-account")
        .role(Role::Group)
        .aria_label("Already have an account");
        self.frame(
            LauncherScreen {
                id: "account-step",
                tight: false,
                caption: self.node_caption(cx),
                back: None,
                label: "[03 / 04] Account".into(),
                headline: "What should people call you?".into(),
                lead: Some(format!(
                "Your account is the name beside everything you write on {}. This device's key signs for it.",
                self.session.read(cx).get().network
            )),
                body: vec![form.into_any_element(), join.into_any_element()],
            },
            window,
            cx,
        )
    }

    /// Passkey: the ceremony is in the browser, or on a phone through the QR.
    fn passkey_waiting(&self, window: &mut Window, cx: &App) -> AnyElement {
        let account = self.account.read(cx).get();
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let (title, hint) = match account.passkey_qr {
            Some(_) => (
                "Scan with your phone",
                "Point its camera at the code. Show the code and its link only to your own phone. It asks for your passkey twice; this screen moves on by itself.",
            ),
            None => (
                "Continue in your browser",
                "Your browser asks for your passkey twice. This screen moves on by itself.",
            ),
        };
        let row = account.passkey_qr.as_ref().map(|url| {
            let copy = {
                let url = url.clone();
                crate::a11y::keyboard(
                    sans(400, 14.)
                        .id("passkey-qr-copy")
                        .control(Role::Button, "Copy the link instead")
                        .underline()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                        })
                        .child("Copy the link instead"),
                    ink.ink,
                )
            };
            div()
                .flex()
                .items_start()
                .gap(px(20.))
                .child(
                    div()
                        .id("passkey-qr")
                        .role(Role::Image)
                        .aria_label("Passkey QR code")
                        .size(px(168.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border(px(1.5))
                        .border_color(ink.ink)
                        .bg(gpui_kit::white())
                        .child(crate::render::qr(&view_wire::Qr {
                            payload: Some(url.clone().into_bytes()),
                            size: Some(view_wire::QrSize::Total(150.)),
                            ..Default::default()
                        })),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(10.))
                        // a tag's look; its word is the live region's own
                        .child(
                            crate::a11y::live(
                                mono(400, 12.)
                                    .text_color(ink.muted)
                                    .id("passkey-waiting-status")
                                    .role(Role::Status),
                                accesskit::Live::Polite,
                                "Waiting",
                            )
                            .child("Waiting"),
                        )
                        .child(sans(500, 16.).child(words("passkey-twice", "Your passkey, twice")))
                        .child(ink::note(
                            "passkey-second",
                            "A new code appears here for the second time.",
                            ink.muted,
                        ))
                        .child(copy),
                )
                .into_any_element()
        });
        let links = closing(
            [
                match account.passkey_qr {
                    Some(_) => None,
                    None => Some(link_running(
                        "passkey-use-phone",
                        "Use a phone instead",
                        self.on_account(Account::passkey_use_phone),
                        false,
                        &ink,
                    )),
                },
                Some(link_running(
                    "passkey-cancel",
                    "Cancel",
                    self.on_account(Account::passkey_cancel),
                    false,
                    &ink,
                )),
            ]
            .into_iter()
            .flatten(),
            &ink,
        );
        self.frame(
            LauncherScreen {
                id: "passkey-waiting",
                tight: false,
                caption: self.node_caption(cx),
                back: None,
                label: "[03 / 04] Account · passkey".into(),
                headline: title.into(),
                lead: Some(hint.into()),
                body: row.into_iter().chain([links.into_any_element()]).collect(),
            },
            window,
            cx,
        )
    }

    /// This device waits under a short code for one already on the account
    /// to approve it; the fingerprint is compared on both screens.
    fn link_waiting(&self, window: &mut Window, cx: &App) -> AnyElement {
        let account = self.account.read(cx).get();
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let fingerprint = crate::backend::hex_decode(&account.signer_key)
            .map(|key| crate::backend::join::fingerprint(&key))
            .unwrap_or_default();
        let big = |id: &'static str, name: &'static str, text: String| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(tag(SharedString::from(format!("{id}-tag")), name, &ink))
                .child(crate::a11y::whole(
                    mono(400, 28.)
                        .id(id)
                        .role(Role::Label)
                        .aria_label(text.clone())
                        .child(text),
                ))
        };
        let body = vec![
            div()
                .flex()
                .gap(px(40.))
                .child(big("link-code", "Code", account.link_code.clone()))
                .child(big("link-fingerprint", "This device", fingerprint))
                .into_any_element(),
            // a note's look; its words are the live region's own
            crate::a11y::live(
                sans(400, 13.)
                    .line_height(px(13. * 1.55))
                    .text_color(ink.muted)
                    .id("link-waiting-status")
                    .role(Role::Status),
                accesskit::Live::Polite,
                LINK_WAITING,
            )
            .child(LINK_WAITING)
            .into_any_element(),
            div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .children(
                    (!account.error.is_empty())
                        .then(|| alert("link-error", account.error.clone(), &ink)),
                )
                .into_any_element(),
        ];
        self.frame(
            LauncherScreen {
                id: "link-waiting",
                tight: false,
                caption: self.node_caption(cx),
                // the step back reads as recovery's: the same way to the account
                back: Some((
                    "link-back",
                    "← Back",
                    Box::new(self.on_account(Account::link_cancel)),
                )),
                label: "[03 / 04] Account · another device".into(),
                headline: "Approve this device".into(),
                lead: None,
                body,
            },
            window,
            cx,
        )
    }
}
