# ducktape-gpui-ios

zed's `crates/gpui_ios` at `a47f721832e5b6ecf977891c1b7c6aac1406723e` (zed #63068 "gpui_ios: Add iOS
platform backend", merged 2026-10-14), on the gpui-pre 0.3.9 names, with the changes below. No
`gpui-pre-ios` crate is published. The phone builds its `Application` on it
(`crates/phone/src/os/ios/ffi.rs`: `Application::with_platform(Rc::new(IosPlatform::new()))`).

The commit that adds this directory is the gpui-pre fork's `vendor/gpui-pre-ios` at eef11e96 as it
was (its one symlink, `LICENSE-APACHE`, copied as the file it named); the commit after it is ours.

## How it differs from zed's crate

- `Cargo.toml`: a standalone manifest. Package `ducktape-gpui-ios`, library `ducktape_gpui_ios`.
  `gpui` is `gpui-pre =0.3.9`, `gpui_apple` is `gpui-pre-apple =0.3.9` (published, file-for-file
  zed's `gpui_apple` at the snapshot rev); the other dependency lines are zed's workspace lines at
  `a47f721832` inlined, and zed's `[workspace.lints]` inlined as `[lints]`.
  `crate-type = ["staticlib", "rlib"]`, every dependency under `cfg(target_os = "ios")`, as upstream.
  zed's `examples/ios` is left out (a package of its own that patched the fork by path).
- `src/ios/display.rs`: zed's `unsafe impl Send for IosDisplay {}` / `unsafe impl Sync for IosDisplay {}`
  are deleted. `IosDisplay` wraps a `Retained<UIScreen>`, a `MainThreadOnly` class, and nothing needs
  the bounds (every use is `Rc<dyn PlatformDisplay>` on the main thread).
- `src/gpui_ios.rs`, `src/ios.rs`, `src/ios/window.rs`, `src/ios/text_input.rs`: `keyboardType` and
  `isSecureTextEntry` answer the keyboard the phone last told the crate with
  `ios::set_keyboard(Keyboard { kind, secure })` (`KeyboardKind` -> `UIKeyboardType`; zed answers
  `Default` and not secure always), and a change while the keyboard is up reloads it. The core's
  `TextInputConfiguration` is read only for the fields published gpui has (autocorrect, autocapitalize,
  suggestions, input action). The harness test asserts the default, the told keyboard, and that telling
  the same one again reloads nothing.
- `tests/text_input.rs`: it includes `src/ios/text_input.rs` by path, so it imports the crate's `Keyboard`
  and `KeyboardKind` at its root, where that file's `use crate::{Keyboard, KeyboardKind}` finds them.
- `src/ios/window.rs`, `src/ios/application.rs`: the display link is demand-driven. `IosWindow`
  implements `frame_waker` and `schedule_frame` (zed's has neither), `renderFrame:` pauses the link
  after a tick in which no window asked for another frame, and a window's next demand resumes it
  while the scene is in front. Zed's link ticks every window on every vsync while the app is in front:
  ~120 main-thread wakeups a second on a still screen in the simulator, 2 a second with this.
- `src/ios/application.rs`: the app delegate hands Rust the remote-notification device token and a
  background push's payload (`IosPlatform`'s hooks; the phone's push), which zed's has neither of.
- `src/gpui_ios.rs`, `src/ios.rs`, `src/ios/window.rs`, `src/ios/text_input.rs`, `Cargo.toml`:
  `ios::show_edit_menu(at, items, pick)` and `ios::hide_edit_menu()` show UIKit's own edit menu
  beside a text field's selection, which zed's crate has none of. The text input view adds a
  `UIEditMenuInteraction` (iOS 16 and later; `UIMenuController` before) whose target rect is `at`,
  answers `canPerformAction:withSender:` for `cut:`, `copy:`, `paste:` and `selectAll:` with the
  `EditItem`s offered (and no other action while the menu is up), and hands a pick to `pick`. The
  manifest adds objc2-ui-kit's `UIEditMenuInteraction`, `UIInteraction` and `UIMenuController`
  features. The harness test asserts what the view answers before, during and after a menu, and
  that a pick reaches `pick` once.

The rest of `src/` and `tests/` is verbatim.

## `unsafe` in this crate

The crate has 29 `unsafe {}` blocks and no `unsafe impl`, none with a `// SAFETY:` comment, and zed's
`[lints]` neither deny `unsafe_code` nor ask for `clippy::undocumented_unsafe_blocks`. They are not
annotated here on purpose: the code is verbatim zed and is refreshed from zed on every bump. The
blocks were reviewed one by one on 2026-10-15 and found sound as used; a refresh that changes any
`unsafe` site re-does that review.

**The phone profile builds with `panic = "abort"`** (the app's `[profile.phone]`). objc2 0.6's
`define_class!` methods are `extern "C-unwind"`, and no ObjC entry point here has an unwind guard, so
a Rust panic reached from one would otherwise unwind into UIKit frames.

## What it needs from gpui core

It compiles only against a gpui core that carries #63068's core half (the gpui-pre fork's "ios: the
#63068 core half" until a gpui-pre release ships it): `App::{on_app_lifecycle, on_memory_warning}`,
`AppLifecyclePhase::Disconnected`, `Window::insets()`, `Window::{notify_text_input_changed,
notify_text_selection_changed}`, `PlatformInputHandler::{focus_id, query_accepts_focused_text_input,
selection_bounds_for_range}`, a public `Window::last_input_was_touch`, and
`focused_text_input: Option<FocusId>`.

## Build (Mac only)

`vendor/` is outside the app's workspace; this crate builds only for iOS, so no Linux gate compiles
it. From the app's root on a Mac: `IPHONEOS_DEPLOYMENT_TARGET=15.0 cargo build -p ducktape-gpui-ios
--target aarch64-apple-ios-sim`, or the phone itself (`phone/Makefile`). The harness test
(`tests/text_input.rs`) needs the crate's dev-dependencies, which cargo reads only for a workspace
member: in a local copy that adds `vendor/gpui-ios` to the root's `members`, `cargo test -p
ducktape-gpui-ios --target aarch64-apple-ios-sim` with a simulator booted and
`CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER="xcrun simctl spawn booted"`, since cargo would otherwise
run the simulator binary on macOS, where dyld refuses it ("DYLD_ROOT_PATH not set for simulator
program"). `backtrace` 0.3.76 needs
libc <= 0.2.189 on iOS, which the app's lock holds.

## On a bump

Refresh from zed's `crates/gpui_ios` at the new snapshot's zed rev (or a published crate, if one
appears), re-apply the changes above, and build the phone.
