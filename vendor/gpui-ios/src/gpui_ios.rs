//! Native iOS platform support for GPUI.
//!
//! This crate connects GPUI to UIKit, CoreText, and Metal through the shared
//! Apple renderer. It owns iOS application lifecycle, windowing, native
//! text input, safe-area and keyboard insets, and raw touch delivery.

#[cfg(target_os = "ios")]
pub mod ios;

#[cfg(target_os = "ios")]
pub use ios::{IosPlatform, current_platform};

/// The foreground color used by the iOS status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusBarContentStyle {
    /// Light status-bar content for dark backgrounds.
    Light,
    /// Dark status-bar content for light backgrounds.
    #[default]
    Dark,
}

/// The software keyboard a text field opens with ([`ios::set_keyboard`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Keyboard {
    /// The keys it shows.
    pub kind: KeyboardKind,
    /// What is typed is a secret: no suggestions, no third-party keyboard.
    pub secure: bool,
}

/// The keys a software keyboard shows (UIKit's `UIKeyboardType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyboardKind {
    /// Letters.
    #[default]
    Text,
    /// Digits.
    Numeric,
    /// Digits and a decimal point.
    Decimal,
    /// A phone number.
    Tel,
    /// An email address.
    Email,
    /// A web address.
    Url,
    /// A search.
    Search,
}

/// One item of a text field's edit menu ([`ios::show_edit_menu`]): UIKit's own standard edit action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditItem {
    /// `cut:`.
    Cut,
    /// `copy:`.
    Copy,
    /// `paste:`.
    Paste,
    /// `selectAll:`.
    SelectAll,
}
