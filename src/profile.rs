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

static HELD: Mutex<Vec<File>> = Mutex::new(Vec::new());

pub(crate) enum Claim {
    Held(Profile),
    HandedOver,
}

pub(crate) struct Profile {
    lock: File,
    listener: Option<UnixListener>,
}

pub(crate) fn claim() -> Result<Claim, String> {
    let dir = crate::backend::config_dir()?;
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    claim_at(&dir, &args).map_err(|error| format!("{}: {error}", dir.display()))
}

pub(crate) fn claim_at(dir: &Path, args: &[String]) -> std::io::Result<Claim> {
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
    if hand_over(&dir.join(RUN).join(SOCKET), args) {
        info!(target: TARGET, event = "profile_handed_over", "the copy running this profile took this launch");
        return Ok(Claim::HandedOver);
    }
    info!(target: TARGET, event = "profile_waiting", "another copy holds this profile and took nothing: this launch waits for it to quit");
    lock.lock()?;
    Ok(Claim::Held(Profile::bind(dir, lock)))
}

fn hand_over(socket: &Path, args: &[String]) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let Ok(asked) = serde_json::to_vec(args) else {
        return false;
    };
    let mut said = Vec::new();
    stream.write_all(&asked).is_ok()
        && stream.shutdown(Shutdown::Write).is_ok()
        && stream.read_to_end(&mut said).is_ok()
        && said == TAKEN
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

    pub(crate) fn serve(self, deliver: impl Fn(Vec<String>) -> bool + Send + 'static) {
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

fn serve_on(listener: &UnixListener, deliver: &dyn Fn(Vec<String>) -> bool) {
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
        let args = serde_json::from_slice(&asked).unwrap_or_default();
        if deliver(args) {
            let _ = stream.write_all(TAKEN);
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
        let first = held(claim_at(dir.path(), &[]).unwrap());
        let (arrived, arrivals) = std::sync::mpsc::channel();
        let listener = first.listener.as_ref().unwrap().try_clone().unwrap();
        std::thread::spawn(move || serve_on(&listener, &|args| arrived.send(args).is_ok()));
        let link = "duck://garage/chat/dm 1".to_string();
        let second = claim_at(dir.path(), std::slice::from_ref(&link)).unwrap();
        assert!(matches!(second, Claim::HandedOver));
        assert_eq!(arrivals.recv().unwrap(), vec![link]);
        let lock = File::open(dir.path().join(LOCK)).unwrap();
        assert!(
            matches!(lock.try_lock(), Err(TryLockError::WouldBlock)),
            "the first copy still holds the profile"
        );
        drop(first);
        lock.try_lock().unwrap();
    }

    #[test]
    fn a_launch_nothing_takes_waits_for_the_holder_and_then_holds_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let first = held(claim_at(dir.path(), &[]).unwrap());
        let path = dir.path().to_owned();
        let waiting = std::thread::spawn(move || held(claim_at(&path, &[]).unwrap()));
        drop(first);
        let second = waiting.join().unwrap();
        assert!(second.listener.is_some(), "it serves the launches after it");
        let lock = File::open(dir.path().join(LOCK)).unwrap();
        assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));
    }

    #[test]
    fn a_copy_that_stops_taking_launches_lets_the_launch_wait_for_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let first = held(claim_at(dir.path(), &[]).unwrap());
        let listener = first.listener.as_ref().unwrap().try_clone().unwrap();
        std::thread::spawn(move || serve_on(&listener, &|_| false));
        let path = dir.path().to_owned();
        let waiting = std::thread::spawn(move || held(claim_at(&path, &[]).unwrap()));
        drop(first);
        waiting.join().unwrap();
    }

    #[test]
    fn a_socket_left_by_a_copy_that_is_gone_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let first = held(claim_at(dir.path(), &[]).unwrap());
        drop(first);
        assert!(dir.path().join(RUN).join(SOCKET).exists());
        let second = held(claim_at(dir.path(), &[]).unwrap());
        assert!(second.listener.is_some());
        let mode = std::fs::metadata(dir.path().join(RUN))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "no other user reaches the socket");
    }
}
