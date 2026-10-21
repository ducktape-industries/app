# ducktape-xim

zed-xim 0.4.0-zed as published (Riey's xim-rs, zed's fork), renamed, with the changes below. It is
the X11 input-method client of our Linux platform crate (`vendor/gpui-linux`).

Source: https://static.crates.io/crates/zed-xim/zed-xim-0.4.0-zed.crate
Archive SHA-256: `0c0b46ed118eba34d9ba53d94ddc0b665e0e06a2cf874cfa2dd5dec278148642`.
The commit that adds this directory is the unpacked archive exactly; every change after it is ours.
Published sources and the MIT license retained.

## Changes

- **Name.** Package `ducktape-xim`, library `ducktape_xim`, `publish = false`. The published
  `Cargo.lock` is dropped: as a path dependency this crate resolves in the app's lock.
- **Korean and Chinese decode, and nothing the server sends panics.** `xim-ctext` 0.4.1
  (published: 0.3.0) decodes KS C 5601 and GB 2312, which ibus (Ubuntu's default input method)
  sends Hangul and Chinese in through Xlib; the published decoder read neither and `.expect()`ed
  the result, so the first composed syllable crashed the app. `client::decode_text` logs and drops
  text it cannot decode, for a reset, a commit and a preedit draw.
  `hangul_in_ks_c_5601_decodes` and `undecodable_text_is_dropped` pin it.

`vendor/` is outside the app's workspace. Its own tests run in a scratch tree whose root
`Cargo.toml` lists `vendor/xim` among the workspace members (and drops `exclude`), with the network
on for its dev-dependencies: `cargo test -p ducktape-xim --lib --features client`.

## On a bump

Replace this directory with the new published archive (one commit, the archive as is), record its
SHA-256 here, then re-apply the changes above (one commit), dropping each the new release already
does.
