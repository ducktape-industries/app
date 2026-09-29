//! The reducer: one message in, the state moved, a task out. Each domain
//! has its own sub-reducer; this match only routes.

use super::{AppMessage as Message, Ducktape};
use crate::ui::task::Subscription;
use crate::ui::task::Task;

/// How often the node's status is asked for while connected.
pub(super) const STATUS_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

impl Ducktape {
    /// One message from outside, and what it asks of the native side. A
    /// message that crosses between the launcher and the desk also asks
    /// the console window to take the other's size.
    pub(crate) fn handle(&mut self, message: Message) -> Task<Message> {
        let launcher = self.in_launcher();
        let task = self.update(message);
        if launcher && !self.in_launcher() {
            crate::perf::mark("desk");
        }
        match launcher == self.in_launcher() {
            true => task,
            false => Task::batch([task, crate::shell::swap_console()]),
        }
    }

    /// Each domain's arm is timed as `reducer.<domain>` (docs/perf.md) and
    /// never keyed by the message: `AppMessage`'s `Debug` prints payloads,
    /// and the typed passwords, codes and phrase words are among them.
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        use crate::perf::{Key, time};
        use Message as M;
        let timed = |domain: &'static str| time(Key::Shell, domain);
        match message {
            m @ (M::EndpointTyped(_)
            | M::ConnectSubmit
            | M::ConnectTo(_)
            | M::Connected { .. }
            | M::ConnectFailed { .. }
            | M::StatusPushed(_)
            | M::StatusMissed
            | M::AccountResolved { .. }
            | M::Disconnect
            | M::SwitchNetwork(_)
            | M::ForgetEndpoint(_)
            | M::Tick) => {
                let _timed = timed("reducer.connect");
                self.on_connect(m)
            }
            m @ (M::ToggleNetworkMenu
            | M::TogglePopover(_)
            | M::CloseOverlay(_)
            | M::OpenSpotlight
            | M::SpotlightTyped(_)
            | M::SpotlightMove { .. }
            | M::SpotlightSubmit
            | M::Spot(_)
            | M::OpenSettings
            | M::ShowSettingsPage(_)) => {
                let _timed = timed("reducer.overlay");
                self.on_overlay(m)
            }
            m @ (M::NotifyOpen(_)
            | M::NotifyMarkAllRead
            | M::NotifyClearRead
            | M::NotifySettings
            | M::NotifyPermission(..)
            | M::NotifyNotNow(_)
            | M::SetNotifyBanners(_)
            | M::SetNotifyInFront(_)
            | M::SetNotifyBurst(_)) => {
                let _timed = timed("reducer.notify");
                self.on_notify(m)
            }
            m @ (M::PasswordTyped(_)
            | M::UnlockSubmit
            | M::DeviceKey(_)
            | M::Unlocked(_)
            | M::UnlockFailed(_)
            | M::Lock
            | M::BrowseWithoutKey
            | M::SignIn
            | M::RecoveryKeyStart
            | M::PhraseCancel
            | M::PhraseWrittenDown
            | M::PhraseWordTyped(..)
            | M::PhraseCheckSubmit
            | M::RecoveryKeyAdded(_)
            | M::PhraseShowAgain
            | M::RestorePhraseTyped(_)
            | M::RecoverShow
            | M::RecoverCancel
            | M::RecoverSubmit
            | M::LinkStart
            | M::LinkCancel
            | M::Joined(_)
            | M::ApproveOpen
            | M::ApproveCodeTyped(_)
            | M::ApproveFind
            | M::ApproveFound(_)
            | M::ApproveConfirm
            | M::ApproveDone(_)
            | M::AccountNameTyped(_)
            | M::PasskeyCreateSubmit
            | M::PasskeySignInSubmit
            | M::PasskeyUsePhone
            | M::PasskeyQr(_)
            | M::PasskeyCancel
            | M::PasskeyFailed(_)
            | M::PasskeyDone(_)
            | M::ShowCreateAccount
            | M::CreateAccountLater
            | M::CreateAccountSubmit
            | M::AccountCreated(_)) => {
                let _timed = timed("reducer.sign_in");
                self.on_sign_in(m)
            }
            m @ (M::SetAppearance(_)
            | M::SetMotion(_)
            | M::SelectView(_)
            | M::OpenHelp
            | M::ViewEvent(..)
            | M::OpenLink(_)
            | M::ShowToast(_)
            | M::DismissToast
            | M::ToastTick
            | M::WallTick
            | M::ConsoleOpened(_)
            | M::WindowWasClosed(_)
            | M::WindowFocused(_)
            | M::WindowUnfocused(_)
            | M::ModifierStateChanged(_)
            | M::TrayOpen
            | M::TrayQuit) => {
                let _timed = timed("reducer.desk");
                self.on_desk(m)
            }
            m @ (M::Pane(..) | M::DeskShown { .. }) => {
                let _timed = timed("reducer.pane");
                self.on_pane(m)
            }
        }
    }

    /// What runs while the app does: the clocks, and nothing else.
    pub(crate) fn subscriptions(&self) -> Subscription<Message> {
        let mut recipes = vec![
            Subscription::run(wall_ticks),
            Subscription::run(toast_ticks),
        ];
        if self.connected {
            recipes.push(Subscription::run(status_ticks));
        }
        Subscription::batch(recipes)
    }
}

fn wall_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(std::time::Duration::from_secs(1)).map(|()| Message::WallTick)
}

fn toast_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(std::time::Duration::from_millis(300)).map(|()| Message::ToastTick)
}

fn status_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(STATUS_EVERY).map(|()| Message::Tick)
}

use futures::StreamExt as _;
