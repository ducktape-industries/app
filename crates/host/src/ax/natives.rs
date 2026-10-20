//! The inspector's native nodes: what the host's own controls and fields
//! record of themselves as they are drawn (`a11y::inspected`) while the
//! door serves, and a press that goes in as a pointer's does.

use super::shown::{Shown, Views};
use crate::a11y::Native;
use gpui_kit::{
    App, Bounds, Global, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels,
    PlatformInput, Styled as _, Window, canvas,
};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

/// Every native element drawn while the door serves. Each record lives in
/// its element's own state, which gpui drops in the first frame the element
/// is not drawn and carries over a cached view's reuse; this keeps only a
/// weak hold on it.
#[derive(Default)]
pub struct Natives {
    drawn: Vec<Weak<RefCell<Drawn>>>,
    order: u64,
}

impl Global for Natives {}

/// One native element as its last draw left it.
pub struct Drawn {
    /// Its own element id.
    pub id: String,
    /// Its whole gpui path, ids joined by `.`: what tells apart two
    /// elements that share an id.
    pub at: String,
    pub native: Native,
    pub shown: Shown,
    order: u64,
}

impl Drawn {
    /// What `press` can do to it: a pointer's `click` on any, `type` on a
    /// field.
    pub fn presses(&self) -> Vec<&'static str> {
        match self.native.set_value {
            Some(_) => vec!["click", "type"],
            None => vec!["click"],
        }
    }
}

/// Starts recording: from the next draw on, every native element is kept.
pub fn serve(cx: &mut App) {
    cx.set_global(Natives::default());
    cx.set_global(Views::default());
    cx.refresh_windows();
}

/// The element that records `native` for the element it is a child of: no
/// node and no paint of its own, laid over its parent.
pub fn mark(native: Native) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| record(native, bounds, window, cx),
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

fn record(native: Native, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    if !cx.has_global::<Natives>() {
        return;
    }
    let shown = Shown::new(bounds, window);
    window.with_global_id("inspected".into(), |global, window| {
        let path = &global[..global.len().saturating_sub(1)];
        let id = path.last().map(ToString::to_string).unwrap_or_default();
        let at = path
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(".");
        window.with_element_state::<Rc<RefCell<Drawn>>, _>(global, |kept, _| {
            let natives = cx.global_mut::<Natives>();
            natives.order += 1;
            let drawn = Drawn {
                id,
                at,
                native,
                shown,
                order: natives.order,
            };
            let kept = match kept {
                Some(kept) => {
                    *kept.borrow_mut() = drawn;
                    kept
                }
                None => {
                    let kept = Rc::new(RefCell::new(drawn));
                    natives.drawn.retain(|drawn| drawn.strong_count() > 0);
                    natives.drawn.push(Rc::downgrade(&kept));
                    kept
                }
            };
            ((), kept)
        })
    })
}

/// Every native element its window last drew, in the order they were
/// recorded.
pub fn drawn(cx: &App) -> Vec<Rc<RefCell<Drawn>>> {
    let Some(natives) = cx.try_global::<Natives>() else {
        return Vec::new();
    };
    let mut drawn: Vec<_> = natives.drawn.iter().filter_map(Weak::upgrade).collect();
    drawn.sort_by_key(|drawn| drawn.borrow().order);
    drawn
}

/// Why a native press did not go in: a status and the words.
pub type Refused = (u16, String);

/// A pointer pressed and released at the middle of what the window shows
/// of `drawn`, through the window's own input dispatch, so its own click
/// listeners run as a person's click runs them. Refused when nothing of it
/// is shown, or when something drawn over it would take the press.
pub fn press(
    drawn: &Rc<RefCell<Drawn>>,
    press: &str,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), Refused> {
    if press != "click" {
        return Err((400, format!("a native element takes a click, not {press}")));
    }
    let at = {
        let drawn = drawn.borrow();
        drawn.shown.aim(&drawn.id, window)?
    };
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
        }),
        cx,
    );
    Ok(())
}

/// Sets a native field's text to `text`, as assistive technology's
/// SetValue sets it.
pub fn type_into(
    drawn: &Rc<RefCell<Drawn>>,
    text: &str,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), Refused> {
    let (set_value, id) = {
        let drawn = drawn.borrow();
        (drawn.native.set_value.clone(), drawn.id.clone())
    };
    let set_value = set_value.ok_or_else(|| (400, format!("{id} is not a field")))?;
    set_value(text.to_owned(), window, cx);
    Ok(())
}

#[cfg(all(test, feature = "ax-door"))]
mod tests {
    use super::*;
    use crate::a11y::Control as _;
    use crate::ax::shown::wheel;
    use gpui_kit::{
        Context, InteractiveElement as _, ParentElement as _, Render, Role,
        StatefulInteractiveElement as _, TestAppContext, VisualTestContext, div, px, size,
    };
    use gpui_kit::{ElementId, point};
    use std::cell::Cell;

    struct Screen {
        pressed: Rc<Cell<usize>>,
        covered: Rc<Cell<bool>>,
        gone: Rc<Cell<bool>>,
    }

    impl Render for Screen {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let pressed = self.pressed.clone();
            let send = div()
                .id("send")
                .control(Role::Button, "Send")
                .w(px(80.))
                .h(px(30.))
                .on_click(move |_, _, _| pressed.set(pressed.get() + 1));
            let cover = div().id("cover").absolute().inset_0().occlude();
            div()
                .id("screen")
                .size_full()
                .relative()
                .children((!self.gone.get()).then_some(send))
                .children(self.covered.get().then_some(cover))
        }
    }

    struct Mounted {
        native: VisualTestContext,
        pressed: Rc<Cell<usize>>,
        covered: Rc<Cell<bool>>,
        gone: Rc<Cell<bool>>,
    }

    fn mounted(cx: &mut TestAppContext) -> Mounted {
        cx.update(|cx| {
            gpui_kit::init(cx);
            serve(cx);
        });
        let pressed = Rc::new(Cell::new(0));
        let covered = Rc::new(Cell::new(false));
        let gone = Rc::new(Cell::new(false));
        let screen = (pressed.clone(), covered.clone(), gone.clone());
        let window = cx.open_window(size(px(300.), px(200.)), move |_, _| Screen {
            pressed: screen.0.clone(),
            covered: screen.1.clone(),
            gone: screen.2.clone(),
        });
        let mut native = VisualTestContext::from_window(window.into(), cx);
        native.update(crate::render::draw_frame);
        Mounted {
            native,
            pressed,
            covered,
            gone,
        }
    }

    fn redrawn(native: &mut VisualTestContext) {
        native.update(|window, cx| {
            window.refresh();
            crate::render::draw_frame(window, cx);
        });
    }

    fn send(native: &mut VisualTestContext) -> Option<Rc<RefCell<Drawn>>> {
        native.update(|_, cx| {
            drawn(cx)
                .into_iter()
                .find(|drawn| drawn.borrow().id == "send")
        })
    }

    #[gpui_kit::test]
    fn a_control_is_recorded_and_pressed_as_a_pointer_presses_it(cx: &mut TestAppContext) {
        let mut screen = mounted(cx);
        let send = send(&mut screen.native).expect("the button is recorded");
        {
            let send = send.borrow();
            assert_eq!(send.native.role, Some(Role::Button));
            assert_eq!(send.native.label.as_deref(), Some("Send"));
            assert!(send.at.ends_with("screen.send"), "{}", send.at);
            assert_eq!(send.presses(), ["click"]);
        }
        screen
            .native
            .update(|window, cx| press(&send, "click", window, cx))
            .unwrap();
        assert_eq!(screen.pressed.get(), 1);
        let refused = screen
            .native
            .update(|window, cx| press(&send, "submit", window, cx));
        assert_eq!(refused.unwrap_err().0, 400);
        assert_eq!(screen.pressed.get(), 1);
    }

    #[gpui_kit::test]
    fn a_covered_control_is_not_pressed(cx: &mut TestAppContext) {
        let mut screen = mounted(cx);
        screen.covered.set(true);
        redrawn(&mut screen.native);
        let send = send(&mut screen.native).unwrap();
        let refused = screen
            .native
            .update(|window, cx| press(&send, "click", window, cx));
        let (status, words) = refused.unwrap_err();
        assert_eq!(status, 409);
        assert!(words.contains("covered"), "{words}");
        assert_eq!(screen.pressed.get(), 0);
    }

    #[gpui_kit::test]
    fn a_control_not_drawn_is_gone(cx: &mut TestAppContext) {
        let mut screen = mounted(cx);
        assert!(send(&mut screen.native).is_some());
        screen.gone.set(true);
        redrawn(&mut screen.native);
        assert!(send(&mut screen.native).is_none());
    }

    struct List {
        pressed: Rc<RefCell<Vec<usize>>>,
        covered: bool,
    }

    impl Render for List {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let rows = (0..10).map(|row| {
                let pressed = self.pressed.clone();
                div()
                    .id(ElementId::Name(format!("row-{row}").into()))
                    .control(Role::Button, format!("Row {row}"))
                    .w(px(80.))
                    .h(px(30.))
                    .flex_shrink_0()
                    .on_click(move |_, _, _| pressed.borrow_mut().push(row))
            });
            let list = div()
                .id("list")
                .flex()
                .flex_col()
                .w(px(100.))
                .h(px(90.))
                .overflow_y_scroll()
                .children(rows);
            let cover = div().id("cover").absolute().inset_0().occlude();
            div()
                .id("screen")
                .size_full()
                .relative()
                .child(list)
                .children(self.covered.then_some(cover))
        }
    }

    fn listed(
        cx: &mut TestAppContext,
        covered: bool,
    ) -> (VisualTestContext, Rc<RefCell<Vec<usize>>>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            serve(cx);
        });
        let pressed = Rc::new(RefCell::new(Vec::new()));
        let list = pressed.clone();
        let window = cx.open_window(size(px(300.), px(200.)), move |_, _| List {
            pressed: list.clone(),
            covered,
        });
        let mut native = VisualTestContext::from_window(window.into(), cx);
        native.update(crate::render::draw_frame);
        (native, pressed)
    }

    fn row(native: &mut VisualTestContext, row: usize) -> Rc<RefCell<Drawn>> {
        let id = format!("row-{row}");
        native
            .update(|_, cx| drawn(cx).into_iter().find(|drawn| drawn.borrow().id == id))
            .unwrap_or_else(|| panic!("row {row} is not drawn"))
    }

    fn shown(native: &mut VisualTestContext, at: usize) -> Shown {
        row(native, at).borrow().shown.clone()
    }

    #[gpui_kit::test]
    fn a_wheel_at_a_row_scrolls_the_list_under_it(cx: &mut TestAppContext) {
        let (mut native, _) = listed(cx, false);
        let before = shown(&mut native, 1);
        let at = before.visible().center();
        native.update(|window, cx| wheel(at, point(px(0.), px(-30.)), window, cx));
        let after = shown(&mut native, 1);
        assert_eq!(after.bounds.origin.y, before.bounds.origin.y - px(30.));
    }

    #[gpui_kit::test]
    fn a_clipped_row_is_wheeled_into_view_and_pressed_once(cx: &mut TestAppContext) {
        let (mut native, pressed) = listed(cx, false);
        let clipped = shown(&mut native, 5);
        assert!(clipped.area() <= 0.);
        let by = clipped.toward();
        assert_eq!(by, point(px(0.), px(-90.)));
        native.update(|window, cx| wheel(clipped.mask.center(), by, window, cx));
        assert_eq!(shown(&mut native, 5).toward(), point(px(0.), px(0.)));
        let five = row(&mut native, 5);
        native
            .update(|window, cx| press(&five, "click", window, cx))
            .unwrap();
        assert_eq!(*pressed.borrow(), [5]);
    }

    #[gpui_kit::test]
    fn a_covered_list_is_not_wheeled(cx: &mut TestAppContext) {
        let (mut native, _) = listed(cx, true);
        let first = shown(&mut native, 0);
        let (status, words) = native
            .update(|window, _| first.aim("row-0", window))
            .unwrap_err();
        assert_eq!(status, 409);
        assert!(words.contains("covered"), "{words}");
    }
}
