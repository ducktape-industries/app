// The Rust library's entries the launcher calls (`crates/phone/src/os/ios/ffi.rs`): runs the phone over the app
// container's Documents and Caches paths, and never returns.
void ducktape_phone_main(const char *documents, const char *caches);

// Back, as the left edge's swipe asks it (`App.swift` `EdgeBack`); main thread.
void ducktape_phone_back(void);

// The left edge's swipe under way, `x` points in from where it started; then, let go short of Back. Main thread.
void ducktape_phone_back_drag(double x);
void ducktape_phone_back_cancel(void);
