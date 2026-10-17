#include <stdint.h>

// The Rust library's entries the launcher calls (`crates/phone/src/os/ios/ffi.rs`): runs the phone over the app
// container's Documents and Caches paths, and never returns.
void ducktape_phone_main(const char *documents, const char *caches);

// Back, as the left edge's swipe asks it (`App.swift` `EdgeBack`); main thread.
void ducktape_phone_back(void);

// The left edge's swipe under way, `x` points in from where it started; then, let go short of Back. Main thread.
void ducktape_phone_back_drag(double x);
void ducktape_phone_back_cancel(void);

// A call's Answer (`App.swift` `Ring`): its `duck://` link, opened as a link the OS hands over. Main thread.
void ducktape_phone_link(const char *url);

// This device's push token of `kind` ("voip": PushKit's, as hex), for the device registry; "" when withdrawn.
void ducktape_phone_push_token(const char *kind, const char *token);

// CallKit's audio session up (1) or down (0), for the voice the host plays and records. Main thread.
void ducktape_phone_call_audio(uint8_t active);
