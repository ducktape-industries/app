//! Linux: Chromium's X11 event source is a source on glib's default main
//! context. With CEF's external pump nothing else runs that context (gpui's
//! loop is calloop), so Chromium never heard its X events: another program
//! asking for the clipboard it owns waited forever. The pump turns the
//! context, as cefclient's Linux pump does. So does gpui's clipboard read
//! (X11 and Wayland) while it waits on this thread for an answer
//! (`gpui_linux::set_clipboard_wait`): else a page's copy pasted into an
//! app field froze the app (4 s per format asked).
//!
//! Why a turn is safe inside a gpui read: it runs no Chromium task. CEF's
//! external pump never enters `MessagePumpGlib::Run`, so the pump's own
//! glib work source never dispatches (read in Chromium's source, not
//! re-checked at CEF 154); only Chromium's X event and fd sources run, so
//! no page handler is called. What can run is
//! `on_schedule_message_pump_work`, which only sends on a thread-safe
//! channel. Nothing in this crate reads the clipboard, so no `Shared`
//! borrow is live during a read.

use std::os::raw::{c_int, c_void};

#[link(name = "glib-2.0")]
unsafe extern "C" {
    fn g_main_context_iteration(context: *mut c_void, may_block: c_int) -> c_int;
}

/// Most glib dispatches in one turn, so a flood of X events cannot hold
/// the pump.
const MOST: usize = 64;

/// Dispatches what is ready on glib's default context, without waiting.
/// On the main thread, where CEF's UI thread is.
pub(crate) fn turn() {
    for _ in 0..MOST {
        // SAFETY: null is the default context; 0 never blocks
        if unsafe { g_main_context_iteration(std::ptr::null_mut(), 0) } == 0 {
            return;
        }
    }
}
