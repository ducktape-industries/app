//! The simulator gate (wasm-mobile SPEC §13.3 (b)): an iOS app that runs
//! the recorded chat and forge sessions through [`JscRunner`], the views'
//! real code metered by `prepare` and run in the page, and holds every
//! number to the wasmtime reference (`gate.rs`'s test pins them): the fuel
//! over the session, every recorded frame, the frame hash after a restore.
//! Then a death with a tick in flight: on the main thread, so no answer can
//! land meanwhile, it begins the tick, prints `KILL_ME pid <n>` for the
//! outside (`kill -9` of the WebContent process, `crates/host-jsc/
//! sim-gate.sh`) and waits until that process is gone. The tick in flight
//! and one begun after must each land the runner's fault once, and the
//! view revived as the host does must give wasmtime's revive's frames and
//! fuel. Every result is a `GATE` line on the console; the last is `GATE
//! PASS` or `GATE FAIL`.
//!
//! Built for the simulator only (`--features gate --target
//! aarch64-apple-ios-sim`), bundled and run by `sim-gate.sh`.
#![allow(
    clippy::print_stdout,
    reason = "the gate's console lines are its output"
)]

#[cfg(target_os = "ios")]
mod app {
    use ducktape_host::contract::{RunnerConfig, ViewInstance};
    use ducktape_host_jsc::{JscRunner, gate};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
    use objc2::{ClassType, MainThreadOnly, define_class};
    use objc2_foundation::NSString;
    use objc2_ui_kit::{
        UIApplication, UIApplicationDelegate, UIScene, UISceneDelegate, UIViewController, UIWindow,
        UIWindowScene, UIWindowSceneDelegate,
    };
    use std::cell::RefCell;
    use std::time::Duration;

    thread_local! {
        static WINDOW: RefCell<Option<Retained<UIWindow>>> = const { RefCell::new(None) };
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "DucktapeSimGate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UIApplicationDelegate for Delegate {}
    );

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "DucktapeSimGateScene"]
        struct Scene;

        unsafe impl NSObjectProtocol for Scene {}

        unsafe impl UISceneDelegate for Scene {
            #[unsafe(method(scene:willConnectToSession:options:))]
            fn connect(&self, scene: &UIScene, _: &AnyObject, _: &AnyObject) {
                if let Some(scene) = scene.downcast_ref::<UIWindowScene>() {
                    launch(scene);
                }
            }
        }

        unsafe impl UIWindowSceneDelegate for Scene {}
    );

    fn line(text: String) {
        println!("GATE {text}");
    }

    fn launch(scene: &UIWindowScene) {
        let mtm = scene.mtm();
        let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
        window.setRootViewController(Some(&UIViewController::new(mtm)));
        window.makeKeyAndVisible();
        WINDOW.with(|kept| *kept.borrow_mut() = Some(window));
        let caches = std::env::temp_dir().join("view-code");
        let runner = match JscRunner::new(RunnerConfig {
            signals_based_traps: false,
            cache_dir: Some(caches),
            compile_apart: None,
        }) {
            Ok(runner) => runner,
            Err(failure) => {
                line(format!("FAIL the runner: {failure}"));
                std::process::exit(1);
            }
        };
        std::thread::spawn(move || {
            let failures = run(&runner);
            match failures.is_empty() {
                true => line("PASS".into()),
                false => line(format!("FAIL {}", failures.join("; "))),
            }
            std::process::exit(i32::from(!failures.is_empty()));
        });
    }

    fn percentile(sorted: &[Duration], p: usize) -> Duration {
        sorted
            .get(sorted.len().saturating_sub(1) * p / 100)
            .copied()
            .unwrap_or_default()
    }

    fn run(runner: &JscRunner) -> Vec<String> {
        let mut failures = Vec::new();
        let sessions = gate::sessions();
        for session in &sessions {
            match gate::run(runner, session) {
                Ok(ran) => {
                    line(format!(
                        "{} fuel {} (wasmtime {}) frames {}/{} restored {} (wasmtime {}) tick median {:?} p95 {:?}",
                        session.view,
                        ran.fuel,
                        session.fuel,
                        ran.frames.0,
                        ran.frames.1,
                        ran.restored,
                        session.restored,
                        percentile(&ran.ticks, 50),
                        percentile(&ran.ticks, 95),
                    ));
                    if ran.fuel != session.fuel
                        || ran.frames.0 != ran.frames.1
                        || ran.restored != session.restored
                    {
                        failures.push(format!("{} differs from wasmtime", session.view));
                    }
                }
                Err(why) => failures.push(format!("{}: {why}", session.view)),
            }
        }
        let chat = &sessions[0];
        let kill = |instance: &mut dyn ViewInstance, events: Vec<u8>| -> Result<(), String> {
            let pid = runner.content_process().ok_or("no content process")?;
            let seen = runner.deaths();
            let killed = dispatch2::run_on_main(move |_| {
                runner.stall();
                instance.begin_tick(events);
                line(format!("KILL_ME pid {pid}"));
                gone(pid)
            });
            if !killed {
                return Err(format!("the content process {pid} was not killed"));
            }
            let deaths = runner.wait_for_death(seen);
            line(format!("DIED deaths {deaths}"));
            Ok(())
        };
        match gate::revive(runner, chat, Some(&kill)) {
            Ok(revived) => {
                line(format!(
                    "death: the tick in flight landed {}; one begun after it {}; anything more {}; after the revive frames {} fuel {} (wasmtime's revive {} {})",
                    revived.in_flight,
                    revived.begun_after,
                    revived.stray,
                    revived.after.0,
                    revived.after.1,
                    chat.revived.0,
                    chat.revived.1,
                ));
                if !revived.in_flight.starts_with("Runner: ")
                    || !revived.begun_after.starts_with("Runner: ")
                    || revived.stray
                {
                    failures.push("a call into the dead page was not answered once".into());
                }
                if (revived.after.0.as_str(), revived.after.1) != chat.revived {
                    failures.push("the revive differs from wasmtime's".into());
                }
            }
            Err(why) => failures.push(format!("death: {why}")),
        }
        failures
    }

    #[allow(unsafe_code, reason = "libc's kill, with signal 0")]
    fn gone(pid: i32) -> bool {
        let until = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < until {
            // SAFETY: signal 0 sends nothing; it asks whether `pid` exists
            let there = unsafe { libc::kill(pid, 0) } == 0
                || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
            if !there {
                return true;
            }
            std::hint::spin_loop();
        }
        false
    }

    pub fn main() {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let _scene = Scene::class();
        let delegate = NSString::from_class(Delegate::class());
        UIApplication::main(None, Some(&delegate), mtm);
    }
}

fn main() {
    #[cfg(target_os = "ios")]
    app::main();
}
