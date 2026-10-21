//! A headless inner platform for tests, driven by gpui's `TestDispatcher` through
//! `Application::run_embedded`, and `Harness`: one window of it under `A11yPlatform`.
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use anyhow::Result;
use futures::channel::oneshot;
use gpui::{
    A11yCallbacks, Action, AnyWindowHandle, App, AppContext, Application, ApplicationHandle,
    AtlasKey, AtlasTextureId, AtlasTextureKind, AtlasTile, BackgroundExecutor, Bounds, Capslock,
    ClipboardItem, Context, CursorStyle, DevicePixels, DispatchEventResult, DisplayId,
    DummyKeyboardMapper, ForegroundExecutor, GpuSpecs, Keymap, Menu, MenuItem, Modifiers,
    NoopTextSystem, PathPromptOptions, Pixels, Platform, PlatformAtlas, PlatformDisplay,
    PlatformInput, PlatformInputHandler, PlatformKeyboardLayout, PlatformKeyboardMapper,
    PlatformTextSystem, PlatformWindow, Point, PromptButton, PromptLevel, Render,
    RequestFrameOptions, Scene, Size, Task, TestDispatcher, ThermalState, TileId, Window,
    WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowHandle,
    WindowOptions, WindowParams, WindowVisibility, accesskit, px,
};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle as RawWindowHandle,
};

use crate::{A11yPlatform, WindowA11y};

pub struct HeadlessPlatform {
    background: BackgroundExecutor,
    foreground: ForegroundExecutor,
    windows: RefCell<Vec<Rc<HeadlessWindow>>>,
    clipboard: RefCell<Option<ClipboardItem>>,
}

impl HeadlessPlatform {
    pub fn new(dispatcher: TestDispatcher) -> Rc<Self> {
        let dispatcher = Arc::new(dispatcher);
        Rc::new(Self {
            background: BackgroundExecutor::new(dispatcher.clone()),
            foreground: ForegroundExecutor::new(dispatcher),
            windows: RefCell::default(),
            clipboard: RefCell::default(),
        })
    }

    pub fn windows(&self) -> Vec<Rc<HeadlessWindow>> {
        self.windows.borrow().clone()
    }
}

type FrameRequest = Box<dyn FnMut(RequestFrameOptions)>;

#[derive(Default)]
pub struct HeadlessWindow {
    bounds: Bounds<Pixels>,
    input_handler: RefCell<Option<PlatformInputHandler>>,
    request_frame: RefCell<Option<FrameRequest>>,
    a11y: RefCell<Option<A11yCallbacks>>,
    tree: RefCell<Option<accesskit::TreeUpdate>>,
}

impl HeadlessWindow {
    pub fn draw(&self, force_render: bool) {
        let callback = self.request_frame.borrow_mut().take();
        if let Some(mut callback) = callback {
            callback(RequestFrameOptions {
                require_presentation: false,
                force_render,
                ..Default::default()
            });
            *self.request_frame.borrow_mut() = Some(callback);
        }
    }

    pub fn adapter_activates(&self) -> bool {
        let a11y = self.a11y.borrow();
        let Some(callbacks) = a11y.as_ref() else {
            return false;
        };
        (callbacks.activation)();
        true
    }

    pub fn tree(&self) -> Option<accesskit::TreeUpdate> {
        self.tree.borrow().clone()
    }
}

pub struct Harness<V: 'static> {
    pub dispatcher: TestDispatcher,
    pub app: ApplicationHandle,
    pub inner: Rc<HeadlessWindow>,
    pub a11y: Rc<WindowA11y>,
    pub root: WindowHandle<V>,
}

impl<V: Render + 'static> Harness<V> {
    pub fn open(build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static) -> Self {
        let dispatcher = TestDispatcher::new(0);
        let headless = HeadlessPlatform::new(dispatcher.clone());
        let platform = A11yPlatform::new(headless.clone());
        let app = Application::with_platform(platform.clone()).run_embedded(|_: &mut App| {});
        let root = app.update(|cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                cx.new(|cx| build(window, cx))
            })
            .expect("the headless platform opens every window")
        });
        let inner = headless.windows()[0].clone();
        let a11y = platform
            .window(root.into())
            .expect("the wrapper keeps every window it opened");
        let harness = Self {
            dispatcher,
            app,
            inner,
            a11y,
            root,
        };
        harness.draw();
        harness
    }

    pub fn draw(&self) {
        self.settle_and_draw(true)
    }

    pub fn draw_unforced(&self) {
        self.settle_and_draw(false)
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> R) -> R {
        self.app.update(|cx| {
            self.root
                .update(cx, f)
                .expect("the harness window stays open")
        })
    }

    pub fn tree(&self) -> accesskit::TreeUpdate {
        self.a11y.tree().expect("a tree gpui sent while active")
    }

    pub fn act(&self, action: accesskit::Action, target: accesskit::NodeId) {
        let tree_id = self.tree().tree_id;
        self.a11y.act(accesskit::ActionRequest {
            action,
            target_node: target,
            target_tree: tree_id,
            data: None,
        });
        self.draw();
    }

    fn settle_and_draw(&self, force_render: bool) {
        self.dispatcher.run_until_parked();
        self.inner.draw(force_render);
        self.dispatcher.run_until_parked();
    }
}

#[derive(Debug)]
struct HeadlessDisplay;

impl PlatformDisplay for HeadlessDisplay {
    fn id(&self) -> DisplayId {
        DisplayId::new(1)
    }
    fn uuid(&self) -> Result<uuid::Uuid> {
        Ok(uuid::Uuid::nil())
    }
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(Point::default(), Size::new(px(1920.), px(1080.)))
    }
    fn refresh_interval(&self) -> Option<std::time::Duration> {
        None
    }
}

struct HeadlessLayout;

impl PlatformKeyboardLayout for HeadlessLayout {
    fn id(&self) -> &str {
        "headless"
    }
    fn name(&self) -> &str {
        "headless"
    }
}

impl Platform for HeadlessPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.background.clone()
    }
    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground.clone()
    }
    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        Arc::new(NoopTextSystem::new())
    }
    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        on_finish_launching()
    }
    fn quit(&self) {}
    fn restart(&self, _: Option<PathBuf>, _: Vec<OsString>) {}
    fn activate(&self, _: bool) {}
    fn hide(&self) {}
    fn hide_other_apps(&self) {}
    fn unhide_other_apps(&self) {}
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        vec![Rc::new(HeadlessDisplay)]
    }
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(Rc::new(HeadlessDisplay))
    }
    fn on_displays_changed(&self, _: Box<dyn FnMut()>) {}
    fn active_window(&self) -> Option<AnyWindowHandle> {
        None
    }
    fn open_window(
        &self,
        _handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let window = Rc::new(HeadlessWindow {
            bounds: options.bounds,
            ..Default::default()
        });
        self.windows.borrow_mut().push(window.clone());
        Ok(Box::new(OpenWindow(window)))
    }
    fn window_appearance(&self) -> WindowAppearance {
        WindowAppearance::Light
    }
    fn open_url(&self, _: &str) {}
    fn on_open_urls(&self, _: Box<dyn FnMut(Vec<String>)>) {}
    fn register_url_scheme(&self, _: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
    fn prompt_for_paths(
        &self,
        _: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let (answer, answered) = oneshot::channel();
        answer.send(Ok(None)).ok();
        answered
    }
    fn prompt_for_new_path(
        &self,
        _: &Path,
        _: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let (answer, answered) = oneshot::channel();
        answer.send(Ok(None)).ok();
        answered
    }
    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }
    fn reveal_path(&self, _: &Path) {}
    fn open_with_system(&self, _: &Path) {}
    fn on_quit(&self, _: Box<dyn FnMut() -> bool>) {}
    fn on_reopen(&self, _: Box<dyn FnMut()>) {}
    fn on_system_sleep(&self, _: Box<dyn FnMut()>) {}
    fn on_system_wake(&self, _: Box<dyn FnMut()>) {}
    fn set_menus(&self, _: Vec<Menu>, _: &Keymap) {}
    fn set_dock_menu(&self, _: Vec<MenuItem>, _: &Keymap) {}
    fn on_app_menu_action(&self, _: Box<dyn FnMut(&dyn Action)>) {}
    fn on_will_open_app_menu(&self, _: Box<dyn FnMut()>) {}
    fn on_validate_app_menu_command(&self, _: Box<dyn FnMut(&dyn Action) -> bool>) {}
    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }
    fn on_thermal_state_change(&self, _: Box<dyn FnMut()>) {}
    fn prevent_idle_sleep(&self, _: &str) -> Task<Result<gpui::ActivityGuard>> {
        Task::ready(Ok(gpui::ActivityGuard::noop()))
    }
    fn app_path(&self) -> Result<PathBuf> {
        Ok(PathBuf::from("/"))
    }
    fn path_for_auxiliary_executable(&self, _: &str) -> Result<PathBuf> {
        Ok(PathBuf::from("/"))
    }
    fn set_cursor_style(&self, _: CursorStyle) {}
    fn hide_cursor_until_mouse_moves(&self) {}
    fn is_cursor_visible(&self) -> bool {
        true
    }
    fn should_auto_hide_scrollbars(&self) -> bool {
        false
    }
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.clipboard.borrow().clone()
    }
    fn write_to_clipboard(&self, item: ClipboardItem) {
        *self.clipboard.borrow_mut() = Some(item);
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn read_from_primary(&self) -> Option<ClipboardItem> {
        None
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn write_to_primary(&self, _: ClipboardItem) {}
    #[cfg(target_os = "macos")]
    fn read_from_find_pasteboard(&self) -> Option<ClipboardItem> {
        None
    }
    #[cfg(target_os = "macos")]
    fn write_to_find_pasteboard(&self, _: ClipboardItem) {}
    fn write_credentials(&self, _: &str, _: &str, _: &[u8]) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
    fn read_credentials(&self, _: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Ok(None))
    }
    fn delete_credentials(&self, _: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(HeadlessLayout)
    }
    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }
    fn on_keyboard_layout_change(&self, _: Box<dyn FnMut()>) {}
}

struct OpenWindow(Rc<HeadlessWindow>);

impl HasWindowHandle for OpenWindow {
    fn window_handle(&self) -> Result<RawWindowHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }
}

impl HasDisplayHandle for OpenWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }
}

#[derive(Default)]
struct HeadlessAtlas(parking_lot::Mutex<(u32, HashMap<AtlasKey, AtlasTile>)>);

impl PlatformAtlas for HeadlessAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, std::borrow::Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let mut state = self.0.lock();
        if let Some(tile) = state.1.get(&key) {
            return Ok(Some(*tile));
        }
        let Some((size, _)) = build()? else {
            return Ok(None);
        };
        state.0 += 1;
        let tile = AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: AtlasTextureKind::Monochrome,
            },
            tile_id: TileId(state.0),
            padding: 0,
            bounds: Bounds::new(Point::default(), size),
        };
        state.1.insert(key, tile);
        Ok(Some(tile))
    }
    fn remove(&self, key: &AtlasKey) {
        self.0.lock().1.remove(key);
    }
}

impl PlatformWindow for OpenWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.0.bounds
    }
    fn is_maximized(&self) -> bool {
        false
    }
    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Windowed(self.0.bounds)
    }
    fn content_size(&self) -> Size<Pixels> {
        self.0.bounds.size
    }
    fn resize(&mut self, _: Size<Pixels>) {}
    fn scale_factor(&self) -> f32 {
        1.
    }
    fn appearance(&self) -> WindowAppearance {
        WindowAppearance::Light
    }
    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(Rc::new(HeadlessDisplay))
    }
    fn mouse_position(&self) -> Point<Pixels> {
        Point::default()
    }
    fn modifiers(&self) -> Modifiers {
        Modifiers::default()
    }
    fn capslock(&self) -> Capslock {
        Capslock::default()
    }
    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        *self.0.input_handler.borrow_mut() = Some(input_handler);
    }
    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.0.input_handler.borrow_mut().take()
    }
    fn prompt(
        &self,
        _: PromptLevel,
        _: &str,
        _: Option<&str>,
        _: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        None
    }
    fn activate(&self) {}
    fn is_active(&self) -> bool {
        true
    }
    fn visibility(&self) -> WindowVisibility {
        WindowVisibility::Visible
    }
    fn is_hovered(&self) -> bool {
        false
    }
    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }
    fn set_title(&mut self, _: &str) {}
    fn set_background_appearance(&self, _: WindowBackgroundAppearance) {}
    fn minimize(&self) {}
    fn zoom(&self) {}
    fn toggle_fullscreen(&self) {}
    fn is_fullscreen(&self) -> bool {
        false
    }
    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        *self.0.request_frame.borrow_mut() = Some(callback);
    }
    fn on_input(&self, _: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {}
    fn on_active_status_change(&self, _: Box<dyn FnMut(bool)>) {}
    fn on_visibility_change(&self, _: Box<dyn FnMut(WindowVisibility)>) {}
    fn on_hover_status_change(&self, _: Box<dyn FnMut(bool)>) {}
    fn on_resize(&self, _: Box<dyn FnMut(Size<Pixels>, f32)>) {}
    fn on_moved(&self, _: Box<dyn FnMut()>) {}
    fn on_display_changed(&self, _: Box<dyn FnMut()>) {}
    fn on_should_close(&self, _: Box<dyn FnMut() -> bool>) {}
    fn on_hit_test_window_control(&self, _: Box<dyn FnMut() -> Option<WindowControlArea>>) {}
    fn on_close(&self, _: Box<dyn FnOnce()>) {}
    fn on_appearance_changed(&self, _: Box<dyn FnMut()>) {}
    fn draw(&self, _: &Scene) {}
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        Arc::new(HeadlessAtlas::default())
    }
    fn is_subpixel_rendering_supported(&self) -> bool {
        false
    }
    fn gpu_specs(&self) -> Option<GpuSpecs> {
        None
    }
    fn update_ime_position(&self, _: Bounds<Pixels>) {}
    fn a11y_init(&self, callbacks: A11yCallbacks) {
        *self.0.a11y.borrow_mut() = Some(callbacks);
    }
    fn a11y_tree_update(&self, tree_update: accesskit::TreeUpdate) {
        *self.0.tree.borrow_mut() = Some(tree_update);
    }
}
