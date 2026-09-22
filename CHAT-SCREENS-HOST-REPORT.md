# Host screenshot harness

The debug app entry `--render-tree <json> --size WxH --theme light|dark` renders a deserialized `view_wire::Node` with the real `ViewTree`, GPUI `Root`, bundled product fonts, theme, and desktop font fallback. It returns before normal startup, so no shell model, node, network runtime, tray, or account access starts. Normal startup reuses the extracted font/theme initialization function without behavior changes.

Workspace SDK dependencies point to modules-shots to share wire types. Cargo.lock changes must remain uncommitted.

Run `./scripts/chat-screens.sh` from this worktree to build and capture; `--capture-only` uses the existing debug executable. The parent rerun script first exports fixtures. Inputs are `modules-shots/target/chat-screens/manifest.json` and `<name>.json`; manifest entries carry name, theme, width, height, and how.

Each capture starts an isolated Xvfb with the exact requested dimensions, launches only the renderer entry, waits for a stable painted image with more than 32 colors, and records logs under `chat-screens/logs`. It shuts down only subprocesses it started after verifying `/proc/<pid>/cmdline`. No UI fixes were made.

Validation completed here: bash syntax and Python compilation. Cargo build and rendered screenshots are owned by the parent task; no build ran concurrently from this worker. This report does not assert image quality or runtime success.

Editor projection fidelity: each tree mounts a real `EditorStore` and answers its document requests using `<name>.editors.json` arrays of `{document,text}`. Begin/chunk/complete messages follow the production wire protocol; the host validates bytes and uses the cursor/revisions already present in the tree. Missing nonempty document text fails instead of showing a placeholder. Manifest `scroll: top|bottom` uses native wheel events after initial paint; otherwise the real list honors its own tree anchor.
