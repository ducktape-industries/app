use super::*;
use std::io::Write as _;
use std::process::{Child, Command, Stdio};

fn start_daemon() -> Child {
    let mut child = Command::new("gnome-keyring-daemon")
        .args(["--foreground", "--unlock", "--components=secrets"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"proof\n").unwrap();
    child
}

fn owner_changed(signals: &mut impl Iterator<Item = zbus::Message>, present: bool) {
    for message in signals {
        let (name, _, owner): (String, String, String) = message.body().deserialize().unwrap();
        let expected = name == "org.freedesktop.secrets" && owner.is_empty() != present;
        if expected {
            return;
        }
    }
    panic!("session bus ended before the Secret Service changed owner");
}

#[test]
#[ignore = "needs dbus-run-session and gnome-keyring-daemon"]
fn daemon_restart_reconnects_on_the_next_ask() {
    const CHILD: &str = "DUCKTAPE_KEYRING_RESTART_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        let status = Command::new("dbus-run-session")
            .arg("--")
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backend::device_key::tests::restart::daemon_restart_reconnects_on_the_next_ask",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HOME", home.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let connection = zbus::blocking::Connection::session().unwrap();
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .unwrap();
    let mut signals = proxy.receive_signal("NameOwnerChanged").unwrap();
    let mut daemon = start_daemon();
    owner_changed(&mut signals, true);
    os_store().unwrap();
    let account = "device-key:restart:duckpond";
    let entry = keyring::Entry::new(SERVICE, account).unwrap();
    entry.set_secret(b"kept across a daemon restart").unwrap();
    assert_eq!(
        kept(&entry).unwrap().unwrap().as_slice(),
        b"kept across a daemon restart"
    );
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    owner_changed(&mut signals, false);
    let mut daemon = start_daemon();
    owner_changed(&mut signals, true);
    assert!(kept(&entry).is_err());
    assert!(keyring::get_default_store().is_none());
    os_store().unwrap();
    let reconnected = keyring::Entry::new(SERVICE, account).unwrap();
    assert_eq!(
        kept(&reconnected).unwrap().unwrap().as_slice(),
        b"kept across a daemon restart"
    );
    reconnected.delete_credential().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}
