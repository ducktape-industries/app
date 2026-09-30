//! The Connect screen, first in the launcher: a node address, the nodes
//! reached before, and why the last try did not land.

use super::super::super::ink::*;
use super::super::super::{Message, theme};
use super::super::fields::TextField;
use super::{LauncherLayer, LauncherScreen};
use crate::a11y::Control as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

impl LauncherLayer {
    /// Main (Connect): an address, the nodes reached before, and why the
    /// last try did not land.
    pub(super) fn connect(&self, window: &mut Window, cx: &App) -> AnyElement {
        let session = self.session.read(cx).get();
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        // The address refusal is about what is typed now; a failed try is
        // about the last address. Both clear on the next keystroke or try.
        let note = [&session.endpoint_error, &session.error]
            .into_iter()
            .find(|note| !note.is_empty())
            .cloned();
        let input = self.fields.endpoint.input(
            "endpoint",
            TextField {
                label: "Node address".into(),
                private: false,
                error: note.clone(),
                size: 15.,
            },
            cx,
        );
        let focused = self.fields.endpoint.focused(window, cx);
        let border = match note {
            Some(_) => ink.danger,
            None => ink.field,
        };
        // Only a try in flight has a status worth reading: the address it
        // reaches for (on this screen no node is in hand).
        let below = match (&note, session.connecting) {
            (Some(note), _) => Some(alert("connect-error", note.clone(), &ink)),
            (None, true) => {
                let status = format!("Reaching {}…", session.endpoint);
                Some(
                    // a note's look; its words are the live region's own
                    crate::a11y::live(
                        sans(400, 13.)
                            .line_height(px(13. * 1.55))
                            .text_color(ink.muted)
                            .id("connect-status")
                            .role(Role::Status),
                        accesskit::Live::Polite,
                        status.clone(),
                    )
                    .child(status)
                    .into_any_element(),
                )
            }
            (None, false) => None,
        };
        let form = field(
            "endpoint-label",
            "Node address",
            div()
                .flex()
                .gap(px(8.))
                .child(div().flex_1().child(
                    field_box(input, focused, border, 44., &ink).font_family(theme::FAMILY_MONO),
                ))
                .child(self.button(
                    "connect",
                    "Connect",
                    Kind::Primary,
                    || Message::ConnectSubmit,
                    session.connecting,
                    &ink,
                ))
                .into_any_element(),
            below,
            &ink,
        );
        let rows = session.recent_endpoints.iter().map(|entry| {
            let pick_model = self.model.clone();
            let pick_target = entry.url.clone();
            let forget_model = self.model.clone();
            let forget_target = entry.url.clone();
            let name = entry.name();
            let danger = ink.danger;
            // `display: flex; align-items: baseline; gap: 16px;
            // padding: 12px 0; border-bottom: 1px solid line`
            let pick = div()
                .id(SharedString::from(format!("recent/{}", entry.url)))
                .control(Role::Button, SharedString::from(entry.label()))
                .cursor_pointer()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(px(16.))
                .hover(|style| style.opacity(0.7))
                .on_click(move |_, _, cx| {
                    let target = pick_target.clone();
                    pick_model.update(cx, |model, cx| {
                        model.dispatch(Message::ConnectTo(target), cx)
                    })
                })
                .child(
                    div()
                        .w(px(120.))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(sans(500, 15.).truncate().child(name))
                        .when(entry.other_chain, |cell| {
                            cell.child(sans(400, 12.).text_color(ink.muted).child("Other chain"))
                        }),
                )
                .child(
                    mono(400, 13.)
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ink.muted)
                        .child(entry.host().to_owned()),
                );
            let forget = div()
                .id(SharedString::from(format!("forget/{}", entry.url)))
                .control(
                    Role::Button,
                    SharedString::from(format!("Forget {}", entry.url)),
                )
                .cursor_pointer()
                .flex_shrink_0()
                .child(
                    sans(400, 16.)
                        .text_color(ink.muted)
                        .hover(move |style| style.text_color(danger))
                        .child("×"),
                )
                .on_click(move |_, _, cx| {
                    let target = forget_target.clone();
                    forget_model.update(cx, |model, cx| {
                        model.dispatch(Message::ForgetEndpoint(target), cx)
                    })
                });
            // Both have a tab stop: a keyboard-only reader reaches a node
            // used before, and can forget it.
            div()
                .flex()
                .items_baseline()
                .gap(px(16.))
                .py(px(12.))
                .border_b_1()
                .border_color(ink.line)
                .child(crate::a11y::keyboard(pick, ink.ink))
                .child(crate::a11y::keyboard(forget, ink.ink))
        });
        let recent = (!session.recent_endpoints.is_empty()).then(|| {
            div()
                .id("recent")
                .flex()
                .flex_col()
                .child(
                    tag("recent-title", "Recent", &ink)
                        .pb(px(8.))
                        .border_b_1()
                        .border_color(ink.line),
                )
                .children(rows)
                .into_any_element()
        });
        let caption = match session.recent_endpoints.is_empty() {
            true => "No network yet.",
            false => "A node, drawn in characters.",
        };
        self.frame(
            LauncherScreen {
                id: "connect",
                tight: false,
                caption: caption.into(),
                back: None,
                label: "[01 / 03] Network".into(),
                headline: "Connect to a network".into(),
                lead: Some("Any node on it will do. The node serves the programs you use and keeps your account.".into()),
                body: std::iter::once(form.into_any_element()).chain(recent).collect(),
            },
            window,
            cx,
        )
    }
}
