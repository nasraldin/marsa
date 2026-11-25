//! Logical GUI layout, selective icon mirroring, and technical text isolation.

use std::fmt::Write as _;

use gpui::{
    AbsoluteLength, Anchor, App, DefiniteLength, Div, Length, SharedString, Styled, TextAlign,
    Window, div, point, px,
};
use gpui_base::{LayoutDirection, LogicalStyledExt};
use marsa_l10n::Locale;

/// Set the initial layout direction and install the directional icon resolver.
pub fn init(locale: Locale, cx: &mut App) {
    set_locale(locale, cx);
    gpui_component::set_icon_resolver(cx, |path, cx| {
        if LayoutDirection::global(cx).is_rtl() {
            mirrored_icon(path).map(SharedString::from)
        } else {
            None
        }
    });
}

/// Relayout all windows and update Component behavior for the selected locale.
pub fn set_locale(locale: Locale, cx: &mut App) {
    match locale {
        Locale::EnglishUs => LayoutDirection::Ltr,
        Locale::Arabic => LayoutDirection::Rtl,
    }
    .set_global(cx);
}

/// Create a horizontal row whose first child is at the logical start.
pub fn row(cx: &App) -> Div {
    div().flex_row_dir(cx)
}

/// Create an inline layout for the specified language, independent of app state.
pub fn row_for(locale: Locale) -> Div {
    let row = div().flex().items_center();
    match locale {
        Locale::EnglishUs => row.flex_row(),
        Locale::Arabic => row.flex_row_reverse(),
    }
}

/// Create a fixed left-to-right row for ordered technical data such as a chart axis.
pub fn technical_row() -> Div {
    div().flex().flex_row()
}

/// Align the children of a column to its logical start.
pub fn align_column_to_start(element: Div, locale: Locale) -> Div {
    match locale {
        Locale::EnglishUs => element.items_start(),
        Locale::Arabic => element.items_end(),
    }
}

/// Anchor a popover at the top of the trigger's logical start edge.
pub fn top_start_anchor(locale: Locale) -> Anchor {
    match locale {
        Locale::EnglishUs => Anchor::TopLeft,
        Locale::Arabic => Anchor::TopRight,
    }
}

/// Anchor a popover at the bottom of the trigger's logical start edge.
pub fn bottom_start_anchor(locale: Locale) -> Anchor {
    match locale {
        Locale::EnglishUs => Anchor::BottomLeft,
        Locale::Arabic => Anchor::BottomRight,
    }
}

/// Align element text to the inline start for the specified language.
pub fn align_to_start(element: Div, locale: Locale) -> Div {
    match locale {
        Locale::EnglishUs => element.text_align(TextAlign::Left),
        Locale::Arabic => element.text_align(TextAlign::Right),
    }
}

/// Width of the collapsed icon rail, including its border.
/// Wide enough for centered 36px icon buttons with equal start/end padding, and
/// for the traffic-light cluster above them.
pub const COLLAPSED_SIDEBAR_WIDTH: f32 = 88.;
/// Shell inset around the sidebar — must match the desktop row `p_2`.
const SHELL_INSET: f32 = 8.;
/// Width of the close→zoom traffic-light cluster (buttons + gaps).
/// Used to center the cluster on the same axis as the collapsed toggle/brand.
const TRAFFIC_LIGHT_WIDTH: f32 = 54.;
/// Vertical inset for the traffic lights (GPUI pads this amount above and below).
const TRAFFIC_LIGHT_Y: f32 = 22.;

/// Position the native macOS controls in the workspace's leading sidebar.
///
/// Compact mode centers the traffic-light cluster on the collapsed sidebar so it
/// lines up with the toggle and brand below. Expanded mode uses a standard
/// titlebar leading inset.
pub fn position_window_controls(window: &Window, locale: Locale, compact: bool) {
    let x = if compact {
        // Sidebar starts after the shell inset; center the cluster in that rail.
        SHELL_INSET + (COLLAPSED_SIDEBAR_WIDTH - TRAFFIC_LIGHT_WIDTH) * 0.5
    } else {
        24.
    };
    let inset = match locale {
        Locale::EnglishUs => px(x),
        Locale::Arabic => window.viewport_size().width - px(x + TRAFFIC_LIGHT_WIDTH),
    };
    window.set_traffic_light_position(point(inset, px(TRAFFIC_LIGHT_Y)));
}

/// Return the sidebar toggle glyph for the component resolver to mirror.
pub fn sidebar_toggle(compact: bool) -> gpui_component::IconName {
    use gpui_component::IconName;
    if compact {
        IconName::PanelLeftOpen
    } else {
        IconName::PanelLeftClose
    }
}

/// Return the sidebar-to-content launch arrow for the current reading direction.
pub fn launch_arrow(locale: Locale) -> &'static str {
    match locale {
        Locale::EnglishUs => "icons/arrow-up-right.svg",
        Locale::Arabic => "icons/arrow-up-left.svg",
    }
}

macro_rules! logical_length {
    ($(#[$doc:meta])* $name:ident, $method:ident, $length:ty) => {
        $(#[$doc])*
        fn $name(self, value: impl Into<$length>, cx: &App) -> Self {
            self.$method(value.into(), cx)
        }
    };
}

/// Style the inline edges of a GPUI element using the active UI direction.
pub trait LogicalStyle: Styled + Sized {
    logical_length!(
        /// Set padding at the start edge.
        padding_start,
        ps,
        DefiniteLength
    );
    logical_length!(
        /// Set padding at the end edge.
        padding_end,
        pe,
        DefiniteLength
    );
    logical_length!(
        /// Set the margin at the start edge.
        margin_start,
        ms,
        Length
    );
    logical_length!(
        /// Set the margin at the end edge.
        margin_end,
        me,
        Length
    );
    logical_length!(
        /// Set the border width at the start edge.
        border_start,
        border_s,
        AbsoluteLength
    );
    logical_length!(
        /// Set the border width at the end edge.
        border_end,
        border_e,
        AbsoluteLength
    );
    logical_length!(
        /// Set the inset from the positioned ancestor's start edge.
        inset_start,
        inset_s,
        Length
    );
    logical_length!(
        /// Set the inset from the positioned ancestor's end edge.
        inset_end,
        inset_e,
        Length
    );

    /// Align inline text to the start edge.
    fn text_start(self, cx: &App) -> Self {
        LogicalStyledExt::text_start(self, cx)
    }

    /// Align inline text to the end edge.
    fn text_end(self, cx: &App) -> Self {
        LogicalStyledExt::text_end(self, cx)
    }

    /// Pack the children against the start edge of a [row].
    fn justify_row_start(self) -> Self {
        self.justify_flex_start()
    }

    /// Pack the children against the end edge of a [row].
    fn justify_row_end(self) -> Self {
        self.justify_flex_end()
    }
}

impl<T: Styled> LogicalStyle for T {}

fn mirrored_icon(path: &str) -> Option<&'static str> {
    const PAIRS: &[(&str, &str)] = &[
        ("icons/arrow-left.svg", "icons/arrow-right.svg"),
        ("icons/chevron-left.svg", "icons/chevron-right.svg"),
        ("icons/panel-left.svg", "icons/panel-right.svg"),
        ("icons/panel-left-close.svg", "icons/panel-right-close.svg"),
        ("icons/panel-left-open.svg", "icons/panel-right-open.svg"),
    ];
    PAIRS.iter().find_map(|&(start, end)| match path {
        path if path == start => Some(end),
        path if path == end => Some(start),
        _ => None,
    })
}

/// Isolate a technical value in left-to-right display runs, even inside Arabic.
///
/// Line and paragraph breaks are retained, with a new isolate after each one.
/// Embedded Unicode direction controls are written as visible escapes so a value
/// cannot change its own direction or interrupt an enclosing run. Keep the
/// original value separately for operations such as copying an identifier.
pub fn isolate_technical_text(value: &str) -> SharedString {
    const LRI: char = '\u{2066}';
    const PDI: char = '\u{2069}';
    let mut display = String::with_capacity(value.len() + LRI.len_utf8() + PDI.len_utf8());
    let mut in_run = false;

    for character in value.chars() {
        match character {
            '\r' | '\n' | '\u{001c}'..='\u{001e}' | '\u{0085}' | '\u{2028}' | '\u{2029}' => {
                if in_run {
                    display.push(PDI);
                    in_run = false;
                }
                display.push(character);
            }
            _ => {
                if !in_run {
                    display.push(LRI);
                    in_run = true;
                }
                match character {
                    '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}' => {
                        // Formatting into a String is infallible.
                        let _ = write!(display, "{}", character.escape_unicode());
                    }
                    _ => display.push(character),
                }
            }
        }
    }
    if in_run {
        display.push(PDI);
    }
    display.into()
}

#[cfg(test)]
mod tests {
    use super::isolate_technical_text;

    #[test]
    fn starts_a_fresh_isolate_after_a_paragraph_break() {
        let value = "/work/app\r\n\u{2029}repo:3";
        assert_eq!(
            isolate_technical_text(value).as_ref(),
            "\u{2066}/work/app\u{2069}\r\n\u{2029}\u{2066}repo:3\u{2069}",
        );
        assert_eq!(isolate_technical_text("").as_ref(), "");
    }

    #[test]
    fn escapes_direction_controls_inside_a_value() {
        let value = "--name=\u{2069}\u{202e}abc\u{061c}";
        assert_eq!(
            isolate_technical_text(value).as_ref(),
            "\u{2066}--name=\\u{2069}\\u{202e}abc\\u{61c}\u{2069}",
        );
    }
}
