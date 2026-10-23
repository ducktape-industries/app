//! Which node the page reads: `/config.json`'s `node` where the server gives one, else the page's own origin where it
//! answers `/v1/status`, else the node this build names (`DUCKTAPE_WEB_NODE` at build time, else duckpond's).

use wasm_bindgen::JsCast as _;
use wasm_bindgen_futures::JsFuture;

const BUILT_IN_NODE: &str = match option_env!("DUCKTAPE_WEB_NODE") {
    Some(node) => node,
    None => "https://duckpond.ducktape.industries",
};

pub fn page() -> web_sys::Window {
    web_sys::window().expect("the shell runs in a page")
}

pub async fn choose() -> String {
    if let Some(node) = configured().await {
        return node;
    }
    let origin = page().location().origin().unwrap_or_default();
    let origin_answers = noded_client::Client::new(origin.clone())
        .status()
        .await
        .is_ok();
    match origin_answers {
        true => origin,
        false => BUILT_IN_NODE.to_owned(),
    }
}

async fn configured() -> Option<String> {
    let response: web_sys::Response = JsFuture::from(page().fetch_with_str("/config.json"))
        .await
        .ok()?
        .dyn_into()
        .ok()?;
    if !response.ok() {
        return None;
    }
    let text = JsFuture::from(response.text().ok()?)
        .await
        .ok()?
        .as_string()?;
    let config: serde_json::Value = serde_json::from_str(&text).ok()?;
    config.get("node")?.as_str().map(str::to_owned)
}
