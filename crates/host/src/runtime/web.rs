//! Which view may draw a web page: the one whose manifest declares `web`.
//! Each manifest read says whether it does ([`seen`]), and an http(s) link
//! opens in that view when this build has a web view ([`view_for`]). The
//! page itself is the window's (`render/web.rs`).
use std::collections::BTreeSet;
use std::sync::Mutex;
use view_wire::methods::Capability;

/// The programs whose view's manifest declares `web`, as last read.
fn web_views() -> &'static Mutex<BTreeSet<&'static str>> {
    static VIEWS: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    &VIEWS
}

/// `program`'s view manifest was read: whether it declares `web`.
pub fn seen(program: &'static str, capabilities: &[Capability]) {
    let mut views = web_views().lock().expect("web views");
    match capabilities.contains(&Capability::Web) {
        true => views.insert(program),
        false => views.remove(program),
    };
}

/// The view an http(s) `link` opens in: the program whose view declares
/// `web` (the first by name, if more do), when this build has a web view
/// and the address is one a page loads. `None`: the system browser's.
pub fn view_for(link: &str) -> Option<&'static str> {
    if !loadable(link) {
        return None;
    }
    web_views().lock().expect("web views").first().copied()
}

fn loadable(link: &str) -> bool {
    #[cfg(feature = "web")]
    return ducktape_web::loadable(link);
    #[cfg(not(feature = "web"))]
    {
        let _ = link;
        false
    }
}
