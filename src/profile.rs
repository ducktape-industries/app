//! One running copy per profile, the config directory every file of the person's lives under:
//! the copy holding `<config>/lock`. A later launch hands its arguments to it over a socket
//! named for the profile in a short private directory, or says another copy holds it.

use std::fs::{File, TryLockError};
use std::io::{Read as _, Write as _};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use tracing::{info, warn};

const TARGET: &str = "ducktape::app";
const LOCK: &str = "lock";
const TAKEN: &[u8] = b"taken";
const TAKEN_WHOLE: &[u8] = b"taken whole";

static HELD: Mutex<Vec<File>> = Mutex::new(Vec::new());

pub(crate) enum Claim {
    Held(Profile),
    HandedOver,
    Busy(Busy),
}

pub(crate) enum Start {
    Now(Option<Profile>, Launch),
    WhenFree(Busy, Launch),
}

pub(crate) struct Busy {
    reason: String,
    activation: Option<String>,
    lock: File,
    socket: PathBuf,
}

impl Busy {
    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }

    pub(crate) fn activation(&self) -> Option<&str> {
        self.activation.as_deref()
    }

    pub(crate) fn wait(self) -> std::io::Result<Profile> {
        self.lock.lock()?;
        info!(target: TARGET, event = "profile_freed", "the copy holding this profile quit: this launch goes on");
        Ok(Profile::bind(&self.socket, self.lock))
    }
}

pub(crate) struct Profile {
    lock: File,
    listener: Result<UnixListener, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Launch {
    pub(crate) args: Vec<String>,
    pub(crate) activation: Option<String>,
}

const ACTIVATION: [&str; 2] = ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"];

pub(crate) fn launch() -> Launch {
    Launch {
        args: std::env::args_os()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        activation: ACTIVATION
            .into_iter()
            .find_map(|name| std::env::var(name).ok().filter(|token| !token.is_empty())),
    }
}

pub(crate) fn claim(launch: &Launch) -> Result<Claim, String> {
    let config = crate::backend::config_dir()?;
    let sockets = sockets_dir()?;
    claim_at(&config, &sockets, launch).map_err(|error| format!("{}: {error}", config.display()))
}

fn sockets_dir() -> Result<PathBuf, String> {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(runtime));
    }
    // SAFETY: getuid(2) takes nothing and cannot fail.
    let uid = unsafe { libc::getuid() };
    let dir = std::env::temp_dir().join(format!("ducktape-{uid}"));
    own_private(&dir, uid).map_err(|error| format!("{}: {error}", dir.display()))?;
    Ok(dir)
}

fn own_private(dir: &Path, uid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    crate::os::private_dir(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    match meta.is_dir() && meta.uid() == uid && meta.permissions().mode() & 0o777 == 0o700 {
        true => Ok(()),
        false => Err(std::io::Error::other(
            "not a directory of this user's alone",
        )),
    }
}

fn kept_sticky(socket: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(socket)?.permissions().mode();
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(mode | 0o1000))
}

pub(crate) fn socket_of(config: &Path, sockets: &Path) -> PathBuf {
    let digest = ducktape_updater::Sha::digest(config.as_os_str().as_encoded_bytes()).to_string();
    sockets.join(format!("ducktape-{}.sock", &digest[..16]))
}

pub(crate) fn claim_at(config: &Path, sockets: &Path, launch: &Launch) -> std::io::Result<Claim> {
    std::fs::create_dir_all(config)?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(config.join(LOCK))?;
    let socket = socket_of(config, sockets);
    match lock.try_lock() {
        Ok(()) => return Ok(Claim::Held(Profile::bind(&socket, lock))),
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Error(error)) => return Err(error),
    }
    match hand_over(&socket, launch) {
        Ok(()) => {
            info!(target: TARGET, event = "profile_handed_over", "the copy running this profile took this launch");
            Ok(Claim::HandedOver)
        }
        Err(reason) => {
            warn!(target: TARGET, event = "profile_busy", socket = %socket.display(), %reason, "another copy holds this profile and took nothing: this launch waits for it to quit");
            Ok(Claim::Busy(Busy {
                reason,
                activation: launch.activation.clone(),
                lock,
                socket,
            }))
        }
    }
}

fn hand_over(socket: &Path, launch: &Launch) -> Result<(), String> {
    let said = match ask(socket, launch)? {
        said if said == TAKEN => ask(socket, &launch.args)?,
        said => said,
    };
    match said == TAKEN || said == TAKEN_WHOLE {
        true => Ok(()),
        false => Err("it did not take this launch".into()),
    }
}

fn ask(socket: &Path, asked: &impl serde::Serialize) -> Result<Vec<u8>, String> {
    let mut stream = UnixStream::connect(socket).map_err(|error| error.to_string())?;
    let asked = serde_json::to_vec(asked).map_err(|error| error.to_string())?;
    let mut said = Vec::new();
    stream
        .write_all(&asked)
        .and_then(|()| stream.shutdown(Shutdown::Write))
        .and_then(|()| stream.read_to_end(&mut said))
        .map_err(|error| error.to_string())?;
    Ok(said)
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
    fn bind(socket: &Path, lock: File) -> Self {
        let listener = match std::fs::remove_file(socket) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
        .and_then(|()| UnixListener::bind(socket))
        .map_err(|error| format!("{}: {error}", socket.display()));
        if listener.is_ok()
            && let Err(error) = kept_sticky(socket)
        {
            warn!(target: TARGET, event = "profile_socket_unsticky", socket = %socket.display(), %error, "a runtime-dir cleanup may remove this copy's socket");
        }
        if let Err(error) = &listener {
            warn!(target: TARGET, event = "profile_unbound", %error, "a later launch of this profile can't hand over to this copy");
        }
        Self { lock, listener }
    }

    pub(crate) fn unbound(&self) -> Option<&str> {
        self.listener.as_ref().err().map(String::as_str)
    }

    pub(crate) fn serve(self, deliver: impl Fn(Launch) -> bool + Send + 'static) {
        let Self { lock, listener } = self;
        HELD.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(lock);
        let Ok(listener) = listener else {
            return;
        };
        let served = std::thread::Builder::new()
            .name("profile".into())
            .spawn(move || serve_on(&listener, &deliver));
        if let Err(error) = served {
            warn!(target: TARGET, event = "profile_unserved", %error, "a later launch of this profile can't hand over to this copy");
        }
    }
}

fn serve_on(listener: &UnixListener, deliver: &dyn Fn(Launch) -> bool) {
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                warn!(target: TARGET, event = "profile_unserved", %error, "a later launch of this profile can't hand over to this copy");
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
    use super::*;

    fn held(claim: Claim) -> Profile {
        match claim {
            Claim::Held(profile) => profile,
            Claim::HandedOver => panic!("handed over"),
            Claim::Busy(busy) => panic!("busy: {}", busy.reason),
        }
    }

    fn serving(profile: &Profile, deliver: impl Fn(Launch) -> bool + Send + 'static) {
        let listener = profile.listener.as_ref().unwrap().try_clone().unwrap();
        std::thread::spawn(move || serve_on(&listener, &deliver));
    }

    #[test]
    fn a_second_launch_hands_its_arguments_to_the_copy_holding_the_profile() {
        let (config, sockets) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let first = held(claim_at(config.path(), sockets.path(), &Launch::default()).unwrap());
        let (arrived, arrivals) = std::sync::mpsc::channel();
        serving(&first, move |launch| arrived.send(launch).is_ok());
        let launch = Launch {
            args: vec!["duck://garage/chat/dm 1".into()],
            activation: Some("ducktape-1_TIME42".into()),
        };
        let second = claim_at(config.path(), sockets.path(), &launch).unwrap();
        assert!(matches!(second, Claim::HandedOver));
        assert_eq!(arrivals.recv().unwrap(), launch);
        let lock = File::open(config.path().join(LOCK)).unwrap();
        assert!(
            matches!(lock.try_lock(), Err(TryLockError::WouldBlock)),
            "the first copy still holds the profile"
        );
        drop(first);
        lock.try_lock().unwrap();
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
        let (config, sockets) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let first = held(claim_at(config.path(), sockets.path(), &Launch::default()).unwrap());
        let (arrived, arrivals) = std::sync::mpsc::channel();
        serving(&first, move |launch| arrived.send(launch).is_ok());
        let socket = socket_of(config.path(), sockets.path());
        assert_eq!(said(&socket, br#"["duck://garage/chat"]"#), TAKEN);
        let launch = arrivals.recv().unwrap();
        assert_eq!(launch.args, ["duck://garage/chat"]);
        assert_eq!(launch.activation, None);
    }

    #[test]
    fn a_launch_hands_its_link_to_an_earlier_release() {
        let (config, sockets) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let first = held(claim_at(config.path(), sockets.path(), &Launch::default()).unwrap());
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
        let claim = claim_at(config.path(), sockets.path(), &launch).unwrap();
        assert!(matches!(claim, Claim::HandedOver));
        assert_eq!(arrivals.recv().unwrap(), Vec::<String>::new());
        assert_eq!(arrivals.recv().unwrap(), launch.args);
    }

    #[test]
    fn a_launch_this_copy_cannot_read_raises_it_and_is_not_taken() {
        let (config, sockets) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let first = held(claim_at(config.path(), sockets.path(), &Launch::default()).unwrap());
        let (arrived, arrivals) = std::sync::mpsc::channel();
        serving(&first, move |launch| arrived.send(launch).is_ok());
        let socket = socket_of(config.path(), sockets.path());
        assert_ne!(said(&socket, b"{\"args\": 7}"), TAKEN_WHOLE);
        assert_eq!(arrivals.recv().unwrap(), Launch::default());
    }

    #[test]
    fn a_profile_deeper_than_a_socket_path_allows_still_hands_over() {
        let (root, sockets) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut config = root.path().to_owned();
        while config.as_os_str().len() < 131 {
            config.push("a-long-profile-directory");
        }
        let first = held(claim_at(&config, sockets.path(), &Launch::default()).unwrap());
        assert_eq!(first.unbound(), None);
        serving(&first, |_| true);
        let second = claim_at(&config, sockets.path(), &Launch::default()).unwrap();
        assert!(matches!(second, Claim::HandedOver));
    }

    #[test]
    fn a_launch_nothing_takes_is_busy_at_once_and_holds_the_profile_once_it_frees() {
        let (config, sockets) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let first = held(claim_at(config.path(), sockets.path(), &Launch::default()).unwrap());
        serving(&first, |_| false);
        let refused = claim_at(config.path(), sockets.path(), &Launch::default()).unwrap();
        assert!(matches!(refused, Claim::Busy(_)), "it took nothing");
        drop(first.listener);
        let Claim::Busy(busy) =
            claim_at(config.path(), sockets.path(), &Launch::default()).unwrap()
        else {
            panic!("nothing listens, and it did not say busy");
        };
        let waiting = std::thread::spawn(move || busy.wait().unwrap());
        drop(first.lock);
        let profile = waiting.join().unwrap();
        assert_eq!(profile.unbound(), None, "it serves the launches after it");
        let lock = File::open(config.path().join(LOCK)).unwrap();
        assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));
    }

    #[test]
    fn two_profiles_name_two_sockets_and_a_socket_left_behind_is_replaced() {
        let sockets = tempfile::tempdir().unwrap();
        let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        assert_ne!(
            socket_of(one.path(), sockets.path()),
            socket_of(two.path(), sockets.path())
        );
        let first = held(claim_at(one.path(), sockets.path(), &Launch::default()).unwrap());
        drop(first);
        assert!(socket_of(one.path(), sockets.path()).exists());
        let again = held(claim_at(one.path(), sockets.path(), &Launch::default()).unwrap());
        assert_eq!(again.unbound(), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_socket_is_sticky_so_a_runtime_dir_cleanup_keeps_it() {
        use std::os::unix::fs::PermissionsExt as _;
        let (config, sockets) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let first = held(claim_at(config.path(), sockets.path(), &Launch::default()).unwrap());
        assert_eq!(first.unbound(), None);
        let socket = socket_of(config.path(), sockets.path());
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode();
        assert_eq!(mode & 0o1000, 0o1000, "mode {mode:o}");
    }

    #[test]
    fn a_shared_temp_directory_is_used_only_when_it_is_this_users_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        // SAFETY: getuid(2) takes nothing and cannot fail.
        let uid = unsafe { libc::getuid() };
        let dir = root.path().join("ducktape");
        own_private(&dir, uid).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert!(own_private(&dir, uid + 1).is_err(), "another user's");
        let file = root.path().join("file");
        std::fs::write(&file, b"").unwrap();
        assert!(own_private(&file, uid).is_err());
    }
}
