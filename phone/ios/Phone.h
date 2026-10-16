// The Rust library's one entry the launcher calls (`crates/phone/src/os/ios/ffi.rs`): runs the phone over the app
// container's Documents and Caches paths, and never returns.
void ducktape_phone_main(const char *documents, const char *caches);
