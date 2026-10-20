//! The page's context menu. CEF says what a right-click offers there
//! (`run_context_menu`: its model); the page keeps what this app allows and
//! draws it with the kit's popup menu, the one the app's own text fields
//! show on a right-click. A choice goes back through CEF's callback
//! (`cont`); a menu closed without one cancels it.

use std::cell::Cell;
use std::os::raw::c_int;
use std::rc::Rc;

use cef::rc::Rc as _;
use cef::sys::cef_menu_id_t::*;
use cef::{
    Browser, CefString, ContextMenuHandler, ContextMenuParams, EventFlags, Frame, ImplBrowser as _,
    ImplContextMenuHandler, ImplContextMenuParams as _, ImplMenuModel as _,
    ImplRunContextMenuCallback as _, MenuItemType, MenuModel, RunContextMenuCallback,
    WrapContextMenuHandler,
};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, DismissEvent, Entity, Focusable as _,
    IntoElement as _, ParentElement as _, Pixels, Point, Subscription, Window, anchored, deferred,
    point, px,
};

use crate::page::{Page, Shared};

const KEPT: [cef::sys::cef_menu_id_t; 7] = [
    MENU_ID_BACK,
    MENU_ID_FORWARD,
    MENU_ID_RELOAD,
    MENU_ID_CUT,
    MENU_ID_COPY,
    MENU_ID_PASTE,
    MENU_ID_SELECT_ALL,
];

#[derive(Debug, PartialEq)]
enum Row {
    Separator,
    Command {
        id: c_int,
        label: String,
        enabled: bool,
    },
    Reload,
    CopyLink(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Entry<'a> {
    Separator,
    Command(c_int, &'a str, bool),
}

fn rows(model: &[Entry], link: &str) -> Vec<Row> {
    let kept = |id: c_int| KEPT.iter().any(|&kept| kept as c_int == id);
    let reload = !model
        .iter()
        .any(|entry| matches!(entry, Entry::Command(id, ..) if *id == MENU_ID_RELOAD as c_int));
    let mut rows = Vec::new();
    if !link.is_empty() {
        rows.extend([Row::CopyLink(link.to_owned()), Row::Separator]);
    }
    for &entry in model {
        match entry {
            Entry::Separator => rows.push(Row::Separator),
            Entry::Command(id, label, enabled) if kept(id) => {
                rows.push(Row::Command {
                    id,
                    label: label
                        .replace("&&", "\0")
                        .replace('&', "")
                        .replace('\0', "&"),
                    enabled,
                });
                if reload && id == MENU_ID_FORWARD as c_int {
                    rows.push(Row::Reload);
                }
            }
            Entry::Command(..) => {}
        }
    }
    let mut tidy: Vec<Row> = Vec::with_capacity(rows.len());
    for row in rows {
        let separator = row == Row::Separator;
        if !(separator && tidy.last().is_none_or(|last| *last == Row::Separator)) {
            tidy.push(row);
        }
    }
    if tidy.last() == Some(&Row::Separator) {
        tidy.pop();
    }
    tidy
}

struct Answer(Cell<Option<RunContextMenuCallback>>);

impl Answer {
    fn choose(&self, id: c_int) {
        if let Some(callback) = self.0.take() {
            callback.cont(id, EventFlags::default());
        }
    }

    fn cancel(&self) {
        if let Some(callback) = self.0.take() {
            callback.cancel();
        }
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(crate) struct Asked {
    at: (i32, i32),
    rows: Vec<Row>,
    answer: Rc<Answer>,
}

pub(crate) struct Open {
    menu: Entity<PopupMenu>,
    at: Point<Pixels>,
    answer: Rc<Answer>,
    _closed: Subscription,
}

impl Open {
    pub(crate) fn has_keys(&self, window: &Window, cx: &App) -> bool {
        self.menu.focus_handle(cx).contains_focused(window, cx)
    }

    pub(crate) fn element(&self) -> AnyElement {
        deferred(
            anchored()
                .position(self.at)
                .snap_to_window_with_margin(px(8.))
                .child(self.menu.clone()),
        )
        .with_priority(gpui_kit::base::POPUP_PRIORITY)
        .into_any_element()
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.answer.cancel();
    }
}

pub(crate) fn open(
    asked: Asked,
    shared: &Rc<Shared>,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<Page>,
) -> Open {
    let Asked { at, rows, answer } = asked;
    let menu = PopupMenu::build(window, cx, |mut menu, _, _| {
        for row in rows {
            let (answer, shared) = (answer.clone(), shared.clone());
            menu = match row {
                Row::Separator => menu.separator(),
                Row::Command { id, label, enabled } => menu.item(
                    PopupMenuItem::new(label)
                        .disabled(!enabled)
                        .on_click(move |_, _, _| answer.choose(id)),
                ),
                Row::Reload => menu.item(PopupMenuItem::new("Reload").on_click(move |_, _, _| {
                    if let Some(browser) = shared.browser.borrow().as_ref() {
                        browser.reload();
                    }
                })),
                Row::CopyLink(link) => menu.item(PopupMenuItem::new("Copy link address").on_click(
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                    },
                )),
            };
        }
        menu
    });
    let _closed = cx.subscribe(&menu, |page, _, _: &DismissEvent, cx| page.menu_closed(cx));
    let focus = menu.focus_handle(cx);
    cx.defer_in(window, move |_, window, cx| window.focus(&focus, cx));
    Open {
        menu,
        at: origin + point(px(at.0 as f32), px(at.1 as f32)),
        answer,
        _closed,
    }
}

cef::wrap_context_menu_handler! {
    pub(crate) struct PageMenu {
        shared: Rc<Shared>,
    }

    impl ContextMenuHandler {
        fn run_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> c_int {
            let (Some(params), Some(model), Some(callback)) = (params, model, callback) else {
                return 0;
            };
            let labels: Vec<String> = (0..model.count())
                .map(|index| CefString::from(&model.label_at(index)).to_string())
                .collect();
            let entries: Vec<Entry> = (0..model.count())
                .map(|index| match model.type_at(index) == MenuItemType::SEPARATOR {
                    true => Entry::Separator,
                    false => Entry::Command(
                        model.command_id_at(index),
                        &labels[index],
                        model.is_enabled_at(index) == 1,
                    ),
                })
                .collect();
            let link = CefString::from(&params.link_url()).to_string();
            let rows = rows(&entries, &link);
            if rows.is_empty() {
                callback.cancel();
                return 1;
            }
            let answer = Rc::new(Answer(Cell::new(Some(callback.clone()))));
            *self.shared.menu.borrow_mut() = Some(Asked {
                at: (params.xcoord(), params.ycoord()),
                rows,
                answer,
            });
            self.shared.dirty.set(true);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Row, rows};
    use cef::sys::cef_menu_id_t::*;
    use std::os::raw::c_int;

    fn command(id: cef::sys::cef_menu_id_t, label: &str) -> Entry<'_> {
        Entry::Command(id as c_int, label, true)
    }

    #[test]
    fn a_page_menu_keeps_what_the_app_allows() {
        let page = [
            command(MENU_ID_BACK, "&Back"),
            command(MENU_ID_FORWARD, "&Forward"),
            Entry::Separator,
            command(MENU_ID_PRINT, "&Print..."),
            command(MENU_ID_VIEW_SOURCE, "View s&ource"),
        ];
        let shown = rows(&page, "https://example.com/");
        let label = |id: cef::sys::cef_menu_id_t, label: &str| Row::Command {
            id: id as c_int,
            label: label.into(),
            enabled: true,
        };
        assert_eq!(
            shown,
            [
                Row::CopyLink("https://example.com/".into()),
                Row::Separator,
                label(MENU_ID_BACK, "Back"),
                label(MENU_ID_FORWARD, "Forward"),
                Row::Reload,
            ]
        );
        let field = [
            command(MENU_ID_UNDO, "&Undo"),
            command(MENU_ID_REDO, "&Redo"),
            Entry::Separator,
            command(MENU_ID_CUT, "Cu&t"),
            command(MENU_ID_COPY, "&Copy"),
            command(MENU_ID_PASTE, "&Paste"),
            command(MENU_ID_DELETE, "&Delete"),
            Entry::Separator,
            command(MENU_ID_SELECT_ALL, "Select &all"),
            Entry::Separator,
            command(MENU_ID_SPELLCHECK_SUGGESTION_0, "fox"),
        ];
        let shown = rows(&field, "");
        let labels: Vec<_> = shown
            .iter()
            .map(|row| match row {
                Row::Command { label, .. } => label.as_str(),
                Row::Separator => "-",
                _ => "?",
            })
            .collect();
        assert_eq!(labels, ["Cut", "Copy", "Paste", "-", "Select all"]);
        assert!(rows(&[command(MENU_ID_PRINT, "&Print...")], "").is_empty());
    }
}
