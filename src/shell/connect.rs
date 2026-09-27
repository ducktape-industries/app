//! The Connect screen, first in the launcher: a node address, the nodes
//! reached before, and why the last try did not land.

use super::launcher::LauncherScreen;
use super::text_field::TextField;
use super::*;

impl DesktopWindow {
    /// Main (Connect): an address, the nodes reached before, and why the
    /// last try did not land.
    pub(super) fn connect(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::{self, *};
        use gpui_kit::*;
        let state = self.model.read(cx).state.facts();
        let ink = Ink::of(state.dark);
        let field = self.input(
            TextField {
                key: "endpoint",
                placeholder: "127.0.0.1:8844",
                masked: false,
                value: |state| &state.endpoint,
                on_change: Message::EndpointTyped,
                on_enter: || Message::ConnectSubmit,
                label: Some("Node address".into()),
                private: false,
                size: 15.,
            },
            window,
            cx,
        );
        // The address refusal is about what is typed now; a failed try is
        // about the last address. Both clear on the next keystroke or try.
        let note = [&state.endpoint_error, &state.error]
            .into_iter()
            .find(|note| !note.is_empty())
            .cloned();
        let border = match note {
            Some(_) => ink.danger,
            None => ink.strong,
        };
        // Only a try in flight has a status worth reading.
        let below = match (&note, state.connecting) {
            (Some(note), _) => Some(self.alert("connect-error", note.clone(), &ink)),
            (None, true) => Some(
                ink::note(state.status.clone(), ink.muted)
                    .id("connect-status")
                    .role(Role::Status)
                    .aria_label(state.status.clone())
                    .into_any_element(),
            ),
            (None, false) => None,
        };
        let form = self.field(
            "Node address",
            div()
                .flex()
                .gap(px(8.))
                .child(div().flex_1().child(
                    field_box(field, border, 44., &ink).font_family(super::theme::FAMILY_MONO),
                ))
                .child(self.button(
                    "connect",
                    "Connect",
                    Kind::Primary,
                    || Message::ConnectSubmit,
                    state.connecting,
                    &ink,
                ))
                .into_any_element(),
            below,
            &ink,
        );
        let rows = state.recent_endpoints.iter().map(|entry| {
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
                .child(crate::a11y::keyboard(pick))
                .child(crate::a11y::keyboard(forget))
        });
        let recent = (!state.recent_endpoints.is_empty()).then(|| {
            div()
                .id("recent")
                .flex()
                .flex_col()
                .child(
                    tag("Recent", &ink)
                        .pb(px(8.))
                        .border_b_1()
                        .border_color(ink.line),
                )
                .children(rows)
                .into_any_element()
        });
        let caption = match state.recent_endpoints.is_empty() {
            true => "No network yet.",
            false => "A node, drawn in characters.",
        };
        self.launcher(
            LauncherScreen {
                id: "connect",
                tight: false,
                figure: figure::Figure::Roll,
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
