//! A macOS bundle's CEF helper apps: each runs one child process (renderer,
//! GPU, utility) and nothing of the app.
fn main() {
    std::process::exit(ducktape_web::child().unwrap_or(1));
}
