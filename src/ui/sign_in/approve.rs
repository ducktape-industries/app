//! "Add a device…", on the device already on the account: the code typed,
//! the joining key found by it, and the approval.

use crate::backend;
use crate::ui::{AppMessage as Message, Ducktape, Overlay};
use view_wire::Task;

impl Ducktape {
    pub(super) fn on_approve(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ApproveOpen => {
                self.overlay = Some(Overlay::Approve);
                self.sign_in.approve_code.clear();
                self.sign_in.approve_found = None;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::ApproveCodeTyped(text) => {
                self.sign_in.approve_code = text;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::ApproveFind => {
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                let code = self.sign_in.approve_code.clone();
                self.sign_in.unlock_busy = true;
                self.sign_in.unlock_error.clear();
                Task::future(async move {
                    Message::ApproveFound(backend::join::find_request(&code).await)
                })
            }
            Message::ApproveFound(found) => {
                self.sign_in.unlock_busy = false;
                match found {
                    Ok(request) => self.sign_in.approve_found = Some(request),
                    Err(error) => self.sign_in.unlock_error = error,
                }
                Task::none()
            }
            Message::ApproveConfirm => {
                let (Some(request), Some(Some((account, _)))) =
                    (self.sign_in.approve_found.clone(), self.account.clone())
                else {
                    return Task::none();
                };
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                let code = self.sign_in.approve_code.clone();
                let client = backend::RpcClient::new(self.connected_rpc.clone());
                let network = self.network.clone();
                self.sign_in.unlock_busy = true;
                Task::future(async move {
                    Message::ApproveDone(
                        backend::join::approve(&client, &network, account, &code, &request).await,
                    )
                })
            }
            Message::ApproveDone(done) => {
                self.sign_in.unlock_busy = false;
                match done {
                    Ok(()) => {
                        self.close(Overlay::Approve);
                        self.update(Message::ShowToast(
                            "Approved. The new device finishes on its own.".into(),
                        ))
                    }
                    Err(error) => {
                        self.sign_in.unlock_error = error;
                        Task::none()
                    }
                }
            }
            _ => unreachable!("routed by `on_sign_in`"),
        }
    }
}
