//! The page: gpui on the web platform, the host started on the browser's services and view runner, connected to the
//! node, and the opened program's seat drawn under the preview's banner.

use crate::link::Opened;
use ducktape_host::contract::Boot;
use ducktape_host::runtime::Placed;
use ducktape_host::session::{Place, SeatsEvent, Shared};
use futures::StreamExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    App, AppContext as _, Application, ApplicationHandle, AsyncApp, Context, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Window, WindowOptions, div, px,
};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

const INSTANCE: u64 = 1;
const BANNER_HEIGHT: f32 = 28.;
const PREVIEW: &str = "Read-only preview: sign-in coming";

thread_local! {
    static APP: RefCell<Option<ApplicationHandle>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn run() {
    console_error_panic_hook::set_once();
    wasm_bindgen_futures::spawn_local(boot());
}

fn renderer() -> gpui_kit::web::WebBackendPreference {
    let search = crate::node::page().location().search().unwrap_or_default();
    let asked = web_sys::UrlSearchParams::new_with_str(&search)
        .ok()
        .and_then(|params| params.get("renderer"));
    match asked.as_deref() {
        Some("webgl") => gpui_kit::web::WebBackendPreference::WebGl,
        Some("webgpu") => gpui_kit::web::WebBackendPreference::WebGpu,
        _ => gpui_kit::web::WebBackendPreference::Auto,
    }
}

async fn boot() {
    let node = crate::node::choose().await;
    let path = crate::node::page()
        .location()
        .pathname()
        .unwrap_or_default();
    let opened = crate::link::from_path(&path);
    gpui_kit::platform::web_init();
    let platform = Rc::new(
        gpui_kit::web::WebPlatform::new_with_backend_and_font_fallback(
            false,
            renderer(),
            gpui_kit::web::CanvasFontFallback::EmojiAndCjk,
        ),
    );
    let http = Arc::new(platform.fetch_http_client());
    let handle = Application::with_platform(platform)
        .with_http_client(http)
        .run_embedded(move |cx: &mut App| launch(node, opened, cx));
    APP.with(|app| *app.borrow_mut() = Some(handle));
}

fn launch(node: String, opened: Opened, cx: &mut App) {
    gpui_kit::init(cx);
    let faces = ducktape_host::fonts::BUNDLED_FACES
        .iter()
        .copied()
        .map(Cow::Borrowed)
        .collect();
    if let Err(error) = cx.text_system().add_fonts(faces) {
        web_sys::console::error_1(
            &format!("the bundled faces could not be registered: {error}").into(),
        );
    }
    ducktape_host::theme::configure_native_theme(cx);
    let boot = Boot {
        os: Arc::new(crate::os::WebOs::new()),
        runner: Arc::new(ducktape_host_web::WebRunner::new()),
        open: |_, _| {},
    };
    let (host, _inbound) = ducktape_host::start::start(boot, cx);
    host.shared
        .session
        .update(cx, |session, cx| session.connect(node, cx));
    let program: &'static str = Box::leak(opened.program.into_boxed_str());
    if let Some(route) = opened.route {
        ducktape_host::runtime::route_to(&host.shared.net.network, program, INSTANCE, route);
    }
    let (signals, mut asks) = (host.signals, host.consent);
    cx.spawn(async move |_: &mut AsyncApp| while signals.recv().await.is_ok() {})
        .detach();
    cx.spawn(async move |_: &mut AsyncApp| {
        while asks.next().await.is_some() {
            ducktape_host::runtime::consent::refuse_all();
        }
    })
    .detach();
    let shared = host.shared;
    let opened = gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
        cx.new(|cx| Root::new(shared, program, window, cx))
    });
    if let Err(error) = opened {
        web_sys::console::error_1(&format!("the page's window did not open: {error}").into());
    }
    cx.activate(true);
}

struct Root {
    shared: Shared,
    program: &'static str,
    placed: Option<((f32, f32), bool)>,
    seat_observed: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl Root {
    fn new(
        shared: Shared,
        program: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&shared.session, |_, _, cx| cx.notify()),
            cx.observe(&shared.seats, |this, _, cx| this.observe_seat(cx)),
            cx.subscribe(&shared.seats, |_, _, event: &SeatsEvent, _| match event {
                SeatsEvent::OpenLink(_, _, link) | SeatsEvent::OpenWindow(_, _, link) => {
                    crate::link::open(link)
                }
                _ => {}
            }),
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
        ];
        Self {
            shared,
            program,
            placed: None,
            seat_observed: None,
            _subscriptions: subscriptions,
        }
    }

    fn observe_seat(&mut self, cx: &mut Context<Self>) {
        let seat = self.shared.seats.read(cx).seat(INSTANCE);
        self.seat_observed = seat.map(|seat| cx.observe(&seat, |_, _, cx| cx.notify()));
        cx.notify();
    }

    fn place(&mut self, window: &Window, body: (f32, f32), cx: &mut Context<Self>) {
        let in_front = window.is_window_active();
        if self.placed == Some((body, in_front)) {
            return;
        }
        self.placed = Some((body, in_front));
        let place = (window.window_handle(), Some(body), in_front);
        let wanted: BTreeMap<u64, Place> =
            BTreeMap::from([(INSTANCE, (self.program, Some(place), Placed::default()))]);
        let seats = self.shared.seats.clone();
        cx.defer(move |cx| {
            seats.update(cx, |seats, cx| seats.place(wanted, cx));
            if let Some(seat) = seats.read(cx).seat(INSTANCE) {
                seat.update(cx, |seat, cx| seat.set_keys_free(true, cx));
            }
        });
    }

    fn banner(&self, cx: &App) -> String {
        let session = self.shared.session.read(cx).get();
        match session.error.is_empty() {
            true => PREVIEW.to_owned(),
            false => format!("{}: {}", session.reaching, session.error),
        }
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let body = (
            f32::from(viewport.width),
            f32::from(viewport.height) - BANNER_HEIGHT,
        );
        self.place(window, body, cx);
        let tree = self
            .shared
            .seats
            .read(cx)
            .seat(INSTANCE)
            .and_then(|seat| seat.read(cx).tree());
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family(ducktape_host::fonts::FAMILY_UI)
            .child(
                div()
                    .h(px(BANNER_HEIGHT))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .bg(theme.muted)
                    .text_color(theme.muted_foreground)
                    .child(self.banner(cx)),
            )
            .child(div().w_full().flex_1().children(tree))
    }
}
