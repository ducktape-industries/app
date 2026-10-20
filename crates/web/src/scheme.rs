//! The `duck` scheme in every page: each `duck://` address answered from what
//! the app reads of its network (`serve`), read-only, and the height a page
//! loaded from one tells its browser.
use cef::*;
use std::os::raw::c_int;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

pub const SCHEME: &str = "duck";

pub(crate) const HEIGHT: &str = "ducktape.height";

const TELL: &str = "__ducktapeHeight";

const WATCH: &str = "(() => { const tell = globalThis.__ducktapeHeight; delete globalThis.__ducktapeHeight; const watch = () => new ResizeObserver(() => tell(Math.ceil(document.documentElement.getBoundingClientRect().height))).observe(document.documentElement); if (document.documentElement) { watch(); } else { document.addEventListener('readystatechange', watch, { once: true }); } })();";

pub struct Served {
    pub status: u16,
    pub mime: String,
    pub body: Vec<u8>,
}

pub type Answer = Box<dyn FnOnce(Served) + Send>;

pub struct Network {
    pub read: Box<dyn Fn(String, Answer) + Send + Sync>,
    pub serves: fn(&str) -> bool,
}

static NETWORK: OnceLock<Network> = OnceLock::new();

pub fn serve(network: Network) {
    let _ = NETWORK.set(network);
}

pub(crate) fn leaves(address: &str) -> bool {
    address.starts_with("duck:")
        && !NETWORK
            .get()
            .is_some_and(|network| (network.serves)(address))
}

pub(crate) fn register(registrar: &mut SchemeRegistrar) {
    let options = [
        SchemeOptions::STANDARD,
        SchemeOptions::SECURE,
        SchemeOptions::CORS_ENABLED,
        SchemeOptions::FETCH_ENABLED,
    ]
    .iter()
    .fold(0, |bits, option| bits | option.get_raw());
    registrar.add_custom_scheme(Some(&SCHEME.into()), options as c_int);
}

pub(crate) fn factory() -> SchemeHandlerFactory {
    DuckFactory::new()
}

pub(crate) fn script() -> RenderProcessHandler {
    PageScript::new()
}

struct Sent(Callback);

unsafe impl Send for Sent {}

#[derive(Default)]
struct Reading {
    served: Option<Served>,
    at: usize,
}

fn held(reading: &Mutex<Reading>) -> MutexGuard<'_, Reading> {
    reading.lock().unwrap_or_else(PoisonError::into_inner)
}

wrap_scheme_handler_factory! {
    struct DuckFactory;

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            _request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            Some(DuckResource::new(Arc::default()))
        }
    }
}

wrap_resource_handler! {
    struct DuckResource {
        reading: Arc<Mutex<Reading>>,
    }

    impl ResourceHandler {
        fn open(
            &self,
            request: Option<&mut Request>,
            handle_request: Option<&mut c_int>,
            callback: Option<&mut Callback>,
        ) -> c_int {
            let (Some(request), Some(handle_request), Some(callback), Some(network)) =
                (request, handle_request, callback, NETWORK.get())
            else {
                return 0;
            };
            *handle_request = 0;
            let address = CefString::from(&request.url()).to_string();
            let reading = self.reading.clone();
            let sent = Sent(callback.clone());
            (network.read)(
                address,
                Box::new(move |served| {
                    held(&reading).served = Some(served);
                    let Sent(callback) = sent;
                    callback.cont();
                }),
            );
            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let reading = held(&self.reading);
            let Some(served) = &reading.served else {
                return;
            };
            if let Some(response) = response {
                response.set_status(c_int::from(served.status));
                response.set_mime_type(Some(&served.mime.as_str().into()));
                response.set_header_by_name(Some(&"Cache-Control".into()), Some(&"no-store".into()), 1);
            }
            if let Some(length) = response_length {
                *length = served.body.len() as i64;
            }
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: c_int,
            bytes_read: Option<&mut c_int>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> c_int {
            let mut reading = held(&self.reading);
            let Reading { served, at } = &mut *reading;
            let rest = served
                .as_ref()
                .map_or(&[][..], |served| &served.body[(*at).min(served.body.len())..]);
            let count = rest.len().min(usize::try_from(bytes_to_read).unwrap_or(0));
            if let Some(bytes_read) = bytes_read {
                *bytes_read = count as c_int;
            }
            if count == 0 || data_out.is_null() {
                return 0;
            }
            unsafe { std::ptr::copy_nonoverlapping(rest.as_ptr(), data_out, count) };
            *at += count;
            1
        }
    }
}

wrap_render_process_handler! {
    struct PageScript;

    impl RenderProcessHandler {
        fn on_context_created(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            let (Some(frame), Some(context)) = (frame, context) else {
                return;
            };
            let address = CefString::from(&frame.url()).to_string();
            if frame.is_main() != 1 || !address.starts_with("duck://") {
                return;
            }
            let Some(global) = context.global() else {
                return;
            };
            let mut told = Told::new(frame.clone());
            let Some(mut tell) = v8_value_create_function(Some(&TELL.into()), Some(&mut told)) else {
                return;
            };
            global.set_value_bykey(Some(&TELL.into()), Some(&mut tell), V8Propertyattribute::default());
            context.eval(Some(&WATCH.into()), Some(&address.as_str().into()), 0, None, None);
        }
    }
}

wrap_v8_handler! {
    struct Told {
        frame: Frame,
    }

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            _retval: Option<&mut Option<V8Value>>,
            _exception: Option<&mut CefString>,
        ) -> c_int {
            let Some(Some(height)) = arguments.and_then(<[_]>::first) else {
                return 0;
            };
            let Some(mut message) = process_message_create(Some(&HEIGHT.into())) else {
                return 0;
            };
            if let Some(arguments) = message.argument_list() {
                arguments.set_int(0, height.double_value().ceil() as c_int);
            }
            self.frame.send_process_message(ProcessId::BROWSER, Some(&mut message));
            1
        }
    }
}
