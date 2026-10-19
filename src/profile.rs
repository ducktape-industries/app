//! One running copy per profile, the config directory every file of the person's lives under:
//! the copy holding `<config>/lock`. A later launch hands its arguments to it over
//! `<config>/run/app.sock` and exits, or waits for the lock when nothing takes them.

use std::fs::{File, TryLockError};
use std::io::{Read as _, Write as _};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use tracing::{info, warn};

const TARGET: &str = "ducktape::app";
const LOCK: &str = "lock";
const RUN: &str = "run";
const SOCKET: &str = "app.sock";
const TAKEN: &[u8] = b"taken";
const TAKEN_WHOLE: &[u8] = b"taken whole";

static HELD: Mutex<Vec<File>> = Mutex::new(Vec::new());

pub(crate) enum Claim {
    Held(Profile),
    HandedOver,
}

pub(crate) struct Profile {
    lock: File,
    listener: Option<UnixListener>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Launch {
    pub(crate) args: Vec<String>,
    pub(crate) activation: Option<String>,
}

const ACTIVATION: [&str; 2] = ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"];

pub(crate) fn claim() -> Result<Claim, String> {
    let dir = crate::backend::config_dir()?;
    let launch = Launch {
        args: std::env::args_os()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        activation: ACTIVATION
            .into_iter()
            .find_map(|name| std::env::var(name).ok().filter(|token| !token.is_empty())),
    };
    claim_at(&dir, &launch).map_err(|error| format!("{}: {error}", dir.display()))
}

pub(crate) fn claim_at(dir: &Path, launch: &Launch) -> std::io::Result<Claim> {
    std::fs::create_dir_all(dir)?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK))?;
    match lock.try_lock() {
        Ok(()) => return Ok(Claim::Held(Profile::bind(dir, lock))),
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Error(error)) => return Err(error),
    }
    if hand_over(&dir.join(RUN).join(SOCKET), launch) {
        info!(target: TARGET, event = "profile_handed_over", "the copy running this profile took this launch");
        return Ok(Claim::HandedOver);
    }
    info!(target: TARGET, event = "profile_waiting", "another copy holds this profile and took nothing: this launch waits for it to quit");
    lock.lock()?;
    Ok(Claim::Held(Profile::bind(dir, lock)))
}

fn hand_over(socket: &Path, launch: &Launch) -> bool {
    match ask(socket, launch) {
        Some(said) if said == TAKEN_WHOLE => true,
        Some(said) if said == TAKEN => ask(socket, &launch.args).is_some_and(|said| said == TAKEN),
        _ => false,
    }
}

fn ask(socket: &Path, asked: &impl serde::Serialize) -> Option<Vec<u8>> {
    let mut stream = UnixStream::connect(socket).ok()?;
    let asked = serde_json::to_vec(asked).ok()?;
    let mut said = Vec::new();
    stream.write_all(&asked).ok()?;
    stream.shutdown(Shutdown::Write).ok()?;
    stream.read_to_end(&mut said).ok()?;
    Some(said)
}

fn read_launch(asked: &[u8]) -> Option<(Launch, &'static [u8])> {
    if let Ok(launch) = serde_json::from_slice::<Launch>(asked) {
        return Some((launch, TAKEN_WHOLE));
    }
    let args = serde_json::from_slice::<Vec<String>>(asked).ok()?;
    Some((
        Launch {
            args,
            activation: None,
        },
        TAKEN,
    ))
}

impl Profile {
    fn bind(dir: &Path, lock: File) -> Self {
        let run = dir.join(RUN);
        let socket = run.join(SOCKET);
        let bound = crate::os::private_dir(&run)
            .and_then(|()| match std::fs::remove_file(&socket) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            })
            .and_then(|()| UnixListener::bind(&socket));
        let listener = bound
            .inspect_err(|error| {
                warn!(target: TARGET, event = "profile_unbound", socket = %socket.display(), %error, "later launches of this profile wait for this copy to quit")
            })
            .ok();
        Self { lock, listener }
    }

    pub(crate) fn serve(self, deliver: impl Fn(Launch) -> bool + Send + 'static) {
        let Self { lock, listener } = self;
        HELD.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(lock);
        let Some(listener) = listener else {
            return;
        };
        let served = std::thread::Builder::new()
            .name("profile".into())
            .spawn(move || serve_on(&listener, &deliver));
        if let Err(error) = served {
            warn!(target: TARGET, event = "profile_unserved", %error, "later launches of this profile wait for this copy to quit");
        }
    }
}

fn serve_on(listener: &UnixListener, deliver: &dyn Fn(Launch) -> bool) {
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                warn!(target: TARGET, event = "profile_unserved", %error, "later launches of this profile wait for this copy to quit");
                return;
            }
        };
        let mut asked = Vec::new();
        if stream.read_to_end(&mut asked).is_err() {
            continue;
        }
        let Some((launch, answer)) = read_launch(&asked) else {
            warn!(target: TARGET, event = "profile_unread", bytes = asked.len(), "a later launch said what this copy can't read: the console comes forward, and the launch is not taken");
            deliver(Launch::default());
            continue;
        };
        if deliver(launch) {
            let _ = stream.write_all(answer);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn held(claim: Claim) -> Profile {
        match claim {
            Claim::Held(profile) => profile,
            Claim::HandedOver => panic!("handed over"),
        }
    }

    #[test]
    fn a_second_launch_hands_its_arguments_to_the_copy_holding_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let first = held(claim_at(dir.path(), &Launch::default()).unwrap());
        let (arrived, arrivals) = std::sync::mpsc::channel();
        let listener = first.listener.as_ref().unwrap().try_clone().unwrap();
        std::thread::spawn(move || serve_on(&listener, &|launch| arrived.send(launch).is_ok()));
        let launch = Launch {
            args: vec!["duck://garage/chat/dm 1".into()],
            activation: Some("ducktape-1_TIME42".into()),
        };
        let second = claim_at(dir.path(), &launch).unwrap();
        assert!(matches!(second, Claim::HandedOver));
        assert_eq!(arrivals.recv().unwrap(), launch);
        let lock = File::open(dir.path().join(LOCK)).unwrap();
        assert!(
            matches!(lock.try_lock(), Err(TryLockError::WouldBlock)),
            "the first copy still holds the profile"
        );
        drop(first);
        lock.try_lock().unwrap();
    }

    fn served(first: &Profile) -> std::sync::mpsc::Receiver<Launch> {
        let (arrived, arrivals) = std::sync::mpsc::channel();
        let listener = first.listener.as_ref().unwrap().try_clone().unwrap();
        std::thread::spawn(move || serve_on(&listener, &|launch| arrived.send(launch).is_ok()));
        arrivals
    }

    fn said(socket: &Path, asked: &[u8]) -> Vec<u8> {
        let mut stream = UnixStream::connect(socket).unwrap();
        stream.write_all(asked).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut said = Vec::new();
        stream.read_to_end(&mut said).unwrap();
        said
    }

    #[test]
    fn an_earlier_releases_launch_hands_its_link_over() {
        let dir = tempfile::tempdir().unwrap();
        let first = held(claim_at(dir.path(), &Launch::default()).unwrap());
        let arrivals = served(&first);
        let socket = dir.path().join(RUN).join(SOCKET);
        assert_eq!(said(&socket, br#"["duck://garage/chat"]"#), TAKEN);
        let launch = arrivals.recv().unwrap();
        assert_eq!(launch.args, ["duck://garage/chat"]);
        assert_eq!(launch.activation, None);
    }

    #[test]
    fn a_launch_hands_its_link_to_an_earlier_release() {
        let dir = tempfile::tempdir().unwrap();
        let first = held(claim_at(dir.path(), &Launch::default()).unwrap());
        let (arrived, arrivals) = std::sync::mpsc::channel();
        let listener = first.listener.as_ref().unwrap().try_clone().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut asked = Vec::new();
                stream.read_to_end(&mut asked).unwrap();
                let args: Vec<String> = serde_json::from_slice(&asked).unwrap_or_default();
                arrived.send(args).unwrap();
                stream.write_all(TAKEN).unwrap();
            }
        });
        let launch = Launch {
            args: vec!["duck://garage/chat".into()],
            activation: Some("x_TIME1".into()),
        };
        assert!(matches!(
            claim_at(dir.path(), &launch).unwrap(),
            Claim::HandedOver
        ));
        assert_eq!(arrivals.recv().unwrap(), Vec::<String>::new());
        assert_eq!(arrivals.recv().unwrap(), launch.args);
    }

    #[test]
    fn a_launch_this_copy_cannot_read_raises_it_and_is_not_taken() {
        let dir = tempfile::tempdir().unwrap();
        let first = held(claim_at(dir.path(), &Launch::default()).unwrap());
        let arrivals = served(&first);
        let socket = dir.path().join(RUN).join(SOCKET);
        assert_ne!(said(&socket, b"{\"args\": 7}"), TAKEN_WHOLE);
        assert_eq!(arrivals.recv().unwrap(), Launch::default());
    }

    #[test]
    fn a_launch_nothing_takes_waits_for_the_holder_and_then_holds_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let first = held(claim_at(dir.path(), &Launch::default()).unwrap());
        let path = dir.path().to_owned();
        let waiting =
            std::thread::spawn(move || held(claim_at(&path, &Launch::default()).unwrap()));
        drop(first);
        let second = waiting.join().unwrap();
        assert!(second.listener.is_some(), "it serves the launches after it");
        let lock = File::open(dir.path().join(LOCK)).unwrap();
        assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));
    }

    #[test]
    fn a_copy_that_stops_taking_launches_lets_the_launch_wait_for_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let first = held(claim_at(dir.path(), &Launch::default()).unwrap());
        let listener = first.listener.as_ref().unwrap().try_clone().unwrap();
        std::thread::spawn(move || serve_on(&listener, &|_| false));
        let path = dir.path().to_owned();
        let waiting =
            std::thread::spawn(move || held(claim_at(&path, &Launch::default()).unwrap()));
        drop(first);
        waiting.join().unwrap();
    }

    #[test]
    fn a_socket_left_by_a_copy_that_is_gone_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let first = held(claim_at(dir.path(), &Launch::default()).unwrap());
        drop(first);
        assert!(dir.path().join(RUN).join(SOCKET).exists());
        let second = held(claim_at(dir.path(), &Launch::default()).unwrap());
        assert!(second.listener.is_some());
        let mode = std::fs::metadata(dir.path().join(RUN))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "no other user reaches the socket");
    }
}
