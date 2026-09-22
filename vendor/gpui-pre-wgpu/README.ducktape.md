# Linux color emoji patch

This is the crates.io `gpui-pre-wgpu` 0.3.5 source used by the existing
application lockfile, with its Apache-2.0 license retained.

Source: https://static.crates.io/crates/gpui-pre-wgpu/gpui-pre-wgpu-0.3.5.crate
Archive SHA-256: `640b666a16ddf9504e2eb3e7a997acb2b2adfaa7859925bdd36fc3acfd8f30a7`

The only behavioral change is in `CosmicTextSystemState::load_family`:
known color emoji fonts bypass the check requiring a Latin `m`. Noto Color
Emoji intentionally has no `m`; the old check removed its successfully
registered fontdb face before constructing the user fallback chain.
Family spelling, registration timing, and CBDT parsing were correct.
Replacing the font with another emoji-only build cannot fix that check.

Upstream needs the same exemption (using `check_is_known_emoji_font`) before
`remove_face`. The existing emoji rasterizer then selects the CBDT color
bitmap without further changes. Remove this local patch when the locked
upstream release contains the fix.

The application regression tests in `app/src/tests/font_fallback.rs` run the
real Linux text system with only bundled fonts. They assert emoji run font
identity, glyph identity and the color-rasterization flag, and exercise all
twelve static text faces. Upstream tests are retained.

## Mechanical source layout

To keep source files below 600 lines, these original files are split at
existing item/method boundaries. Module-level `include!` preserves the
original namespace; renderer methods only gain enclosing `impl` blocks.

- `cosmic_text_system.rs`: state implementation in
  `cosmic_text_system_state.rs`, tests in `cosmic_text_system_tests.rs`.
- `wgpu_renderer.rs`: implementation in `wgpu_renderer_{init,pipelines,
  frame,instances,surface}.rs`.
- `wgpu_context.rs`: tests in `wgpu_context_tests.rs`.
- `shaders.wgsl`: shared helpers remain here; quad shaders move to
  `shaders_quads.wgsl`, remaining primitives to `shaders_primitives.wgsl`.
  The three existing shader constants concatenate these pieces in their
  original order, preserving the exact shader bytes.

These extractions do not change behavior. Compare the concatenated sections
with the upstream archive when refreshing the patch.
