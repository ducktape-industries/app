# ducktape-gpui-linux

gpui-pre-linux 0.3.9 as published (zed's `gpui_linux` at zed@93d7456), renamed, with the changes
below. The app builds its Linux `Application` on it (`gpui_kit::Application::with_platform(
ducktape_gpui_linux::current_platform(false))`, `src/shell/launch.rs`).

Source: https://static.crates.io/crates/gpui-pre-linux/gpui-pre-linux-0.3.9.crate
Archive SHA-256: `e28213c3fde6f13e91ba0b74b90cbaadec9373e36a031eda98561d31059870f3`.
The commit that adds this directory is the unpacked archive exactly (`diff -r` against it prints
nothing); every change after it is ours. Published sources and the Apache-2.0 license retained.

## Changes

- **Name.** Package `ducktape-gpui-linux`, library `ducktape_gpui_linux`, `publish = false`.
- **XIM client.** `xim` is our `vendor/xim` (`ducktape-xim`: zed-xim 0.4.0-zed with Korean and
  Chinese decoding and no panic on text it cannot read; its own README.ducktape.md).
- **AccessKit.** `accesskit` 0.25.1 and `accesskit_unix` 0.24.0 (published: 0.24.0 and 0.22), the
  versions the app's gpui-pre core uses; they move with that core.
- **A window opens where it is asked (X11).** It is created at `WindowParams::bounds`' origin
  (published: 2 px right of it), its `WM_NORMAL_HINTS` carry that origin as a program-specified
  position (published: none, so the window manager placed it) and static gravity, so the x/y name
  where the client area sits, which is what `bounds` reports. A `ConfigureNotify` reads the
  window's origin from the server (`TranslateCoordinates` to the root): the event's own is relative
  to the frame under a reparenting window manager, and often stale on a resize.
- **Bounds observers hear full screen and maximize** (X11 `_NET_WM_STATE`, Wayland configure): a
  state change with no size change still calls the window's `moved` callback.
- **A held key repeats as held (X11).** The client asks XKB for detectable auto-repeat at connect,
  so no release comes before a repeat; `X11ClientState::keys_down` holds the keys pressed and not
  released, a press of one of them is reported `is_held`, and the set is cleared on `FocusOut`.
  Published reported every repeat `is_held: false`. The app's `a11y::HeldKeys` relies on it.
- **Clipboard reads turn the app's loop while they wait.** `set_clipboard_wait(fn())` (crate root)
  stores a function the X11 read (`read_single`'s no-event arm, before its 1 ms sleep) and the
  Wayland read (`read_fd_with_timeout`, its wait cut into 1 ms polls within the same per-chunk
  timeout) call while they wait: an owner in this process whose events a main-thread loop handles
  (CEF's, through glib: app `crates/web/src/glib.rs`) answers only while that loop turns. Nothing
  changes unless it is set. `wait_runs_between_polls_until_the_writer_answers` pins it.

The published `Cargo.lock` is dropped: as a path dependency this crate resolves in the app's
lock. `vendor/` is outside the app's workspace, so the app's gates neither lint nor test it. Its
own tests run in a scratch tree whose root `Cargo.toml` lists `vendor/gpui-linux` among the
workspace members (and drops `exclude`): `cargo test -p ducktape-gpui-linux --features test-support --lib`.

## On a gpui-pre bump

Replace this directory with the new published archive (one commit, the archive as is), record its
SHA-256 here, then re-apply the changes above (one commit), dropping each the new release already
does.
