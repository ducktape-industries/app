//! The OS's own edit menu over a text field's touch selection on a
//! touch-first device: the host shuts the kit's drawn menu, tells the shell
//! where to show its own, draws a growing field's handles, applies a pick.

use super::*;
use crate::contract::{EditAction, EditMenu};
use gpui_base::{SelectionEdge, TouchHandle};
use gpui_kit::{
    AnyWindowHandle, EntityId, Hitbox, TouchDragEvent, TouchPhase, WeakEntity, deferred,
};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

struct Shown {
    field: EntityId,
    window: AnyWindowHandle,
    menu: EditMenu,
    engine: Held,
    wanted: Weak<Cell<bool>>,
}

enum Held {
    Line(WeakEntity<InputState>),
    Area(WeakEntity<TextareaState>),
}

impl Held {
    fn of(engine: &Engine) -> Self {
        match engine {
            Engine::Line(state) => Held::Line(state.downgrade()),
            Engine::Area(state) => Held::Area(state.downgrade()),
        }
    }

    fn upgrade(&self) -> Option<Engine> {
        match self {
            Held::Line(state) => state.upgrade().map(Engine::Line),
            Held::Area(state) => state.upgrade().map(Engine::Area),
        }
    }
}

struct Handle {
    edge: SelectionEdge,
    caret: Bounds<Pixels>,
    hitbox: Hitbox,
}

pub(super) fn take_over(engine: &Engine, wanted: &Cell<bool>, cx: &mut App) {
    match engine!(engine, |state| state.read(cx).touch_selection()) {
        Some(held) if held.dragging().is_some() => wanted.set(false),
        Some(held) if held.is_menu_open() => {
            wanted.set(true);
            engine!(engine, |state| state
                .update(cx, |state, cx| state.close_edit_menu(cx)));
        }
        Some(_) => {}
        None => wanted.set(false),
    }
}

pub(super) fn layer(engine: &Engine, wanted: &Rc<Cell<bool>>) -> AnyElement {
    let (laid, painted, wanted) = (engine.clone(), engine.clone(), wanted.clone());
    deferred(
        canvas(
            move |_, window, cx| match &laid {
                Engine::Area(state) => lay_out_handles(state, window, cx),
                Engine::Line(_) => Vec::new(),
            },
            move |_, handles, window, cx| {
                if let Engine::Area(state) = &painted {
                    paint_handles(&handles, window, cx);
                    follow_drags(&handles, state, window);
                }
                decide(&painted, &wanted, window, cx);
            },
        )
        .absolute()
        .size_0(),
    )
    .with_priority(gpui_base::POPUP_PRIORITY)
    .into_any_element()
}

fn lay_out_handles(
    state: &Entity<TextareaState>,
    window: &mut Window,
    cx: &mut App,
) -> Vec<Handle> {
    let Some(held) = state.read(cx).touch_selection() else {
        return Vec::new();
    };
    if held.is_empty() {
        return Vec::new();
    }
    let on_screen = Bounds::new(Point::default(), window.viewport_size());
    [SelectionEdge::Start, SelectionEdge::End]
        .into_iter()
        .filter(|edge| held.is_edge_visible(*edge))
        .map(|edge| (edge, held.edge(edge)))
        .filter(|(_, caret)| on_screen.contains(&caret.origin))
        .map(|(edge, caret)| Handle {
            edge,
            caret,
            hitbox: window.insert_hitbox(
                TouchHandle::hit_bounds(edge, caret),
                HitboxBehavior::BlockMouse,
            ),
        })
        .collect()
}

fn paint_handles(handles: &[Handle], window: &mut Window, cx: &mut App) {
    let color = gpui_kit::component::ActiveTheme::theme(cx)
        .selection
        .alpha(1.);
    for handle in handles {
        TouchHandle::paint(handle.edge, handle.caret, color, window);
    }
}

fn follow_drags(handles: &[Handle], state: &Entity<TextareaState>, window: &mut Window) {
    for handle in handles {
        let (hitbox, edge, grabbed) = (handle.hitbox.clone(), handle.edge, state.clone());
        window.on_mouse_event(move |event: &TouchDragEvent, phase, window, cx| {
            let begins = phase.bubble()
                && event.phase == TouchPhase::Started
                && !window.default_prevented()
                && hitbox.is_hovered(window);
            if !begins {
                return;
            }
            window.prevent_default();
            cx.stop_propagation();
            grabbed.update(cx, |state, cx| {
                state.begin_edge_drag(edge, event.position, cx)
            });
        });
        let (hitbox, edge, grabbed) = (handle.hitbox.clone(), handle.edge, state.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            let begins =
                phase.bubble() && event.button == MouseButton::Left && hitbox.is_hovered(window);
            if !begins {
                return;
            }
            cx.stop_propagation();
            grabbed.update(cx, |state, cx| {
                state.begin_edge_drag(edge, event.position, cx)
            });
        });
    }
    let dragging = {
        let state = state.clone();
        move |cx: &App| {
            state
                .read(cx)
                .touch_selection()
                .and_then(|held| held.dragging())
                .is_some()
        }
    };
    let (dragged, moved) = (state.clone(), dragging.clone());
    window.on_mouse_event(move |event: &TouchDragEvent, phase, _, cx| {
        let follows = phase.bubble() && event.phase != TouchPhase::Started && moved(cx);
        if !follows {
            return;
        }
        cx.stop_propagation();
        dragged.update(cx, |state, cx| match event.phase {
            TouchPhase::Moved => state.update_edge_drag(event.position, cx),
            _ => state.end_edge_drag(cx),
        });
    });
    let (dragged, moved) = (state.clone(), dragging.clone());
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        let follows =
            phase.bubble() && event.pressed_button == Some(MouseButton::Left) && moved(cx);
        if !follows {
            return;
        }
        dragged.update(cx, |state, cx| state.update_edge_drag(event.position, cx));
    });
    let dragged = state.clone();
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        let lets_go = phase.bubble() && event.button == MouseButton::Left && dragging(cx);
        if !lets_go {
            return;
        }
        dragged.update(cx, |state, cx| state.end_edge_drag(cx));
    });
}

fn offers(engine: &Engine, cx: &App) -> Vec<EditAction> {
    engine!(engine, |state| {
        let state = state.read(cx);
        let can = state.context_menu_capabilities();
        let len = state.value().len();
        let everything = state.selected_range() == (0..len);
        [
            (EditAction::Cut, can.is_editable() && can.is_copyable()),
            (EditAction::Copy, can.is_copyable()),
            (EditAction::Paste, can.is_editable()),
            (EditAction::SelectAll, len > 0 && !everything),
        ]
        .into_iter()
        .filter_map(|(action, offered)| offered.then_some(action))
        .collect()
    })
}

fn decide(engine: &Engine, wanted: &Rc<Cell<bool>>, window: &mut Window, cx: &mut App) {
    let field = engine!(engine, |state| state.entity_id());
    let focused = engine!(engine, |state| state.read(cx).focus_handle(cx)).is_focused(window);
    let held = engine!(engine, |state| state.read(cx).touch_selection());
    let at = held
        .filter(|held| held.dragging().is_none())
        .and_then(|held| held.bounds())
        .filter(|_| wanted.get() && focused);
    match at {
        Some(at) => {
            let menu = EditMenu::Show(at, offers(engine, cx));
            show(field, menu, engine, wanted, window);
        }
        None => hide(field, window),
    }
}

fn show(
    field: EntityId,
    menu: EditMenu,
    engine: &Engine,
    wanted: &Rc<Cell<bool>>,
    window: &Window,
) {
    let told = SHOWN.with_borrow(|shown| {
        shown
            .as_ref()
            .is_some_and(|shown| shown.field == field && shown.menu == menu)
    });
    if told {
        return;
    }
    SHOWN.set(Some(Shown {
        field,
        window: window.window_handle(),
        menu: menu.clone(),
        engine: Held::of(engine),
        wanted: Rc::downgrade(wanted),
    }));
    crate::contract::os().edit_menu(window, menu);
}

fn hide(field: EntityId, window: &Window) {
    let mine = SHOWN.with_borrow(|shown| shown.as_ref().is_some_and(|shown| shown.field == field));
    if mine {
        SHOWN.set(None);
        crate::contract::os().edit_menu(window, EditMenu::Hide);
    }
}

pub(super) fn left(field: EntityId, cx: &mut App) {
    let window = SHOWN.with_borrow(|shown| {
        shown
            .as_ref()
            .filter(|shown| shown.field == field)
            .map(|shown| shown.window)
    });
    let Some(window) = window else {
        return;
    };
    SHOWN.set(None);
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, _| {
            crate::contract::os().edit_menu(window, EditMenu::Hide)
        });
    });
}

pub(crate) fn picked(action: EditAction, cx: &mut App) {
    let Some(shown) = SHOWN.take() else {
        return;
    };
    let Some(engine) = shown.engine.upgrade() else {
        return;
    };
    let _ = shown.window.update(cx, |_, window, cx| {
        apply(action, &engine, &shown.wanted, window, cx)
    });
}

fn apply(
    action: EditAction,
    engine: &Engine,
    wanted: &Weak<Cell<bool>>,
    window: &mut Window,
    cx: &mut App,
) {
    let focus = engine!(engine, |state| state.read(cx).focus_handle(cx));
    match action {
        EditAction::Cut => focus.dispatch_action(&gpui_base::input::Cut, window, cx),
        EditAction::Copy => {
            focus.dispatch_action(&gpui_base::input::Copy, window, cx);
            if let Some(wanted) = wanted.upgrade() {
                wanted.set(false);
            }
        }
        EditAction::Paste => focus.dispatch_action(&gpui_base::input::Paste, window, cx),
        EditAction::SelectAll => engine!(engine, |state| state.update(cx, |state, cx| {
            state.select_all_from_edit_menu(window, cx)
        })),
    }
}
