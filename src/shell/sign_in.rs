//! The desktop's own sign-in steps, driven through `Account`'s callbacks:
//! the ones that need what only the desktop links (the key crate's
//! password-locked files and mnemonics, the passkey ceremony in the
//! browser). Each has the shape the launcher's fields call
//! (`fn(&mut Account, [String,] &mut Context<Account>)`), beside
//! `Account`'s own methods.

use super::entities::{Account, Secret, spawn_on_runtime};
use crate::backend;
use gpui_kit::Context;

pub(crate) fn unlock(account: &mut Account, password: String, cx: &mut Context<Account>) {
    if !account.unlock(cx) || !account.get().key_exists {
        return;
    }
    if password.is_empty() {
        return account.fail("Type this key's password first.", cx);
    }
    let password = Secret::new(password);
    let keyring = account.keyring().to_owned();
    let network = account.network().to_owned();
    let work = async move {
        let opened =
            tokio::task::spawn_blocking(move || backend::open_password_locked(&keyring, &password))
                .await
                .unwrap_or_else(|_| Err("opening this device's key did not finish".into()));
        match opened {
            Ok(key) => Ok(backend::seat_key(&network, std::sync::Arc::new(key)).await),
            Err(error) => Err(backend::user_error(error)),
        }
    };
    let call = spawn_on_runtime(cx, work, |account, opened, cx| match opened {
        Ok(pubkey) => account.unlocked(pubkey, cx),
        Err(error) => account.unlock_failed(error, cx),
    });
    account.begin_call(call, cx);
}

pub(crate) fn recovery_key_start(account: &mut Account, cx: &mut Context<Account>) {
    account.recovery_key_start(backend::recovery::new_recovery_phrase().into(), cx);
}

pub(crate) fn phrase_check(account: &mut Account, answers: [String; 3], cx: &mut Context<Account>) {
    let Some(phrase) = account.phrase_check(answers, cx) else {
        return;
    };
    let (client, network) = (account.client(), account.network().to_owned());
    let work = async move { backend::recovery::add_recovery_key(&client, &network, &phrase).await };
    let call = spawn_on_runtime(cx, work, |account, added, cx| {
        account.recovery_key_added(added, cx)
    });
    account.begin_call(call, cx);
}

pub(crate) fn recover_submit(account: &mut Account, phrase: String, cx: &mut Context<Account>) {
    let Some(phrase) = account.recover_submit(phrase, cx) else {
        return;
    };
    if keystore::userkey::seed_of_mnemonic(&phrase).is_err() {
        return account.fail("Those words aren't a recovery key — check each one.", cx);
    }
    let (client, network) = (account.client(), account.network().to_owned());
    let work =
        async move { backend::recovery::join_with_recovery_key(&client, &network, &phrase).await };
    let call = spawn_on_runtime(cx, work, |account, joined, cx| account.joined(joined, cx));
    account.begin_call(call, cx);
}

pub(crate) fn passkey_create(account: &mut Account, name: String, cx: &mut Context<Account>) {
    passkey(account, Some(name), cx);
}

pub(crate) fn passkey_sign_in(account: &mut Account, cx: &mut Context<Account>) {
    passkey(account, None, cx);
}

fn passkey(account: &mut Account, create: Option<String>, cx: &mut Context<Account>) {
    let Some(name) = account.passkey_begin(create, cx) else {
        return;
    };
    let (client, network, pubkey) = (
        account.client(),
        account.network().to_owned(),
        account.get().signer_key.clone(),
    );
    let work = async move {
        match &name {
            Some(name) => backend::passkey::create_account(&client, &network, name).await,
            None => backend::passkey::sign_in(&client, &network).await,
        }
        .map(|()| pubkey)
    };
    let ceremony = spawn_on_runtime(cx, work, |account, joined, cx| match joined {
        Ok(pubkey) => account.passkey_done(pubkey, cx),
        Err(error) => account.passkey_failed(error, cx),
    });
    account.hold_passkey(ceremony);
}
