//! Add a device (the desktop's "Add a device…", Expo
//! `app/account/approve.tsx`), a sheet over the Account tab: the code the
//! new device shows, typed; the joining key's fingerprint, to compare;
//! this phone's yes, the consent the host signs for the account and hands
//! the new device over the relay. The host runs the flow
//! (`Account::approve_find`, `approve_confirm`); the sheet draws what it
//! holds, and the field owns the code until Next hands it over, as the
//! desktop's dialog does. The sheet going, however, forgets what it found
//! (`Root::set_sheet`); an approval landing closes it, and the host's
//! notice says so.

use super::join::{self, Kind};
use super::look::{self, Palette};
use super::{Root, sheet};
use ducktape_host::a11y::Control as _;
use ducktape_host::session::{AccountState, Shared};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Autocapitalize, Context, Div, Entity, FontWeight,
    InteractiveElement as _, ParentElement as _, Role, StatefulInteractiveElement as _,
    Styled as _, Subscription, TextInputConfiguration, Window, div, px,
};

/// The code field and what its keys do: a keystroke takes the host's
/// refusal down, return is Next, focus raises the keyboard.
pub fn field(window: &mut Window, cx: &mut Context<Root>) -> (Entity<InputState>, Subscription) {
    let code = cx.new(|cx| InputState::new(window, cx).text_input(keyboard()));
    let heard = cx.subscribe_in(
        &code,
        window,
        |root, code, event: &InputEvent, window, cx| match event {
            InputEvent::Change => root
                .shared
                .account
                .update(cx, |account, cx| account.clear_error(cx)),
            InputEvent::PressEnter { .. } => find(&root.shared, code, cx),
            InputEvent::Focus => join::keyboard(window, Some(keyboard())),
            InputEvent::Blur => join::keyboard(window, None),
        },
    );
    (code, heard)
}

fn keyboard() -> TextInputConfiguration {
    TextInputConfiguration {
        autocapitalize: Autocapitalize::Characters,
        ..TextInputConfiguration::default()
    }
}

fn find(shared: &Shared, code: &Entity<InputState>, cx: &mut App) {
    let typed = code.read(cx).value().to_string();
    shared
        .account
        .update(cx, |account, cx| account.approve_find(typed, cx));
}

/// The sheet's content, showing `account` (the host's, or what it held as
/// the sheet went): its title; the code step until the host found the
/// joining key, then its fingerprint and Approve; Cancel at the foot.
pub fn content(
    shared: &Shared,
    account: &AccountState,
    code: &Entity<InputState>,
    focused: bool,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Div {
    let busy = account.busy;
    let problem = (!account.error.is_empty()).then(|| account.error.clone());
    let said = |words: &'static str| {
        look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted()).child(words)
    };
    let body = div()
        .flex()
        .flex_col()
        .gap(px(look::gap::LG))
        .pt(px(look::gap::BLOCK))
        .px(px(look::SHEET_INSET))
        .pb(px(look::gap::SM));
    let body = match account.approve.clone() {
        None => {
            let mut next = join::button(
                "approve/next",
                "Next".to_owned(),
                Kind::Primary,
                None,
                busy,
                palette,
            );
            if !busy {
                let (shared, code) = (shared.clone(), code.clone());
                next = next.on_click(move |_, _, cx| find(&shared, &code, cx));
            }
            let under = problem.clone().map(|problem| {
                look::text(look::CAPTION, FontWeight::NORMAL, palette.danger())
                    .mt(px(look::gap::XS))
                    .id("approve/problem")
                    .control(Role::Alert, problem.clone())
                    .debug_selector(|| "approve/problem".to_owned())
                    .child(problem)
            });
            body.child(said(
                "On the new device, choose \"Add this device from another device\" (on a phone, \
                 \"Add this phone from another device\"). Type the code it shows.",
            ))
            .child(
                join::field(
                    code,
                    "approve/code",
                    "Code",
                    true,
                    focused,
                    problem.is_some(),
                    palette,
                )
                .children(under),
            )
            .child(next.w_full())
        }
        Some(fingerprint) => {
            let mut approve = join::button(
                "approve/confirm",
                "Approve".to_owned(),
                Kind::Primary,
                None,
                busy,
                palette,
            );
            if !busy {
                let account = shared.account.clone();
                approve = approve.on_click(move |_, _, cx| {
                    account.update(cx, |account, cx| account.approve_confirm(cx));
                });
            }
            let boxed = div()
                .flex()
                .flex_col()
                .gap(px(look::gap::LG))
                .p(px(look::gap::BLOCK))
                .border_1()
                .border_color(palette.border_strong())
                .child(
                    look::text(look::EYEBROW, FontWeight::MEDIUM, palette.muted())
                        .font_family(ducktape_host::fonts::FAMILY_MONO)
                        .child("KEY FINGERPRINT"),
                )
                .child(
                    look::text(look::DISPLAY, FontWeight::MEDIUM, palette.foreground())
                        .id("approve/fingerprint")
                        .control(Role::Label, fingerprint.clone())
                        .debug_selector(|| "approve/fingerprint".to_owned())
                        .font_family(ducktape_host::fonts::FAMILY_MONO)
                        .child(fingerprint),
                );
            body.child(said(
                "Approve only if the new device shows these same characters. It can then write \
                 as your account.",
            ))
            .child(boxed)
            .children(problem.map(|problem| join::notice("approve/failure", problem, palette)))
            .child(approve.w_full())
        }
    };
    let cancel = join::button(
        "approve/cancel",
        "Cancel".to_owned(),
        Kind::Quiet,
        None,
        false,
        palette,
    )
    .on_click(cx.listener(|root, _, _, cx| root.close_sheet(cx)));
    div()
        .flex()
        .flex_col()
        .child(sheet::title("Add a device", palette))
        .child(body.child(div().flex().justify_center().child(cancel)))
}
