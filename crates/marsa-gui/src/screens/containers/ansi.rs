//! Interpret log SGR styling, leaving command and hyperlink sequences inert.

use std::ops::Range;

use marsa_ui::{
    ColorToken, Palette,
    direction::isolate_technical_text,
    gpui::{FontStyle, FontWeight, HighlightStyle, Hsla, StyledText, rgb},
};

use super::color;

#[derive(Clone, Copy)]
enum TerminalColor {
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Default)]
struct Attributes {
    foreground: Option<TerminalColor>,
    background: Option<TerminalColor>,
    bold: bool,
    italic: bool,
}

pub(super) fn styled(prefix: &str, input: &str, palette: Palette) -> StyledText {
    let prefix = isolate_technical_text(prefix);
    let mut text = prefix.to_string();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    walk(input, |segment, attributes| {
        let start = text.len();
        text.push_str(isolate_technical_text(segment).as_ref());
        let style = HighlightStyle {
            color: attributes
                .foreground
                .as_ref()
                .map(|color| terminal_color(color, palette)),
            background_color: attributes
                .background
                .as_ref()
                .map(|color| terminal_color(color, palette)),
            font_weight: attributes.bold.then_some(FontWeight::BOLD),
            font_style: attributes.italic.then_some(FontStyle::Italic),
            ..HighlightStyle::default()
        };
        if style != HighlightStyle::default() {
            highlights.push((start..text.len(), style));
        }
    });
    StyledText::new(text).with_highlights(highlights)
}

pub(super) fn plain(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    walk(input, |segment, _| result.push_str(segment));
    result
}

fn walk(input: &str, mut add: impl FnMut(&str, &Attributes)) {
    let mut attributes = Attributes::default();
    let mut pending = String::new();
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if !pending.is_empty() {
                add(&pending, &attributes);
                pending.clear();
            }
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    let mut parameters = String::new();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            if next == 'm' && parameters.len() <= 96 {
                                apply_sgr(&mut attributes, &parameters);
                            }
                            break;
                        }
                        if parameters.len() <= 96 {
                            parameters.push(next);
                        }
                    }
                }
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}'
                            || (next == '\u{1b}' && chars.next_if_eq(&'\\').is_some())
                        {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if !character.is_control() || character == '\t' {
            pending.push(character);
        }
    }
    if !pending.is_empty() {
        add(&pending, &attributes);
    }
}

fn apply_sgr(style: &mut Attributes, arguments: &str) {
    let mut values = arguments.split(';').map(|value| {
        if value.is_empty() {
            Some(0)
        } else {
            value.parse::<u8>().ok()
        }
    });
    while let Some(parameter) = values.next() {
        match parameter {
            Some(0) => *style = Attributes::default(),
            Some(1) => style.bold = true,
            Some(3) => style.italic = true,
            Some(22) => style.bold = false,
            Some(23) => style.italic = false,
            Some(n @ 30..=37) => style.foreground = Some(TerminalColor::Indexed(n - 30)),
            Some(n @ 40..=47) => style.background = Some(TerminalColor::Indexed(n - 40)),
            Some(n @ 90..=97) => style.foreground = Some(TerminalColor::Indexed(n - 90 + 8)),
            Some(n @ 100..=107) => style.background = Some(TerminalColor::Indexed(n - 100 + 8)),
            Some(38 | 48) => {
                let extended = match values.next().flatten() {
                    Some(5) => values.next().flatten().map(TerminalColor::Indexed),
                    Some(2) => match (
                        values.next().flatten(),
                        values.next().flatten(),
                        values.next().flatten(),
                    ) {
                        (Some(red), Some(green), Some(blue)) => {
                            Some(TerminalColor::Rgb(red, green, blue))
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(extended) = extended {
                    if parameter == Some(38) {
                        style.foreground = Some(extended);
                    } else {
                        style.background = Some(extended);
                    }
                }
            }
            Some(39) => style.foreground = None,
            Some(49) => style.background = None,
            _ => {}
        }
    }
}

fn terminal_color(value: &TerminalColor, palette: Palette) -> Hsla {
    let (red, green, blue) = match *value {
        TerminalColor::Rgb(red, green, blue) => (red, green, blue),
        TerminalColor::Indexed(index @ 0..=15) => {
            let token = match index % 8 {
                0 => ColorToken::TextMuted,
                1 => ColorToken::StatusError,
                2 => ColorToken::StatusRunning,
                3 => ColorToken::StatusWarning,
                4 => ColorToken::AccentDefault,
                5 => ColorToken::ChartMemory,
                6 => ColorToken::ChartNetwork,
                _ => ColorToken::TextPrimary,
            };
            return color(palette, token);
        }
        TerminalColor::Indexed(index @ 16..=231) => {
            let index = index - 16;
            let level = |digit: u8| if digit == 0 { 0 } else { 55 + 40 * digit };
            (level(index / 36), level(index / 6 % 6), level(index % 6))
        }
        TerminalColor::Indexed(index) => {
            let level = 8 + (index - 232) * 10;
            (level, level, level)
        }
    };
    rgb(u32::from_be_bytes([0, red, green, blue])).into()
}
