# Fixture screenshots (Linux)

This development tool renders serialized `view_wire::Node` trees through the
real app renderer, theme and bundled fonts. It creates no node or network
runtime. The `--render-tree` entry point only exists in debug builds.

Requirements: Python 3, Xvfb, xdotool, ImageMagick (`import`, `identify`,
`convert`), Mesa's lavapipe Vulkan driver, and the app's normal build tools.

From the repository root:

```sh
app/dev/screens/chat-screens.sh FIXTURES_DIR OUTPUT_DIR
```

The included `app/dev/screens/fixtures` directory exercises all twelve bundled
static faces and the four color emoji used by the regression probe.

Set `CARGO_TARGET_DIR` to reuse an existing build directory. Add
`--capture-only` as the third argument to reuse its debug binary without
building. `CHAT_SCREEN_FILTER='09-conversation|12-message|13-reaction'`
selects fixture names with a Python regular expression. The build log and
per-fixture renderer/Xvfb logs are written to `OUTPUT_DIR/logs`.

## Fixture format

`FIXTURES_DIR/manifest.json` is an array:

```json
[
  {
    "name": "font-probe-light",
    "width": 900,
    "height": 300,
    "theme": "light",
    "scroll": "none",
    "hover": false
  }
]
```

Each `name` references `FIXTURES_DIR/<name>.json`, the serde JSON encoding of
one `view_wire::Node` (for example a `Text`, `Linear`, or `Sensor` root).
Generate trees with the matching modules SDK; this tool does not maintain a
second wire schema. Width and height are positive integer pixels; theme is
`light` or `dark`. Optional `scroll` is `none`, `top`, or `bottom`. Optional
`hover: true` places the pointer over the chat message action area after
painting. Additional manifest fields, such as a scenario description, are
ignored.

Trees containing editor references also need `<name>.editors.json`, an
array of `{"document": "<document-id>", "text": "<full text>"}` records.
The text's byte length must match the reference in the tree. The harness
seeds the real editor store using the normal document transfer messages.

Each fixture gets an isolated Xvfb display. Capture waits for first paint
and two unchanged image samples with more than 32 colors, then applies any
scroll/hover actions. It rejects blank final PNGs and returns a failure if
the renderer exits or a stable nonblank image does not appear within 60
seconds. Outputs are `<name>.png`; only processes started by the harness
are terminated. Three fixtures render concurrently.

For a single tree without a manifest, start an X display and run the debug
binary directly:

```sh
"${CARGO_TARGET_DIR:-target}/debug/ducktape-app" \
  --render-tree path/to/tree.json --size 900x300 --theme light
```

## Pane workspace fixtures

A workspace fixture wraps independently rendered trees in pane chrome:

```json
{
  "panes": [
    {"label": "Chat", "context": "#general", "tree": {"Text": {"key": "hello", "content": "Hello", "size": 14}}}
  ],
  "focused": 0,
  "popped_out": false
}
```

Use complete SDK-serialized trees for `tree`. One to three panes are accepted;
`focused` must name an existing pane. A popped-out fixture contains exactly
one pane and omits the rail. Each tree has its own editor store, so repeated
editor identifiers in different panes remain independent. The optional editor
sidecar supplies documents to each store. Raw tree fixtures remain a separate
rendering mode.

The committed pane fixtures embed the existing SDK conversation screenshot
tree, without changing its content or rendering. Their contexts illustrate
independent panes; the node-less host has no real channels or network. The
fixture rail is static chrome, and fixture strip controls are accessibility
nodes for screenshot inspection, not live console actions.

Capture all four 1280×800 proofs using an already built debug binary:

```sh
export RUSTC_WRAPPER=
app/dev/screens/chat-screens.sh app/dev/screens/panes app/dev/screens/panes/proof --capture-only
```

The manifest covers one, two and three panes plus a popped-out view.
