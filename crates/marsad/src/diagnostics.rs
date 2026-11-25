//! Pair English and Arabic text for the daemon's command-line diagnostics.

use std::env;

#[cfg(target_os = "macos")]
pub(super) const LOGIN_USAGE: &str = "marsad [login status|enable|disable | uninstall]";

pub(super) fn usage(syntax: &str) -> String {
    format!("{} {syntax}", localized("Usage", "الاستخدام"))
}

pub(super) fn localized(english: &'static str, arabic: &'static str) -> &'static str {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|key| env::var(key).ok())
        .find(|value| !value.is_empty());
    if locale
        .as_deref()
        .and_then(|value| value.split(['_', '-', '.', '@']).next())
        == Some("ar")
    {
        arabic
    } else {
        english
    }
}
