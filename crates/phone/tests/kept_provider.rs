//! The Android launcher's kept-files provider (`Kept.kt`): no other app
//! reads the folder the host keeps files in, but the one viewer a picture's
//! view intent grants it to (J-075).
use std::path::Path;

fn launcher(path: &str) -> Result<String, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../phone/android/app/src/main");
    std::fs::read_to_string(root.join(path)).map_err(|error| format!("{path}: {error}"))
}

#[test]
fn the_kept_files_provider_is_closed_and_grants_one_read_per_view() -> Result<(), String> {
    let manifest = launcher("AndroidManifest.xml")?;
    let named = "android:name=\".Kept\"";
    let start = manifest.find(named).ok_or("no provider")?;
    let open = manifest[..start].rfind("<provider").ok_or("no element")?;
    let end = start + manifest[start..].find("/>").ok_or("no end")?;
    let provider = &manifest[open..end];
    assert!(
        provider.contains("android:exported=\"false\""),
        "{provider}"
    );
    assert!(
        provider.contains("android:grantUriPermissions=\"true\""),
        "{provider}"
    );
    let activity = launcher("java/industries/ducktape/mobile/MainActivity.kt")?;
    assert_eq!(
        activity.matches("FLAG_GRANT_READ_URI_PERMISSION").count(),
        1,
        "read is granted on the view intent alone"
    );
    assert!(!activity.contains("FLAG_GRANT_WRITE_URI_PERMISSION"));
    assert!(!activity.contains("FLAG_GRANT_PERSISTABLE_URI_PERMISSION"));
    let kept = launcher("java/industries/ducktape/mobile/Kept.kt")?;
    assert!(
        kept.contains("if (mode != \"r\") throw SecurityException"),
        "read-only"
    );
    Ok(())
}
