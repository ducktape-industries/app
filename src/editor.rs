//! The native multi-line text field a view's `Node::Editor` draws
//! (`TextEditor`, `editor/text.rs`, mounted as `wire::text`), and the
//! per-view store (`wire::EditorStore`) that keeps the field's text in step
//! with the document the view owns. The editor is native; only the document
//! belongs to the guest.
pub mod wire;
