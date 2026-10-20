//! The phone's assets: gpui-kit's whole catalog (`AllAssets`, the set the
//! desktop registers), its Lucide icons drawn at the Expo build's stroke,
//! 1.75 (mobile `src/kit/Icon.tsx`), where the files say 2. The desktop
//! keeps 2. Views draw through the same source, so their icons match the
//! shell's.

use std::borrow::Cow;

use gpui_kit::assets::AllAssets;
use gpui_kit::{AssetSource, SharedString};

/// What the launchers register (`os/ios/ffi.rs`, `os/android/jni.rs`).
pub struct PhoneAssets;

const LUCIDE: &str = r#"stroke-width="2""#;
const PHONE: &str = r#"stroke-width="1.75""#;

impl AssetSource for PhoneAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        Ok(AllAssets.load(path)?.map(|svg| match thinner(&svg) {
            Some(thinner) => Cow::Owned(thinner),
            None => svg,
        }))
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        AllAssets.list(path)
    }
}

fn thinner(svg: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(svg).ok()?;
    let root = &text[..text.find('>')?];
    let at = root.find(LUCIDE)?;
    let mut text = text.to_owned();
    text.replace_range(at..at + LUCIDE.len(), PHONE);
    Some(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(source: &dyn AssetSource, path: &str) -> String {
        let bytes = source.load(path).ok().flatten().unwrap_or_default();
        String::from_utf8(bytes.into_owned()).unwrap_or_default()
    }

    #[test]
    fn lucide_icons_draw_at_the_expo_stroke_and_the_desktop_keeps_its_own() {
        let phone = load(&PhoneAssets, "icons/house.svg");
        assert!(phone.contains(PHONE) && !phone.contains(LUCIDE), "{phone}");
        assert!(load(&AllAssets, "icons/house.svg").contains(LUCIDE));
    }

    #[test]
    fn the_kits_own_icons_are_served_as_they_are() {
        for path in ["icons/resize-corner.svg", "icons/window-close.svg"] {
            assert_eq!(load(&PhoneAssets, path), load(&AllAssets, path), "{path}");
        }
    }
}
