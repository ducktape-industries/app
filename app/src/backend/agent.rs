use super::*;
use gpui_kit::base::text::{TextView, TextViewState};
use gpui_kit::{
    App, AppContext, ClickEvent, Context, Entity, EventEmitter, ImageCache, ImageCacheError,
    IntoElement, MouseButton, ParentElement, Render, RenderImage, Resource, Styled, Window, div,
};

const LINK_TOKEN_BYTES: u64 = 4 * 1024;

/// The module/resource address the shell hands to a generic layer when a run
/// is opened outside the Agents tab. The run itself remains the view's
/// domain identity; this is only the host key that lets close/minimize/restore
/// find the same log layer again.
pub(crate) const SESSION_LOG_MODULE: &str = "agents";

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct SessionLogInstance {
    /// The stable machine resource selected by the Agents view.
    pub(crate) resource: String,
}

impl SessionLogInstance {
    pub(crate) fn for_dispatch(dispatch: &str) -> Option<Self> {
        (!dispatch.is_empty()).then(|| Self {
            resource: format!("runs/{dispatch}"),
        })
    }

    /// A layer identity is the module plus its resource, not an open counter.
    /// Re-opening a run therefore restores its existing layer.
    pub(crate) fn key(&self) -> String {
        format!("{SESSION_LOG_MODULE}:{}", self.resource)
    }
}

pub(crate) fn agent_ws_url(rpc: &str) -> String {
    let base = if let Some(rest) = rpc.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = rpc.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        rpc.to_string()
    };
    format!("{}/v1/ws", base.trim_end_matches('/'))
}

pub(crate) fn read_link_token(workspace: &Path) -> Result<String, String> {
    let path = workspace.join("service-link.token");
    let metadata = std::fs::metadata(&path).map_err(|_| {
        "The node's agent event token is not available in its workspace.".to_string()
    })?;
    if metadata.len() > LINK_TOKEN_BYTES {
        return Err("The node's agent event token is unexpectedly large.".into());
    }
    let token = std::fs::read_to_string(path)
        .map_err(|_| "The node's agent event token could not be read.".to_string())?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("The node's agent event token is empty.".into());
    }
    Ok(token)
}

/// The Markdown surface owns its selection and parse across host frames.
pub struct MarkdownSurface {
    source: String,
    doc: String,
    dark: bool,
    text: Entity<TextViewState>,
    pictures: Entity<ParkedPictures>,
}
impl MarkdownSurface {
    pub fn new(source: String, doc: String, dark: bool, cx: &mut Context<Self>) -> Self {
        let text = cx.new(|cx| TextViewState::markdown(&source, cx));
        let pictures = cx.new(|_| ParkedPictures { doc: doc.clone() });
        Self {
            source,
            doc,
            dark,
            text,
            pictures,
        }
    }
    pub fn replace(&mut self, source: String, doc: String, dark: bool, cx: &mut Context<Self>) {
        if self.source != source {
            self.text
                .update(cx, |state, cx| state.set_text(&source, cx));
            self.source = source;
        }
        self.doc = doc.clone();
        self.dark = dark;
        self.pictures.update(cx, |pictures, _| pictures.doc = doc);
        cx.notify();
    }
}
impl EventEmitter<view_wire::SurfaceValue> for MarkdownSurface {}
impl Render for MarkdownSurface {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        div().w_full().image_cache(self.pictures.clone()).child(
            TextView::new(&self.text)
                .selectable(true)
                .scrollable(false)
                .on_link_click(move |url, event, _, cx| {
                    let opens = match event {
                        ClickEvent::Mouse(click) => {
                            matches!(click.up.button, MouseButton::Left | MouseButton::Middle)
                        }
                        ClickEvent::Keyboard(_) => true,
                        ClickEvent::Touch(click) => !click.long_press,
                    };
                    if opens {
                        let _ = view.update(cx, |_, cx| {
                            cx.emit(view_wire::SurfaceValue::Str(url.to_string()))
                        });
                    }
                }),
        )
    }
}

/// Markdown never fetches URLs itself. Only images already admitted by the
/// repository loader can be read, preserving its address and byte limits.
struct ParkedPictures {
    doc: String,
}
impl ImageCache for ParkedPictures {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let picture = match resource {
            Resource::Uri(uri) => super::picture::inline_picture(&self.doc, uri.as_ref()),
            _ => None,
        };
        let Some(picture) = picture else {
            return Some(Err(ImageCacheError::Asset("Image unavailable".into())));
        };
        match picture.handle {
            super::picture::PictureHandle::Raster(image) => Some(Ok(image)),
            super::picture::PictureHandle::Vector(image) => {
                image.use_render_image(window, cx).map(Ok)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_log_identity_is_stable_for_a_dispatch() {
        let first = SessionLogInstance::for_dispatch("dispatch-1").expect("non-empty dispatch");
        let second = SessionLogInstance::for_dispatch("dispatch-1").expect("non-empty dispatch");

        assert_eq!(first, second);
        assert_eq!(first.resource, "runs/dispatch-1");
        assert_eq!(first.key(), "agents:runs/dispatch-1");
    }

    #[test]
    fn an_empty_dispatch_does_not_open_a_log_layer() {
        assert!(SessionLogInstance::for_dispatch("").is_none());
    }
}
