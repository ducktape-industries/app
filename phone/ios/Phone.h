#include <stdint.h>

// The Rust library's entries the launcher calls (`crates/phone/src/os/ios/ffi.rs`): runs the phone over the app
// container's Documents and Caches paths, and never returns.
void ducktape_phone_main(const char *documents, const char *caches);

// Whether the swipe from anywhere on the screen begins (`App.swift` `SwipeBack`): its finger came down at (x, y) in the
// window's points, moves at (vx, vy) points per second, rested `held` seconds first; 1 when it begins. Main thread,
// inside UIKit's touch handling: it reads only what the last draw published.
uint8_t ducktape_phone_back_begins(double x, double y, double vx, double vy, double held);

// A swipe back under way, `x` points in from where it started, moving `speed` points per second to the right; then
// let go (Rust judges Back), or ended without going back. Main thread.
void ducktape_phone_back_drag(double x, double speed);
void ducktape_phone_back_let_go(void);
void ducktape_phone_back_cancel(void);

// A call's Answer (`App.swift` `Ring`): its `duck://` link, opened as a link the OS hands over. Main thread.
void ducktape_phone_link(const char *url);

// A program's notice the OS showed (`App.swift` `Ring`): its payload as JSON, the Unix seconds it was shown at; `front`
// 1 when it came while the app was in front, `opened` 1 when the person opened it. 1 when the OS shows one that came in
// front (unless this phone banners it itself: the app in front on the push's chain). Main thread.
uint8_t ducktape_phone_notice(const char *payload, double at, uint8_t front, uint8_t opened);

// This device's push token of `kind` ("voip": PushKit's, as hex), for the device registry; "" when withdrawn.
void ducktape_phone_push_token(const char *kind, const char *token);

// CallKit's audio session up (1) or down (0), for the voice the host plays and records. Main thread.
void ducktape_phone_call_audio(uint8_t active);
