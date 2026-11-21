//! Qualify focus order, modal safety and menu commands over four variants.

mod fixture;
mod probe;

use gpui_kit::{Result, base::LayoutDirection, component::ThemeMode};

use fixture::{ARABIC, ENGLISH};
use probe::{check, probe};

pub(super) fn measure() -> Result<()> {
    let variants = [
        ("en-light", ENGLISH, LayoutDirection::Ltr, ThemeMode::Light),
        ("en-dark", ENGLISH, LayoutDirection::Ltr, ThemeMode::Dark),
        ("ar-light", ARABIC, LayoutDirection::Rtl, ThemeMode::Light),
        ("ar-dark", ARABIC, LayoutDirection::Rtl, ThemeMode::Dark),
    ];
    let mut failures = Vec::new();
    for (name, labels, direction, mode) in variants {
        if let Err(error) = probe(name, labels, direction, mode) {
            eprintln!("{name}: {error:#}");
            failures.push(name);
        }
    }
    check(
        failures.is_empty(),
        format!("failed variants: {}", failures.join(", ")),
    )
}
