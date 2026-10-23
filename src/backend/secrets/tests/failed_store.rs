use super::*;

#[test]
fn platform_failure_releases_the_cached_store_but_denial_keeps_it() {
    const CHILD: &str = "DUCKTAPE_KEYRING_FAILURE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "backend::secrets::tests::failed_store::platform_failure_releases_the_cached_store_but_denial_keeps_it"])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    keyring::set_default_store(keyring::mock::Store::new().unwrap());
    let denied = keyring::Error::NoStorageAccess("locked".into());
    assert!(matches!(refused_on(denied, true), KeyRefusal::Denied(_)));
    assert!(keyring::get_default_store().is_some());
    let failed = keyring::Error::PlatformFailure("daemon restarted".into());
    assert!(matches!(refused_on(failed, true), KeyRefusal::NoStore(_)));
    assert!(keyring::get_default_store().is_none());
    keyring::set_default_store(keyring::mock::Store::new().unwrap());
    let failed = keyring::Error::PlatformFailure("provider stopped".into());
    assert!(matches!(refused_on(failed, false), KeyRefusal::Denied(_)));
    assert!(keyring::get_default_store().is_none());
}
