//! The window's pointer router: one capture listener per pointer event, and
//! the text a left drag selects in the scope it began in: the nearest
//! scroller or overlay, else the view's pane.
use super::*;
use gpui_kit::{
    DispatchPhase, EntityId, Global, Hitbox, HitboxBehavior, InputEvent as _, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollDelta, ScrollWheelEvent, TextLayout,
    WindowId,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::{Rc, Weak};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct NodeKey {
    tree: EntityId,
    path: AuthoredPath,
    place: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum ScopeKey {
    Pane(EntityId),
    Node(NodeKey),
}

impl ScopeKey {
    fn tree(&self) -> EntityId {
        match self {
            Self::Pane(tree) => *tree,
            Self::Node(key) => key.tree,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    Scope,
    Claim,
}

fn mark_of(node: &wire::Node, styles: &wire::Styles) -> Option<Mark> {
    let consumes = node
        .interactivity()
        .is_some_and(|interactivity| interactivity.consumes_click);
    match node {
        wire::Node::Field { .. } | wire::Node::ResizeHandle { .. } => Some(Mark::Claim),
        _ if consumes => Some(Mark::Claim),
        wire::Node::UniformList { .. } | wire::Node::List { .. } | wire::Node::Overlay { .. } => {
            Some(Mark::Scope)
        }
        wire::Node::Container(view_wire::ContainerNode { style, .. }) => {
            let style = drawn_style(styles, *style);
            let scroll = Some(gpui_kit::Overflow::Scroll);
            (style.overflow.x == scroll || style.overflow.y == scroll).then_some(Mark::Scope)
        }
        _ => None,
    }
}

impl ViewTree {
    pub(super) fn marked(
        &self,
        node: &wire::Node,
        entered: bool,
        place: u64,
        child: AnyElement,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(mark) = mark_of(node, &self.styles) else {
            return child;
        };
        let key = ScopeKey::Node(NodeKey {
            tree: cx.entity_id(),
            path: self.authored_path.clone(),
            place: if entered { 0 } else { place },
        });
        Marked { mark, key, child }.into_any_element()
    }
}

pub(super) fn pane(tree: EntityId, child: AnyElement) -> AnyElement {
    Marked {
        mark: Mark::Scope,
        key: ScopeKey::Pane(tree),
        child,
    }
    .into_any_element()
}

struct Region {
    mark: Mark,
    key: ScopeKey,
    depth: usize,
    hitbox: Hitbox,
}

pub(super) struct Participant {
    id: u64,
    scope: Option<ScopeKey>,
    order: u64,
    hitbox: Hitbox,
    text: SharedString,
    layout: TextLayout,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Spot {
    id: u64,
    order: u64,
    index: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct Selection {
    scope: ScopeKey,
    anchor: Spot,
    cursor: Spot,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Gesture {
    #[default]
    Idle,
    Selecting {
        at: Point<Pixels>,
        moved: bool,
    },
}

enum Input<'a> {
    Press(&'a MouseDownEvent),
    Move(&'a MouseMoveEvent),
    Release,
    Wheel,
    Frame,
}

#[derive(Default)]
struct Effects {
    notify: Vec<EntityId>,
    scroll: Option<(Point<Pixels>, Point<Pixels>)>,
    arm: bool,
}

type Rows = Vec<Rc<RefCell<Participant>>>;

#[derive(Default)]
pub struct Router {
    regions: Vec<Weak<RefCell<Region>>>,
    participants: Vec<Weak<RefCell<Participant>>>,
    stack: Vec<ScopeKey>,
    next_id: u64,
    gesture: Gesture,
    selection: Option<Selection>,
    took: bool,
    armed: bool,
}

impl Router {
    fn step(&mut self, input: Input, window: &Window) -> Effects {
        match input {
            Input::Press(event) => self.press(event, window),
            Input::Move(event) => self.moved(event),
            Input::Release => self.release(),
            Input::Wheel => self.wheeled(),
            Input::Frame => self.frame(),
        }
    }

    fn regions(&mut self) -> Vec<Rc<RefCell<Region>>> {
        self.regions.retain(|region| region.strong_count() > 0);
        self.regions.iter().filter_map(Weak::upgrade).collect()
    }

    fn live(&mut self) -> Rows {
        self.participants.retain(|row| row.strong_count() > 0);
        self.participants.iter().filter_map(Weak::upgrade).collect()
    }

    fn rows(&mut self, scope: &ScopeKey) -> Rows {
        let mut rows: Rows = self
            .live()
            .into_iter()
            .filter(|row| row.borrow().scope.as_ref() == Some(scope))
            .collect();
        rows.sort_by_key(|row| row.borrow().order);
        rows
    }

    fn visible(&mut self, scope: &ScopeKey) -> Option<Bounds<Pixels>> {
        self.regions().into_iter().find_map(|region| {
            let region = region.borrow();
            (region.mark == Mark::Scope && region.key == *scope).then(|| visible(&region.hitbox))
        })
    }

    fn scope_at(&mut self, window: &Window) -> (Option<ScopeKey>, Option<Spot>) {
        let hovered = self.live().into_iter().find(|row| {
            let row = row.borrow();
            row.scope.is_some() && row.hitbox.is_hovered(window)
        });
        if let Some(row) = hovered {
            let row = row.borrow();
            let at = window.mouse_position();
            let spot = Spot {
                id: row.id,
                order: row.order,
                index: caret(&row.text, &row.layout, at),
            };
            return (row.scope.clone(), Some(spot));
        }
        let innermost = self
            .regions()
            .into_iter()
            .filter(|region| {
                let region = region.borrow();
                region.mark == Mark::Scope && region.hitbox.is_hovered(window)
            })
            .max_by_key(|region| region.borrow().depth);
        (innermost.map(|region| region.borrow().key.clone()), None)
    }

    fn press(&mut self, event: &MouseDownEvent, window: &Window) -> Effects {
        let before = self.selection.clone();
        self.took = false;
        let claimed = self.regions().iter().any(|region| {
            let region = region.borrow();
            region.mark == Mark::Claim && region.hitbox.is_hovered(window)
        });
        if claimed {
            self.gesture = Gesture::Idle;
            return Effects::default();
        }
        let (scope, on_text) = self.scope_at(window);
        let bounds = scope.as_ref().and_then(|scope| self.visible(scope));
        let (Some(scope), Some(bounds)) = (scope, bounds) else {
            self.gesture = Gesture::Idle;
            self.selection = None;
            return changed(&before, &self.selection);
        };
        let rows = self.rows(&scope);
        let cursor = spot_at(clamp(event.position, bounds), &rows);
        self.selection = match (event.click_count, cursor, on_text) {
            (1, Some(cursor), _) => {
                let anchor = before
                    .as_ref()
                    .filter(|held| event.modifiers.shift && held.scope == scope)
                    .map_or(cursor, |held| held.anchor);
                Some(Selection {
                    scope,
                    anchor,
                    cursor,
                })
            }
            (1, None, _) | (_, _, None) => None,
            (clicks, _, Some(on)) => around(&rows, on, clicks).map(|(anchor, cursor)| Selection {
                scope,
                anchor,
                cursor,
            }),
        };
        self.gesture = match event.click_count {
            1 => Gesture::Selecting {
                at: event.position,
                moved: false,
            },
            _ => Gesture::Idle,
        };
        changed(&before, &self.selection)
    }

    fn moved(&mut self, event: &MouseMoveEvent) -> Effects {
        if self.gesture == Gesture::Idle {
            return Effects::default();
        }
        if event.pressed_button != Some(MouseButton::Left) {
            return self.release();
        }
        self.gesture = Gesture::Selecting {
            at: event.position,
            moved: true,
        };
        self.follow(event.position)
    }

    fn follow(&mut self, at: Point<Pixels>) -> Effects {
        let before = self.selection.clone();
        let Some(selection) = before.clone() else {
            return Effects::default();
        };
        let Some(bounds) = self.visible(&selection.scope) else {
            return Effects::default();
        };
        let rows = self.rows(&selection.scope);
        let inside = clamp(at, bounds);
        if let Some(cursor) = spot_at(inside, &rows) {
            self.selection = Some(Selection {
                cursor,
                ..selection
            });
        }
        let mut effects = changed(&before, &self.selection);
        let past = at - inside;
        if past != Point::default() {
            effects.scroll = Some((bounds.center(), past));
            effects.arm = true;
        }
        effects
    }

    fn release(&mut self) -> Effects {
        let Gesture::Selecting { moved, .. } = std::mem::take(&mut self.gesture) else {
            return Effects::default();
        };
        let before = self.selection.clone();
        let taken = !self.text().is_empty();
        self.took = moved && taken;
        if !taken {
            self.selection = None;
        }
        changed(&before, &self.selection)
    }

    fn frame(&mut self) -> Effects {
        self.armed = false;
        match self.gesture {
            Gesture::Selecting { at, .. } => self.follow(at),
            Gesture::Idle => Effects::default(),
        }
    }

    fn wheeled(&self) -> Effects {
        Effects {
            arm: self.gesture != Gesture::Idle,
            ..Effects::default()
        }
    }

    fn text(&mut self) -> String {
        let Some(selection) = self.selection.clone() else {
            return String::new();
        };
        let rows = self.rows(&selection.scope);
        let Some((start, end)) = ends(&selection, &rows) else {
            return String::new();
        };
        rows.iter()
            .filter_map(|row| {
                let row = row.borrow();
                covered(start, end, row.order, row.text.len())
                    .map(|range| row.text[range].to_owned())
            })
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn range_of(&mut self, participant: u64) -> Option<Range<usize>> {
        let selection = self.selection.clone()?;
        let rows = self.rows(&selection.scope);
        let (start, end) = ends(&selection, &rows)?;
        let row = rows.iter().find(|row| row.borrow().id == participant)?;
        let row = row.borrow();
        covered(start, end, row.order, row.text.len())
    }
}

fn changed(before: &Option<Selection>, after: &Option<Selection>) -> Effects {
    if before == after {
        return Effects::default();
    }
    let mut notify: Vec<EntityId> = before
        .iter()
        .chain(after)
        .map(|held| held.scope.tree())
        .collect();
    notify.dedup();
    Effects {
        notify,
        ..Effects::default()
    }
}

fn visible(hitbox: &Hitbox) -> Bounds<Pixels> {
    hitbox.bounds.intersect(&hitbox.content_mask.bounds)
}

fn clamp(at: Point<Pixels>, bounds: Bounds<Pixels>) -> Point<Pixels> {
    at.clamp(&bounds.origin, &bounds.bottom_right())
}

fn spot_at(at: Point<Pixels>, rows: &Rows) -> Option<Spot> {
    let spot = |row: &Participant, index| Spot {
        id: row.id,
        order: row.order,
        index,
    };
    if let Some(row) = rows
        .iter()
        .find(|row| row.borrow().hitbox.bounds.contains(&at))
    {
        let row = row.borrow();
        return Some(spot(&row, caret(&row.text, &row.layout, at)));
    }
    let above = rows
        .iter()
        .rev()
        .find(|row| row.borrow().hitbox.bounds.top() <= at.y);
    match above {
        None => rows.first().map(|row| spot(&row.borrow(), 0)),
        Some(row) => {
            let row = row.borrow();
            let bounds = row.hitbox.bounds;
            let before = at.y < bounds.bottom() && at.x < bounds.left();
            Some(spot(&row, if before { 0 } else { row.text.len() }))
        }
    }
}

fn caret(text: &str, layout: &TextLayout, at: Point<Pixels>) -> usize {
    if text.len() != layout.len() {
        return 0;
    }
    let (height, right) = (layout.line_height(), layout.bounds().right());
    let mut row_end = None;
    for (offset, glyph) in text.char_indices() {
        let Some(start) = layout.position_for_index(offset) else {
            continue;
        };
        if at.y < start.y || at.y >= start.y + height {
            continue;
        }
        let next = offset + glyph.len_utf8();
        let end = layout
            .position_for_index(next)
            .filter(|end| end.y == start.y)
            .map_or(right, |end| end.x);
        if at.x < start.x + (end - start.x) / 2. {
            return offset;
        }
        row_end = Some(next);
    }
    row_end.unwrap_or(if at.y < layout.bounds().top() {
        0
    } else {
        text.len()
    })
}

fn around(rows: &Rows, on: Spot, clicks: usize) -> Option<(Spot, Spot)> {
    let row = rows.iter().find(|row| row.borrow().id == on.id)?;
    let row = row.borrow();
    let range = match clicks {
        2 => word_at(&row.text, on.index),
        _ => line_at(&row.text, on.index),
    };
    (!range.is_empty()).then_some((
        Spot {
            index: range.start,
            ..on
        },
        Spot {
            index: range.end,
            ..on
        },
    ))
}

fn word_at(text: &str, index: usize) -> Range<usize> {
    let word = |glyph: char| glyph.is_alphanumeric() || glyph == '_';
    let start = text[..index]
        .char_indices()
        .rev()
        .take_while(|(_, glyph)| word(*glyph))
        .last()
        .map_or(index, |(at, _)| at);
    let end = text[index..]
        .char_indices()
        .take_while(|(_, glyph)| word(*glyph))
        .last()
        .map_or(index, |(at, glyph)| index + at + glyph.len_utf8());
    match (start == end, text[index..].chars().next()) {
        (true, Some(glyph)) => index..index + glyph.len_utf8(),
        _ => start..end,
    }
}

fn line_at(text: &str, index: usize) -> Range<usize> {
    let start = text[..index].rfind('\n').map_or(0, |at| at + 1);
    let end = text[index..].find('\n').map_or(text.len(), |at| index + at);
    start..end
}

fn ends(selection: &Selection, rows: &Rows) -> Option<((u64, usize), (u64, usize))> {
    let anchor = resolve(selection.anchor, selection.cursor.order, rows)?;
    let cursor = resolve(selection.cursor, selection.anchor.order, rows)?;
    Some(if anchor <= cursor {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    })
}

fn resolve(spot: Spot, other: u64, rows: &Rows) -> Option<(u64, usize)> {
    if let Some(row) = rows.iter().find(|row| row.borrow().id == spot.id) {
        let row = row.borrow();
        return Some((row.order, spot.index.min(row.text.len())));
    }
    match spot.order <= other {
        true => rows.first().map(|row| (row.borrow().order, 0)),
        false => rows.last().map(|row| {
            let row = row.borrow();
            (row.order, row.text.len())
        }),
    }
}

fn covered(start: (u64, usize), end: (u64, usize), order: u64, len: usize) -> Option<Range<usize>> {
    if order < start.0 || order > end.0 {
        return None;
    }
    let from = if order == start.0 { start.1 } else { 0 };
    let to = if order == end.0 { end.1 } else { len };
    (from < to).then_some(from..to)
}

#[derive(Default)]
struct Routers(HashMap<WindowId, Weak<RefCell<Router>>>);

impl Global for Routers {}

fn router(window: &Window, cx: &App) -> Option<Rc<RefCell<Router>>> {
    cx.try_global::<Routers>()?
        .0
        .get(&window.window_handle().window_id())?
        .upgrade()
}

fn kept<T: 'static>(
    id: ElementId,
    window: &mut Window,
    make: impl FnOnce() -> T,
) -> (Rc<RefCell<T>>, bool) {
    window.with_global_id(id, |global_id, window| {
        window.with_element_state::<Rc<RefCell<T>>, _>(global_id, |held, _| match held {
            Some(held) => ((held.clone(), false), held),
            None => {
                let made = Rc::new(RefCell::new(make()));
                ((made.clone(), true), made)
            }
        })
    })
}

fn element_id(mark: Mark, key: &ScopeKey) -> ElementId {
    let mut hasher = std::hash::DefaultHasher::new();
    key.hash(&mut hasher);
    let name = match mark {
        Mark::Scope => "selection-scope",
        Mark::Claim => "selection-claim",
    };
    ElementId::NamedInteger(name.into(), hasher.finish())
}

pub(super) fn register(
    paragraph: &ElementId,
    order: u64,
    hitbox: &Hitbox,
    text: &SharedString,
    layout: &TextLayout,
    window: &mut Window,
    cx: &mut App,
) -> Option<u64> {
    let router = router(window, cx)?;
    let scope = router.borrow().stack.last().cloned();
    let made = || Participant {
        id: 0,
        scope: scope.clone(),
        order,
        hitbox: hitbox.clone(),
        text: text.clone(),
        layout: layout.clone(),
    };
    let (row, new) = kept(paragraph.clone(), window, made);
    let mut router = router.borrow_mut();
    let mut held = row.borrow_mut();
    if new {
        held.id = router.next_id;
        router.next_id += 1;
        router.participants.push(Rc::downgrade(&row));
    }
    held.scope = scope;
    held.order = order;
    held.hitbox = hitbox.clone();
    held.text = text.clone();
    held.layout = layout.clone();
    Some(held.id)
}

pub(super) fn stack(window: &Window, cx: &App) -> Option<Vec<ScopeKey>> {
    router(window, cx).map(|router| router.borrow().stack.clone())
}

pub(super) fn within<R>(
    stack: Option<Vec<ScopeKey>>,
    window: &mut Window,
    cx: &mut App,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    let (Some(stack), Some(router)) = (stack, router(window, cx)) else {
        return f(window, cx);
    };
    let outer = std::mem::replace(&mut router.borrow_mut().stack, stack);
    let out = f(window, cx);
    router.borrow_mut().stack = outer;
    out
}

pub(super) fn range_of(participant: u64, window: &Window, cx: &App) -> Option<Range<usize>> {
    router(window, cx)?.borrow_mut().range_of(participant)
}

pub(super) fn took(window: &Window, cx: &App) -> Option<bool> {
    router(window, cx).map(|router| router.borrow().took)
}

pub fn selected_text(window: &mut Window, cx: &mut App) -> String {
    router(window, cx)
        .map(|router| router.borrow_mut().text())
        .unwrap_or_default()
}

pub fn has_selection(window: &mut Window, cx: &mut App) -> bool {
    !selected_text(window, cx).is_empty()
}

pub fn copy(window: &mut Window, cx: &mut App) {
    let text = selected_text(window, cx);
    if text.is_empty() {
        cx.propagate();
        return;
    }
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
}

fn apply(router: &Rc<RefCell<Router>>, effects: Effects, window: &mut Window, cx: &mut App) {
    for tree in effects.notify {
        cx.notify(tree);
    }
    if let Some((position, past)) = effects.scroll {
        window.defer(cx, move |window, cx| {
            let wheel = ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(-past.x, -past.y)),
                ..Default::default()
            };
            window.dispatch_event(wheel.to_platform_input(), cx);
        });
    }
    if effects.arm && !std::mem::replace(&mut router.borrow_mut().armed, true) {
        let router = router.clone();
        window.on_next_frame(move |window, cx| route(&router, Input::Frame, window, cx));
    }
}

fn route(router: &Rc<RefCell<Router>>, input: Input, window: &mut Window, cx: &mut App) {
    let effects = router.borrow_mut().step(input, window);
    apply(router, effects, window, cx);
}

fn listen(router: &Rc<RefCell<Router>>, window: &mut Window) {
    let pressed = router.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
            route(&pressed, Input::Press(event), window, cx);
        }
    });
    let moving = router.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            route(&moving, Input::Move(event), window, cx);
        }
    });
    let released = router.clone();
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
            route(&released, Input::Release, window, cx);
        }
    });
    let wheeled = router.clone();
    window.on_mouse_event(move |_: &ScrollWheelEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            route(&wheeled, Input::Wheel, window, cx);
        }
    });
}

struct Marked {
    mark: Mark,
    key: ScopeKey,
    child: AnyElement,
}

impl IntoElement for Marked {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Marked {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(router) = router(window, cx) else {
            self.child.prepaint(window, cx);
            return;
        };
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let depth = router.borrow().stack.len();
        let (mark, key) = (self.mark, self.key.clone());
        let made = || Region {
            mark,
            key: key.clone(),
            depth,
            hitbox: hitbox.clone(),
        };
        let (region, new) = kept(element_id(mark, &key), window, made);
        *region.borrow_mut() = Region {
            mark,
            key: key.clone(),
            depth,
            hitbox,
        };
        if new {
            router.borrow_mut().regions.push(Rc::downgrade(&region));
        }
        let scope = mark == Mark::Scope;
        if scope {
            router.borrow_mut().stack.push(key);
        }
        self.child.prepaint(window, cx);
        if scope {
            router.borrow_mut().stack.pop();
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

pub struct Layer;

impl IntoElement for Layer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Layer {
    type RequestLayoutState = ();
    type PrepaintState = Rc<RefCell<Router>>;

    fn id(&self) -> Option<ElementId> {
        Some("pointer-router".into())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (
            window.request_layout(gpui_kit::Style::default(), [], cx),
            (),
        )
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Rc<RefCell<Router>> {
        let id = id.expect("the router layer has an id");
        let router = window.with_element_state::<Rc<RefCell<Router>>, _>(id, |held, _| {
            let router = held.unwrap_or_default();
            (router.clone(), router)
        });
        let window_id = window.window_handle().window_id();
        let known = cx
            .try_global::<Routers>()
            .and_then(|routers| routers.0.get(&window_id))
            .and_then(Weak::upgrade)
            .is_some_and(|known| Rc::ptr_eq(&known, &router));
        if !known {
            let routers = &mut cx.default_global::<Routers>().0;
            routers.retain(|_, held| held.strong_count() > 0);
            routers.insert(window_id, Rc::downgrade(&router));
        }
        router
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        router: &mut Rc<RefCell<Router>>,
        window: &mut Window,
        _: &mut App,
    ) {
        listen(router, window);
    }
}
