//! This device's key and the account it holds, and every way of signing
//! in: the key opened or made on reaching a network (an old
//! password-locked one moved into the OS), locked, and reading without
//! it; a new recovery key's words and their quiz; the account step (a name
//! for a new account, a passkey that makes or joins one, "From another
//! device", a recovery key typed); approving another device; and, once
//! per device, the layout step standing in for the desk until Continue.
//! `Account` holds what the screens draw of it (`AccountState`, compared),
//! owns the tasks each flow runs, and writes `Screen` as a step is passed.
//!
//! Secrets never enter the compared value: a typed password, phrase or
//! word stays in the field that shows it and comes in the submit call. The
//! new recovery key's words are held here uncompared while their screens
//! show, and go with them.
//!
//! What other entities do for it follows through events: a notice
//! (`AccountEvent::Toast`, `Toast`), Help opened greeted for a new
//! account (`Welcome`, `Windows`), "Add a device…" closing once approved
//! (`Approved`, `Windows`).

use super::{AccountStep, Prefs, Screen, Slice, on_runtime, spawn_on_runtime};
use crate::backend;
use futures::StreamExt as _;
use gpui_kit::{Context, Entity, EventEmitter, Task};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A typed secret or a recovery phrase: wiped when it drops, so leaving a
/// step wipes what it held.
pub(crate) type Secret = zeroize::Zeroizing<String>;

/// The account's compared value.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AccountState {
    /// The seated key's public half, hex; empty while locked.
    pub(crate) signer_key: String,
    /// `None` not asked yet; `Some(None)` no account; `Some(Some(..))` found.
    pub(crate) account: Option<Option<(u64, String)>>,
    /// A password-locked key file is here for the network and this
    /// device's OS-kept key is not: the key screen asks for its password,
    /// once, and moves it into the OS.
    pub(crate) key_exists: bool,
    /// Locked on purpose: the key is not reopened until Unlock.
    pub(crate) locked: bool,
    /// This device's key is being opened (or made) for the network reached.
    pub(crate) seating: bool,
    /// A sign-in call is out (any step's); a second submit is ignored.
    pub(crate) busy: bool,
    /// The failure the current sign-in step shows, whichever step: the key,
    /// a phrase, the account, a passkey, "Add a device…". Cleared by the
    /// next keystroke or try.
    pub(crate) error: String,
    /// "Add a device…": the joining key's fingerprint, once found.
    pub(crate) approve: Option<String>,
    /// The code this device waits under for another to approve it.
    pub(crate) link_code: String,
    /// A passkey ceremony is in the browser.
    pub(crate) passkey_waiting: bool,
    /// The passkey QR URL, once the person picked the phone.
    pub(crate) passkey_qr: Option<String>,
    /// Help greets a new account rather than titling itself.
    pub(crate) welcome: bool,
}

/// What other entities do for the account: `Toast` shows the notice,
/// `Windows` opens Help and closes "Add a device…".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AccountEvent {
    Toast(String),
    /// A new account: Help opens on the desk, greeting it.
    Welcome,
    /// Another device approved: its dialog closes.
    Approved,
}

pub(crate) struct Account {
    state: AccountState,
    screen: Entity<Slice<Screen>>,
    /// Read for the layout the desk waits on (`show`), written by its step.
    prefs: Entity<Slice<Prefs>>,
    /// A new recovery key's 24 words, while its screens show.
    phrase: Option<Secret>,
    /// The node and network the account is on (`Session`'s connection,
    /// given as the network is taken up): what the sign-in calls are asked
    /// of. Off a network, a client of no node.
    client: backend::RpcClient,
    network: String,
    /// Where this network's keys live on this device
    /// ([`backend::bind_keyring`]): the name alone is not enough, two chains
    /// can share one.
    keyring: String,
    /// "Add a device…": the code typed, and the request it found.
    approve_code: String,
    approve_found: Option<backend::join::Request>,
    /// Set once the person picks "Use a phone instead"; the ceremony reads it.
    passkey_phone: Arc<AtomicBool>,
    /// The QR URL of the touch in flight.
    passkey_url: String,
    /// The key being opened or made (`open_device_key`).
    seating: Option<Task<()>>,
    /// The sign-in call out: an unlock, a phrase check, a recovery, an
    /// account made, an approval (`busy`).
    call: Option<Task<()>>,
    /// The node asked which account the key holds.
    /// "From another device", waiting to be approved.
    link: Option<Task<()>>,
    /// A passkey ceremony in the browser; dropping it cancels it.
    passkey: Option<Task<()>>,
}

impl EventEmitter<AccountEvent> for Account {}

fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.sign_in")
}

impl Account {
    pub(crate) fn new(screen: Entity<Slice<Screen>>, prefs: Entity<Slice<Prefs>>) -> Self {
        Self {
            state: AccountState::default(),
            screen,
            prefs,
            phrase: None,
            client: backend::RpcClient::new(""),
            network: String::new(),
            keyring: String::new(),
            approve_code: String::new(),
            approve_found: None,
            passkey_phone: Arc::default(),
            passkey_url: String::new(),
            seating: None,
            call: None,
            link: None,
            passkey: None,
        }
    }

    pub(crate) fn get(&self) -> &AccountState {
        &self.state
    }

    /// A new recovery key's words, while its screens show.
    pub(crate) fn phrase(&self) -> Option<&Secret> {
        self.phrase.as_ref()
    }

    /// A field-wise edit, compared as a whole; notifies when it moved.
    fn edit(&mut self, edit: impl FnOnce(&mut AccountState), cx: &mut Context<Self>) -> bool {
        super::slice::edit_compared(&mut self.state, edit, cx)
    }

    fn screen(&self, cx: &Context<Self>) -> Screen {
        *self.screen.read(cx).get()
    }

    /// `screen` on the console. The desk waits while this device has no
    /// layout: every way in shows the layout step instead, once
    /// (`layout_chosen`).
    fn show(&self, screen: Screen, cx: &mut Context<Self>) {
        let screen = match screen {
            Screen::Desk if self.prefs.read(cx).get().layout.is_none() => Screen::Layout,
            screen => screen,
        };
        self.screen.update(cx, |current, cx| {
            current.set(screen, cx);
        });
    }

    fn client(&self) -> backend::RpcClient {
        self.client.clone()
    }

    /// The node the account is on; empty off a network.
    fn node(&self) -> &str {
        self.client.endpoint()
    }

    /// The step's failure goes with the next keystroke in any of its fields.
    pub(crate) fn clear_error(&mut self, cx: &mut Context<Self>) {
        self.edit(|state| state.error.clear(), cx);
    }

    fn fail(&mut self, error: impl Into<String>, cx: &mut Context<Self>) {
        let error = error.into();
        self.edit(|state| state.error = error, cx);
    }

    /// The step left (a back, a close) with its call still out: the call is
    /// dropped, never to land, and the next try is not refused as busy. A
    /// node that stalls holds the step only until the person leaves it.
    fn drop_call(&mut self, cx: &mut Context<Self>) {
        if self.call.take().is_some() {
            self.edit(|state| state.busy = false, cx);
        }
    }

    // ---------- the network ----------

    /// The network `keyring` names becomes the one in hand, on `client`'s
    /// node.
    /// Another chain than the last (a switch, not a second node of the same
    /// network): nothing of the last one carries over (its seated key, its
    /// account, any sign-in half done), and the key step comes first; `true`
    /// then, for what leaving the last network clears. A second node of the
    /// same network keeps the screen it was on.
    pub(crate) fn take_up(
        &mut self,
        keyring: backend::Keyring,
        client: backend::RpcClient,
        network: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let _timed = timed();
        let left = keyring.dir != self.keyring;
        if left {
            self.leave_network(cx);
        }
        if self.screen(cx) == Screen::Connect {
            self.show(Screen::Unlock { awaiting: false }, cx);
        }
        self.keyring = keyring.dir;
        self.client = client;
        self.network = network;
        let key_exists = backend::key_exists(&self.keyring);
        self.edit(|state| state.key_exists = key_exists, cx);
        left
    }

    /// Everything that belonged to the network being left: its seated key
    /// (the seat is one for the whole app), the account it resolved to, and
    /// any sign-in half done. Dropping the tasks cancels them; an account
    /// lookup in flight lands and is discarded by `resolved`'s node and
    /// key check.
    pub(crate) fn leave_network(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.seating = None;
        self.call = None;
        self.link = None;
        self.passkey = None;
        self.phrase = None;
        self.client = backend::RpcClient::new("");
        self.network.clear();
        self.keyring.clear();
        self.approve_code.clear();
        self.approve_found = None;
        self.passkey_url.clear();
        let welcome = self.state.welcome;
        self.edit(
            |state| {
                *state = AccountState {
                    welcome,
                    ..AccountState::default()
                }
            },
            cx,
        );
        self.show(Screen::Unlock { awaiting: false }, cx);
        spawn_on_runtime(cx, backend::lock_signer(), |_, _, _| {}).detach();
    }

    // ---------- this device's key ----------

    /// Opens this device's key for the network in hand and seats it, made
    /// here the first time this device meets the network. Nothing to do when
    /// one is seated, or the person locked it; a password-locked key from
    /// before answers `Ok(None)` and waits for its password once.
    pub(crate) fn open_device_key(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.state.seating
            || self.state.locked
            || !self.state.signer_key.is_empty()
            || self.keyring.is_empty()
        {
            return;
        }
        self.edit(|state| state.seating = true, cx);
        let keyring = self.keyring.clone();
        let legacy = self.state.key_exists;
        let work = async move {
            let opened =
                tokio::task::spawn_blocking(move || backend::device_key::open(&keyring, legacy))
                    .await
                    .unwrap_or_else(|_| Err("opening this device's key did not finish".into()));
            match opened {
                Ok(Some(key)) => Ok(Some(backend::seat_key(key).await)),
                Ok(None) => Ok(None),
                Err(error) => Err(error),
            }
        };
        self.seating = Some(spawn_on_runtime(cx, work, |this, found, cx| {
            this.device_key_answered(found, cx)
        }));
    }

    /// This device's key, opened or made: its public half; `None` when only
    /// a password-locked key is here.
    pub(crate) fn device_key_answered(
        &mut self,
        found: Result<Option<String>, String>,
        cx: &mut Context<Self>,
    ) {
        let _timed = timed();
        self.seating = None;
        self.edit(|state| state.seating = false, cx);
        match found {
            Ok(Some(pubkey)) => {
                self.edit(|state| state.key_exists = false, cx);
                self.unlocked(pubkey, cx);
            }
            // only a password-locked key here: the key screen asks
            Ok(None) => {}
            Err(error) => self.fail(error, cx),
        }
    }

    /// Unlock. With no password typed, it reopens this device's OS-kept
    /// key (after a Lock). With one, it opens a password-locked key from
    /// before keys moved into the OS, and moves it there: the password is
    /// asked this once. A failed try leaves the field as it was.
    pub(crate) fn unlock(&mut self, password: String, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.state.busy {
            return;
        }
        self.edit(
            |state| {
                state.locked = false;
                state.error.clear();
            },
            cx,
        );
        if !self.state.key_exists {
            return self.open_device_key(cx);
        }
        if password.is_empty() {
            return self.fail("Type this key's password first.", cx);
        }
        let password = Secret::new(password);
        let keyring = self.keyring.clone();
        self.edit(|state| state.busy = true, cx);
        let work = async move {
            let opened = tokio::task::spawn_blocking(move || {
                let path = backend::session_key_path(&keyring)?;
                let key = keystore::userkey::open_user_key_at(&path, &password)?;
                // kept by the OS from now on; a refusal only means the
                // password is asked again next time
                if let Err(error) = backend::device_key::save(&keyring, &key) {
                    tracing::info!(target: "ducktape::keys", %error, "password-locked key not moved into the OS");
                }
                Ok::<_, String>(key)
            })
            .await
            .unwrap_or_else(|_| Err("opening this device's key did not finish".into()));
            match opened {
                Ok(key) => Ok(backend::seat_key(key).await),
                Err(error) => Err(backend::user_error(error)),
            }
        };
        self.call = Some(spawn_on_runtime(
            cx,
            work,
            |this, opened, cx| match opened {
                Ok(pubkey) => this.unlocked(pubkey, cx),
                Err(error) => this.unlock_failed(error, cx),
            },
        ));
    }

    /// The key seated. The key step stays up, its password gone, until the
    /// node says whether the key holds an account (`resolved`). A key that
    /// lands after the network was left seats nothing on screen.
    pub(crate) fn unlocked(&mut self, pubkey: String, cx: &mut Context<Self>) {
        let _timed = timed();
        self.call = None;
        self.edit(
            |state| {
                state.busy = false;
                state.key_exists = false;
                state.seating = false;
                state.locked = false;
                // another key's account is not this one's: the views hear
                // none until `resolved` names this key's
                if state.signer_key != pubkey {
                    state.account = None;
                }
                state.signer_key = pubkey;
            },
            cx,
        );
        if self.screen(cx) != Screen::Connect {
            self.show(Screen::Unlock { awaiting: true }, cx);
        }
        self.resolve(cx);
    }

    pub(crate) fn unlock_failed(&mut self, error: String, cx: &mut Context<Self>) {
        let _timed = timed();
        self.call = None;
        let key_exists = backend::key_exists(&self.keyring);
        self.edit(
            |state| {
                state.key_exists = key_exists;
                state.busy = false;
                state.error = error;
            },
            cx,
        );
    }

    /// Lock: the key is not reopened until Unlock. (The account menu closed
    /// as its row was picked: `layers::Chrome`.)
    pub(crate) fn lock(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(
            |state| {
                state.locked = true;
                state.signer_key.clear();
                state.account = None;
            },
            cx,
        );
        self.show(Screen::Unlock { awaiting: false }, cx);
        cx.emit(AccountEvent::Toast("Locked".into()));
        spawn_on_runtime(cx, backend::lock_signer(), |_, _, _| {}).detach();
    }

    /// Reading opens the desk, unless the key is seated already and the
    /// node's answer is on its way: that answer decides.
    pub(crate) fn browse_without_key(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.screen(cx) != (Screen::Unlock { awaiting: true }) {
            self.show(Screen::Desk, cx);
        }
        self.clear_error(cx);
    }

    /// "Sign in" from the bar: the key step, the key reopened.
    pub(crate) fn sign_in(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(|state| state.locked = false, cx);
        self.show(Screen::Unlock { awaiting: false }, cx);
        self.open_device_key(cx);
    }

    /// Asks the node which account the seated key belongs to; the answer
    /// lands in [`Self::resolved`]. A failed ask keeps what the menu bar
    /// already shows: the next block asks again.
    /// `--live`: the key the process seated, on `network` at `client`;
    /// its account is resolved as after any sign-in.
    #[cfg(debug_assertions)]
    pub(crate) fn live(
        &mut self,
        signer: String,
        client: backend::RpcClient,
        network: String,
        cx: &mut Context<Self>,
    ) {
        self.client = client;
        self.network = network;
        self.edit(|state| state.signer_key = signer, cx);
        self.resolve(cx);
    }

    pub(crate) fn resolve(&mut self, cx: &mut Context<Self>) {
        let Ok(key) = backend::hex_decode(&self.state.signer_key) else {
            return;
        };
        if key.is_empty() || self.node().is_empty() {
            return;
        }
        let client = self.client();
        let (node, network, signer) = (
            self.node().to_owned(),
            self.network.clone(),
            self.state.signer_key.clone(),
        );
        let work = async move {
            let lookup = backend::identity::account_of_key(&client, &network, key);
            match tokio::time::timeout(backend::noded::ANSWER_DEADLINE, lookup).await {
                Ok(Ok(account)) => Some(account),
                Ok(Err(error)) => {
                    tracing::debug!(target: "ducktape::app", %error, "account not resolved");
                    None
                }
                Err(_) => {
                    tracing::debug!(target: "ducktape::app", "account lookup unanswered");
                    None
                }
            }
        };
        // never cancelled: a block landing mid-lookup would otherwise drop
        // it, and `resolved` discards an answer for another node or key. A
        // node that never answers one is given up on at its deadline, so
        // lookups do not pile up one per block
        spawn_on_runtime(cx, work, move |this, account, cx| {
            if let Some(account) = account {
                this.resolved(&node, &signer, account, cx);
            }
        })
        .detach();
    }

    /// The node's word on `key`'s account, asked of `node`: an answer from
    /// before a switch or for another key is not this one's. After a
    /// sign-in the key screen stays up (`Screen::Unlock { awaiting }`) until
    /// this lands: a key with no account goes on to the account step, one
    /// with an account to the console, and neither shows the other first.
    /// Later answers move no screen.
    pub(crate) fn resolved(
        &mut self,
        node: &str,
        key: &str,
        account: Option<(u64, String)>,
        cx: &mut Context<Self>,
    ) {
        let _timed = timed();
        if node != self.node() || key != self.state.signer_key {
            return;
        }
        if self.screen(cx) == (Screen::Unlock { awaiting: true }) {
            self.show(
                match account {
                    None => Screen::Account {
                        step: AccountStep::Name,
                    },
                    Some(_) => Screen::Desk,
                },
                cx,
            );
        }
        self.edit(|state| state.account = Some(account), cx);
    }

    // ---------- a new recovery key, from the account menu ----------

    /// "Make a recovery key…": its words, to write down.
    pub(crate) fn recovery_key_start(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.clear_error(cx);
        self.phrase = Some(backend::join::new_recovery_phrase().into());
        cx.notify();
        self.show(Screen::Phrase { quiz: None }, cx);
    }

    pub(crate) fn phrase_cancel(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if matches!(self.screen(cx), Screen::Phrase { .. }) {
            self.drop_call(cx);
        }
        self.leave_phrase(cx);
    }

    /// "I wrote them down": three of the words asked back.
    pub(crate) fn phrase_written_down(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if let Some(phrase) = &self.phrase {
            let quiz = Some(quiz_positions(phrase.split_whitespace().count()));
            self.show(Screen::Phrase { quiz }, cx);
        }
        self.clear_error(cx);
    }

    /// The words typed back: checked, the key they make goes onto the
    /// account.
    pub(crate) fn phrase_check(&mut self, answers: [String; 3], cx: &mut Context<Self>) {
        let _timed = timed();
        let (Screen::Phrase { quiz: Some(asked) }, Some(phrase)) = (self.screen(cx), &self.phrase)
        else {
            return;
        };
        if self.state.busy {
            return;
        }
        if !quiz_matches(phrase, asked, &answers) {
            return self.fail(
                "Those words don't match your phrase. Check them, or show the phrase again.",
                cx,
            );
        }
        let (phrase, client, network) = (phrase.clone(), self.client(), self.network.clone());
        self.edit(|state| state.busy = true, cx);
        let work = async move { backend::join::add_recovery_key(&client, &network, &phrase).await };
        self.call = Some(spawn_on_runtime(cx, work, |this, added, cx| {
            this.recovery_key_added(added, cx)
        }));
    }

    pub(crate) fn recovery_key_added(&mut self, added: Result<(), String>, cx: &mut Context<Self>) {
        let _timed = timed();
        self.call = None;
        self.edit(|state| state.busy = false, cx);
        match added {
            Ok(()) => {
                self.leave_phrase(cx);
                cx.emit(AccountEvent::Toast(
                    "Recovery key added. Keep the paper somewhere safe.".into(),
                ));
            }
            Err(error) => self.fail(error, cx),
        }
    }

    /// "See the words again": the sheet, and the quiz starts over.
    pub(crate) fn phrase_show_again(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if matches!(self.screen(cx), Screen::Phrase { .. }) {
            self.show(Screen::Phrase { quiz: None }, cx);
        }
        self.clear_error(cx);
    }

    /// The recovery phrase and its check, gone once passed (or abandoned):
    /// back to the desk they were opened from.
    fn leave_phrase(&mut self, cx: &mut Context<Self>) {
        if matches!(self.screen(cx), Screen::Phrase { .. }) {
            self.show(Screen::Desk, cx);
        }
        if self.phrase.take().is_some() {
            cx.notify();
        }
        self.clear_error(cx);
    }

    // ---------- a recovery key typed, off the account step ----------

    pub(crate) fn recover_show(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if matches!(self.screen(cx), Screen::Account { .. }) {
            self.show(Screen::Recover, cx);
        }
        self.clear_error(cx);
    }

    pub(crate) fn recover_cancel(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.screen(cx) == Screen::Recover {
            self.drop_call(cx);
            self.show(
                Screen::Account {
                    step: AccountStep::Name,
                },
                cx,
            );
        }
        self.clear_error(cx);
    }

    /// The 24 words typed: this device joins the account they hold.
    pub(crate) fn recover_submit(&mut self, phrase: String, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.screen(cx) != Screen::Recover || self.state.busy {
            return;
        }
        let phrase = zeroize::Zeroizing::new(normalize_phrase(&phrase));
        if keystore::userkey::seed_of_mnemonic(&phrase).is_err() {
            return self.fail("Those words aren't a recovery key — check each one.", cx);
        }
        let (client, network) = (self.client(), self.network.clone());
        self.edit(
            |state| {
                state.busy = true;
                state.error.clear();
            },
            cx,
        );
        let work =
            async move { backend::join::join_with_recovery_key(&client, &network, &phrase).await };
        self.call = Some(spawn_on_runtime(cx, work, |this, joined, cx| {
            this.joined(joined, cx)
        }));
    }

    // ---------- the account step ----------

    /// "From another device": a code shown, and a wait for one on the
    /// account to approve it.
    pub(crate) fn link_start(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if !matches!(self.screen(cx), Screen::Account { .. }) || self.link.is_some() {
            return;
        }
        let code = backend::join::new_code();
        self.edit(
            |state| {
                state.link_code = code.clone();
                state.error.clear();
            },
            cx,
        );
        self.show(
            Screen::Account {
                step: AccountStep::Link,
            },
            cx,
        );
        let (client, network) = (self.client(), self.network.clone());
        let work = async move { backend::join::join_from_device(&client, &network, &code).await };
        self.link = Some(spawn_on_runtime(cx, work, |this, joined, cx| {
            this.joined(joined, cx)
        }));
    }

    pub(crate) fn link_cancel(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.end_link(cx);
    }

    fn end_link(&mut self, cx: &mut Context<Self>) {
        self.link = None;
        self.edit(|state| state.link_code.clear(), cx);
        if let Screen::Account {
            step: AccountStep::Link,
        } = self.screen(cx)
        {
            self.show(
                Screen::Account {
                    step: AccountStep::Name,
                },
                cx,
            );
        }
    }

    /// Another device or the recovery key added this one: the desk.
    pub(crate) fn joined(&mut self, joined: Result<(), String>, cx: &mut Context<Self>) {
        let _timed = timed();
        self.call = None;
        self.edit(|state| state.busy = false, cx);
        self.end_link(cx);
        match joined {
            Ok(()) => {
                if matches!(self.screen(cx), Screen::Account { .. } | Screen::Recover) {
                    self.show(Screen::Desk, cx);
                }
                self.resolve(cx);
            }
            Err(error) => self.fail(error, cx),
        }
    }

    /// A passkey creates an account named `name`.
    pub(crate) fn passkey_create(&mut self, name: String, cx: &mut Context<Self>) {
        let _timed = timed();
        self.passkey(Some(name), cx);
    }

    /// A passkey admits this device's key into the account it holds.
    pub(crate) fn passkey_sign_in(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.passkey(None, cx);
    }

    /// Both are about the ACCOUNT: the device key is already seated before
    /// either is offered (opened, or minted on first contact; it has no
    /// phrase), and it signs the writes.
    fn passkey(&mut self, create: Option<String>, cx: &mut Context<Self>) {
        if !matches!(self.screen(cx), Screen::Account { .. }) || self.state.busy {
            return;
        }
        if self.state.signer_key.is_empty() {
            return self.fail("Unlock this device's key first.", cx);
        }
        let name = create.as_deref().map(str::trim).map(str::to_owned);
        if name.as_deref() == Some("") {
            return self.fail("Name your account first.", cx);
        }
        let (client, network, pubkey) = (
            self.client(),
            self.network.clone(),
            self.state.signer_key.clone(),
        );
        self.passkey_phone = Arc::default();
        self.passkey_url.clear();
        self.edit(
            |state| {
                state.busy = true;
                state.error.clear();
                state.passkey_waiting = true;
                state.passkey_qr = None;
            },
            cx,
        );
        self.show(
            Screen::Account {
                step: AccountStep::Passkey,
            },
            cx,
        );
        let (phone, urls) = backend::auth_page::Phone::new(self.passkey_phone.clone());
        let flow = async move {
            let joined = match &name {
                Some(name) => {
                    backend::passkey::create_account(&client, &network, name, &phone).await
                }
                None => backend::passkey::sign_in(&client, &network, &phone).await,
            };
            match joined {
                Ok(()) => Ok(pubkey),
                Err(error) => Err(error),
            }
        };
        // the flow owns the only URL sender: the QR updates end with it
        let events = futures::stream::select(urls.map(Ok), futures::stream::once(flow).map(Err));
        self.passkey = Some(cx.spawn(async move |this, cx| {
            let mut events = std::pin::pin!(events);
            while let Some(event) = on_runtime(events.next()).await {
                let landed = this.update(cx, |this, cx| match event {
                    Ok(url) => this.passkey_qr(url, cx),
                    Err(Ok(pubkey)) => this.passkey_done(pubkey, cx),
                    Err(Err(error)) => this.passkey_failed(error, cx),
                });
                if landed.is_err() {
                    break;
                }
            }
        }));
    }

    /// "Use a phone instead": the QR shows once its URL is known.
    pub(crate) fn passkey_use_phone(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.passkey_phone.store(true, Ordering::Relaxed);
        self.show_passkey_qr(cx);
    }

    pub(crate) fn passkey_qr(&mut self, url: String, cx: &mut Context<Self>) {
        let _timed = timed();
        self.passkey_url = url;
        self.show_passkey_qr(cx);
    }

    /// The QR URL, while a ceremony runs and the person picked the phone.
    fn show_passkey_qr(&mut self, cx: &mut Context<Self>) {
        let shown = (self.passkey.is_some()
            && self.passkey_phone.load(Ordering::Relaxed)
            && !self.passkey_url.is_empty())
        .then(|| self.passkey_url.clone());
        self.edit(|state| state.passkey_qr = shown, cx);
    }

    pub(crate) fn passkey_cancel(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.end_passkey(cx);
        cx.emit(AccountEvent::Toast("Passkey step cancelled".into()));
    }

    pub(crate) fn passkey_failed(&mut self, error: String, cx: &mut Context<Self>) {
        let _timed = timed();
        self.end_passkey(cx);
        self.fail(error, cx);
    }

    /// The passkey made (or joined) the account: the desk.
    pub(crate) fn passkey_done(&mut self, pubkey: String, cx: &mut Context<Self>) {
        let _timed = timed();
        self.end_passkey(cx);
        if pubkey != self.state.signer_key {
            return;
        }
        if matches!(self.screen(cx), Screen::Account { .. }) {
            self.show(Screen::Desk, cx);
            self.welcome(cx);
        }
        self.resolve(cx);
    }

    /// The passkey ceremony over, however it ended.
    fn end_passkey(&mut self, cx: &mut Context<Self>) {
        self.passkey = None;
        self.edit(
            |state| {
                state.busy = false;
                state.passkey_waiting = false;
                state.passkey_qr = None;
            },
            cx,
        );
        if let Screen::Account {
            step: AccountStep::Passkey,
        } = self.screen(cx)
        {
            self.show(
                Screen::Account {
                    step: AccountStep::Name,
                },
                cx,
            );
        }
    }

    /// "Create account" from the desk: the account step, for a seated key.
    pub(crate) fn create_account(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if !self.state.signer_key.is_empty() {
            self.show(
                Screen::Account {
                    step: AccountStep::Name,
                },
                cx,
            );
        }
        self.clear_error(cx);
    }

    /// "Not now": the desk, without an account.
    pub(crate) fn create_later(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if matches!(self.screen(cx), Screen::Account { .. }) {
            self.drop_call(cx);
            self.show(Screen::Desk, cx);
        }
        self.clear_error(cx);
    }

    /// A plain account named `name`, signed by the seated key.
    pub(crate) fn create_submit(&mut self, name: String, cx: &mut Context<Self>) {
        let _timed = timed();
        if !matches!(self.screen(cx), Screen::Account { .. }) || self.state.busy {
            return;
        }
        let name = name.trim().to_owned();
        if name.is_empty() {
            return self.fail("Type the name others will see first.", cx);
        }
        let (client, network) = (self.client(), self.network.clone());
        self.edit(
            |state| {
                state.busy = true;
                state.error.clear();
            },
            cx,
        );
        let work =
            async move { backend::identity::create_plain_account(&client, &network, &name).await };
        self.call = Some(spawn_on_runtime(cx, work, |this, created, cx| {
            this.account_created(created, cx)
        }));
    }

    pub(crate) fn account_created(
        &mut self,
        created: Result<(u64, String), String>,
        cx: &mut Context<Self>,
    ) {
        let _timed = timed();
        self.call = None;
        self.edit(|state| state.busy = false, cx);
        match created {
            Ok(account) => {
                self.edit(|state| state.account = Some(Some(account)), cx);
                if matches!(self.screen(cx), Screen::Account { .. }) {
                    self.show(Screen::Desk, cx);
                    self.welcome(cx);
                }
            }
            Err(error) => {
                let error = account_error(&self.network, error);
                self.fail(error, cx);
            }
        }
    }

    /// A new account starts on Help, greeted.
    fn welcome(&mut self, cx: &mut Context<Self>) {
        self.edit(|state| state.welcome = true, cx);
        cx.emit(AccountEvent::Welcome);
    }

    /// Help asked for is just help.
    pub(crate) fn help_asked(&mut self, cx: &mut Context<Self>) {
        self.edit(|state| state.welcome = false, cx);
    }

    // ---------- the layout step, before the desk ----------

    /// The layout step's Continue: `layout` saved for this device, and the
    /// desk. A press that lands after the step went (a lock) does nothing.
    pub(crate) fn layout_chosen(&mut self, layout: backend::Layout, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.screen(cx) != Screen::Layout {
            return;
        }
        self.prefs
            .update(cx, |prefs, cx| prefs.set_layout(layout, cx));
        self.show(Screen::Desk, cx);
    }

    // ---------- "Add a device…", on the device already on the account ----------

    /// The dialog opened (`Overlays::open`, by the account menu's row):
    /// what the last one held goes.
    pub(crate) fn approve_open(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.approve_code.clear();
        self.approve_found = None;
        self.edit(
            |state| {
                state.approve = None;
                state.error.clear();
            },
            cx,
        );
    }

    /// The dialog closed, however: what it found, its failure and its call
    /// out go.
    pub(crate) fn approve_closed(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.drop_call(cx);
        self.approve_found = None;
        self.edit(
            |state| {
                state.approve = None;
                state.error.clear();
            },
            cx,
        );
    }

    /// The code the new device shows, typed: the joining key it names.
    pub(crate) fn approve_find(&mut self, code: String, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.state.busy {
            return;
        }
        self.approve_code = code.clone();
        self.edit(
            |state| {
                state.busy = true;
                state.error.clear();
            },
            cx,
        );
        let client = self.client();
        let work = async move { backend::join::find_request(&client, &code).await };
        self.call = Some(spawn_on_runtime(cx, work, |this, found, cx| {
            this.approve_found(found, cx)
        }));
    }

    pub(crate) fn approve_found(
        &mut self,
        found: Result<backend::join::Request, String>,
        cx: &mut Context<Self>,
    ) {
        let _timed = timed();
        self.call = None;
        self.edit(|state| state.busy = false, cx);
        match found {
            Ok(request) => {
                let fingerprint = backend::join::fingerprint(&request.key);
                self.approve_found = Some(request);
                self.edit(|state| state.approve = Some(fingerprint), cx);
            }
            Err(error) => self.fail(error, cx),
        }
    }

    /// This device's yes: consent it signs for the account.
    pub(crate) fn approve_confirm(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        let (Some(request), Some(Some((account, _)))) =
            (self.approve_found.clone(), self.state.account.clone())
        else {
            return;
        };
        if self.state.busy {
            return;
        }
        let (code, client, network) = (
            self.approve_code.clone(),
            self.client(),
            self.network.clone(),
        );
        self.edit(|state| state.busy = true, cx);
        let work = async move {
            backend::join::approve(&client, &network, account, &code, &request).await
        };
        self.call = Some(spawn_on_runtime(cx, work, |this, done, cx| {
            this.approve_done(done, cx)
        }));
    }

    pub(crate) fn approve_done(&mut self, done: Result<(), String>, cx: &mut Context<Self>) {
        let _timed = timed();
        self.call = None;
        self.edit(|state| state.busy = false, cx);
        match done {
            // the dialog closes (`Windows`), which clears what it found
            Ok(()) => {
                cx.emit(AccountEvent::Approved);
                cx.emit(AccountEvent::Toast(
                    "Approved. The new device finishes on its own.".into(),
                ));
            }
            Err(error) => self.fail(error, cx),
        }
    }

    /// The state a test starts from: what the screens draw, the words the
    /// phrase screens show, and the request "Add a device…" found.
    #[cfg(test)]
    pub(crate) fn seed(
        &mut self,
        state: AccountState,
        phrase: Option<Secret>,
        found: Option<backend::join::Request>,
        cx: &mut Context<Self>,
    ) {
        self.phrase = phrase;
        self.approve_found = found;
        self.edit(|current| *current = state, cx);
        cx.notify();
    }

    /// A sign-in call a test holds out, as a submit leaves it.
    #[cfg(test)]
    pub(crate) fn seed_call(&mut self, call: Task<()>, cx: &mut Context<Self>) {
        self.call = Some(call);
        self.edit(|state| state.busy = true, cx);
    }

    /// The node and network a test's account is on.
    #[cfg(test)]
    pub(crate) fn seed_network(&mut self, node: &str, network: &str, keyring: &str) {
        self.client = backend::RpcClient::new(node);
        self.network = network.into();
        self.keyring = keyring.into();
    }
}

/// Three distinct word positions (0-based, ascending) out of `words` to ask
/// back: the person types them to show the phrase was written down.
fn quiz_positions(words: usize) -> [usize; 3] {
    let mut picked = rand::seq::index::sample(&mut rand::thread_rng(), words.max(3), 3).into_vec();
    picked.sort_unstable();
    [picked[0], picked[1], picked[2]]
}

/// Whether each answer is the phrase's word at the position asked,
/// ignoring case and surrounding space.
fn quiz_matches(phrase: &str, asked: [usize; 3], answers: &[impl AsRef<str>; 3]) -> bool {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    asked.iter().zip(answers).all(|(&nth, answer)| {
        words
            .get(nth)
            .is_some_and(|word| word.eq_ignore_ascii_case(answer.as_ref().trim()))
    })
}

/// Whatever a person pastes for a recovery phrase (any run of whitespace
/// between words, any case) folded to what BIP39 checks.
fn normalize_phrase(raw: &str) -> String {
    raw.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A failed `Create` in plain words: a node that could not be reached
/// reads as that, not as a transport string; a refusal names the network
/// and says why.
fn account_error(network: &str, error: String) -> String {
    if error.contains("error sending request") {
        return format!("Can't reach {network}'s node right now. Try again in a moment.");
    }
    format!("{network} didn't create the account: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_phrase_folds_whitespace_and_case() {
        assert_eq!(
            normalize_phrase("  Canoe\n Pond\tFOREST  "),
            "canoe pond forest"
        );
    }

    #[test]
    fn the_quiz_takes_the_asked_words_in_any_case() {
        let phrase = (1..=24)
            .map(|n| format!("w{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let answers = |a: &str, b: &str, c: &str| [a.to_string(), b.to_string(), c.to_string()];
        assert!(quiz_matches(
            &phrase,
            [4, 11, 19],
            &answers("w5", " W12 ", "w20")
        ));
        assert!(!quiz_matches(
            &phrase,
            [4, 11, 19],
            &answers("w5", "w12", "w21")
        ));
        assert!(!quiz_matches(&phrase, [4, 11, 19], &answers("", "", "")));
        let asked = quiz_positions(24);
        assert!(asked[0] < asked[1] && asked[1] < asked[2] && asked[2] < 24);
    }
}
