# gpui-pre-macos, ours

gpui-pre-macos 0.3.9 as published (zed's `gpui_macos` at zed@93d7456), with the changes below. The
app builds its macOS `Application` on it (`gpui_kit::Application::with_platform(Rc::new(
gpui_macos::MacPlatform::new(false)))`, `src/shell/launch.rs`).

It keeps the published package name, and the app's `[patch.crates-io]` puts it in place of the
published crate, so it is the only `gpui-pre-macos` in the graph: gpui-kit always builds
`gpui-pre-platform`, which names `gpui-pre-macos`, and the crate registers its Objective-C classes
(`GPUIApplication`, `GPUIApplicationDelegate`, ...) with `ClassDecl::new(..).unwrap()` in an eager
`#[ctor]` (`src/platform.rs`). Two copies linked into one binary would register each name twice, and
the second registration panics before `main`.

Source: https://static.crates.io/crates/gpui-pre-macos/gpui-pre-macos-0.3.9.crate
Archive SHA-256: `c7c988b7e49775cc8542dbe48d5a10e073d8e898a023ef3ff3d24f33363e9863`.
The commit that adds this directory is the unpacked archive exactly; every change after it is ours.
Published sources and the Apache-2.0 license retained.

## Changes

- **Manifest.** `publish = false` and the description; name and version stay the published ones
  (the patch needs both). The published `Cargo.lock` is dropped: this crate resolves in the app's
  lock.
- **AccessKit.** `accesskit` 0.25.1 and `accesskit_macos` 0.27.1 (published: 0.24.0 and 0.26.0), the
  versions the app's gpui-pre core uses; they move with that core.
- **Korean input's first key.** On the first key after the input source changes, Apple's Korean
  input method commits the key's bare jamo (`insertText:`) and composes the next keys into it by
  reading it back. A field that reports no selection (the web view's page) cannot give it back, so
  한글 typed as ㅎㅏㄴ글. `interpret_key` replaces the key handler's two `handleEvent:` calls: for a
  key whose character is one compatibility jamo, not a repeat, no Control/Option/Command/Function,
  an Apple Korean input source (`com.apple.inputmethod.Korean.*`) and a field with nothing marked
  and no selection, `insertText:` holds that jamo back while the event is read; if it was the
  reading's only answer, the event is read again and the input method starts the syllable as marked
  text (the held jamo is put down only if the second reading answers nothing). Any other answer to
  the first reading puts the held jamo down first. Traced on macOS 27 with Apple Korean 2-Set;
  winit#4744 reads the key again for the same report (Apple FB17460926).
- **Bounds observers hear the end of full screen.** `window_did_exit_full_screen` calls the window's
  `moved` callback once the animation has ended and the frame is final.

`vendor/` is outside the app's workspace; this crate builds only for macOS.

## On a gpui-pre bump

Replace this directory with the new published archive (one commit, the archive as is), record its
SHA-256 here, then re-apply the changes above (one commit), dropping each the new release already
does.
