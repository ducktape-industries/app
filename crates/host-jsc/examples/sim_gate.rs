//! The simulator gate (wasm-mobile SPEC §13.3 (b)): an iOS app that runs
//! the recorded chat and forge sessions through [`JscRunner`], the views'
//! real code metered by `prepare` and run in the page, and holds every
//! number to the wasmtime reference (`gate.rs`'s test pins them): the fuel
//! over the session, every recorded frame, the frame hash after a restore.
//! Then a death: it prints `KILL_ME pid <n>` for the outside (`kill -9` of
//! the WebContent process, `crates/host-jsc/sim-gate.sh`), and once the
//! page is dead revives the view as the host does, the frames and fuel
//! after it held to wasmtime's revive. Every result is a `GATE`
//! line on the console; the last is `GATE PASS` or `GATE FAIL`.
//!
//! Built for the simulator only (`--features gate --target
//! aarch64-apple-ios-sim`), bundled and run by `sim-gate.sh`.
#![allow(
    clippy::print_stdout,
    reason = "the gate's console lines are its output"
)]

#[cfg(target_os = "ios")]
mod app {
    use ducktape_host::contract::RunnerConfig;
    use ducktape_host_jsc::{JscRunner, gate};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::NSString;
    use objc2_ui_kit::{UIApplication, UIApplicationDelegate, UIViewController, UIWindow};
    use std::cell::RefCell;
    use std::time::Duration;

    thread_local! {
        /// The gate's one window, kept for the process.
        static WINDOW: RefCell<Option<Retained<UIWindow>>> = const { RefCell::new(None) };
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "DucktapeSimGate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UIApplicationDelegate for Delegate {
            #[unsafe(method(application:didFinishLaunchingWithOptions:))]
            fn launched(&self, _: &UIApplication, _: Option<&AnyObject>) -> bool {
                launch(self.mtm());
                true
            }
        }
    );

    fn line(text: String) {
        println!("GATE {text}");
    }

    /// A window as the phone has one, the runner, and the gate on a thread
    /// of its own (the runner's answers land on this one).
    fn launch(mtm: MainThreadMarker) {
        let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(390., 844.));
        #[expect(
            deprecated,
            reason = "the gate has no scene; the phone's window is gpui_ios's"
        )]
        let window = UIWindow::initWithFrame(UIWindow::alloc(mtm), frame);
        window.setRootViewController(Some(&UIViewController::new(mtm)));
        window.makeKeyAndVisible();
        WINDOW.with(|kept| *kept.borrow_mut() = Some(window));
        let caches = std::env::temp_dir().join("view-code");
        let runner = match JscRunner::new(RunnerConfig {
            signals_based_traps: false,
            cache_dir: Some(caches),
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
        let seen = runner.deaths();
        let kill = || {
            let pid = runner.content_process().ok_or("no content process")?;
            line(format!("KILL_ME pid {pid}"));
            let deaths = runner.wait_for_death(seen);
            line(format!("DIED deaths {deaths}"));
            Ok(())
        };
        match gate::revive(runner, chat, Some(&kill)) {
            Ok(revived) => {
                line(format!(
                    "death: the tick begun on the dead page landed {}; after the revive frames {} fuel {} (wasmtime's revive {} {})",
                    revived.dead_tick,
                    revived.after.0,
                    revived.after.1,
                    chat.revived.0,
                    chat.revived.1,
                ));
                if !revived.dead_tick.starts_with("Runner: ")
                    || (revived.after.0.as_str(), revived.after.1) != chat.revived
                {
                    failures.push("the revive differs from wasmtime's".into());
                }
            }
            Err(why) => failures.push(format!("death: {why}")),
        }
        failures
    }

    pub fn main() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let delegate = NSString::from_class(Delegate::class());
        UIApplication::main(None, Some(&delegate), mtm);
    }
}

fn main() {
    #[cfg(target_os = "ios")]
    app::main();
}
