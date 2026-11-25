use std::{fs, path::Path};

use marsa_core::settings::{AccentColor, Theme};
use proc_macro2::{Delimiter, TokenStream, TokenTree};

use super::{Appearance, ColorToken::*, Palette, Rgb};

#[test]
fn only_system_preference_follows_appearance_changes() {
    assert_eq!(
        Palette::resolve(Theme::System, Appearance::Light),
        Palette::Light
    );
    assert_eq!(
        Palette::resolve(Theme::System, Appearance::Dark),
        Palette::Dark
    );
    assert_eq!(
        Palette::resolve(Theme::Light, Appearance::Dark),
        Palette::Light
    );
    assert_eq!(
        Palette::resolve(Theme::Dark, Appearance::Light),
        Palette::Dark
    );
}

#[test]
fn text_and_status_colors_meet_aa() {
    use AccentColor::*;

    let accents = [MarsaBlue, Ocean, Fern, Coral, Amber, Violet];
    let text_surfaces = [
        BgCanvas,
        BgSurface,
        BgSidebar,
        BgElevated,
        BgSubtle,
        BgHover,
        BgPressed,
        BgCode,
        GlassRail,
        GlassControl,
        AccentSubtle,
    ];
    let status_pairs = [
        (StatusRunning, StatusRunningBg),
        (StatusStopped, StatusStoppedBg),
        (StatusPaused, StatusPausedBg),
        (StatusError, StatusErrorBg),
        (StatusWarning, StatusWarningBg),
        (StatusStarting, StatusStartingBg),
        (StatusError, BgPressed),
    ];

    for base in [Palette::Light, Palette::Dark] {
        for accent in accents {
            let palette = base.with_accent(accent);
            for background in text_surfaces {
                for foreground in [
                    TextPrimary,
                    TextMuted,
                    StatusRunning,
                    StatusStopped,
                    StatusPaused,
                    StatusError,
                    StatusWarning,
                    StatusStarting,
                ] {
                    assert_contrast(palette, foreground, background, 4.5);
                }
            }
            for (foreground, background) in status_pairs {
                assert_contrast(palette, foreground, background, 4.5);
            }
            for background in [AccentDefault, AccentHover, AccentPressed] {
                assert_contrast(palette, TextOnAccent, background, 4.5);
            }
            assert_contrast(palette, AccentDefault, AccentSubtle, 4.5);
            assert_contrast(palette, TextOnDanger, StatusError, 4.5);
            for background in [StatusWarning, StatusRunning, StatusStarting] {
                assert_contrast(palette, TextOnAccent, background, 4.5);
            }
        }
    }
}

#[test]
fn disk_usage_categories_meet_non_text_contrast() {
    for palette in [Palette::Light, Palette::Dark] {
        for category in [DiskImages, DiskContainers, DiskVolumes, DiskBuildCache] {
            assert_contrast(palette, category, BgElevated, 3.);
        }
    }
}

#[test]
fn desktop_rust_sources_use_color_tokens() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut violations = Vec::new();
    for root in ["marsa-ui/src", "marsa-gui/src"] {
        find_color_literals(&crates.join(root), &mut violations);
    }
    assert!(
        violations.is_empty(),
        "Use semantic colors:\n{}",
        violations.join("\n")
    );
}

#[test]
fn color_lint_distinguishes_source_from_protocol_data() {
    let source = r##"
        // rgb(0xffffff)
        /* rgba(0x112233ff) */
        let help = "use rgb() for CSS";
        fn color() -> gpui::Hsla { Rgb::from_channels(red, green, blue).into() }
        let fixed = rgb(0xffffff);
        let html = "<stop stop-color='#112233'/>";
        let channels = Rgb::from_channels(255, 127, 0);
        let raw = Hsla { h: 0., s: 0., l: 0., a: 1. };
    "##;
    let mut hits = Vec::new();
    color_literals(source.parse().unwrap(), &mut hits);
    assert_eq!(
        hits,
        [
            (6, "rgb"),
            (7, "hex color"),
            (8, "from_channels"),
            (9, "Hsla")
        ]
    );
}

fn find_color_literals(path: &Path, violations: &mut Vec<String>) {
    if path.is_dir() {
        if path.file_name().is_some_and(|name| name == "tokens") {
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            find_color_literals(&entry.unwrap().path(), violations);
        }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
        let source = fs::read_to_string(path).unwrap();
        let mut hits = Vec::new();
        color_literals(source.parse().unwrap(), &mut hits);
        for (line, name) in hits {
            violations.push(format!("{}:{line}: {name}", path.display()));
        }
    }
}

fn color_literals(stream: TokenStream, hits: &mut Vec<(usize, &'static str)>) {
    let mut tokens = stream.into_iter().peekable();
    while let Some(token) = tokens.next() {
        let kind = match &token {
            TokenTree::Group(group) => {
                color_literals(group.stream(), hits);
                None
            }
            TokenTree::Literal(literal) => literal
                .to_string()
                .split('#')
                .skip(1)
                .any(|tail| {
                    matches!(
                        tail.bytes().take_while(u8::is_ascii_hexdigit).count(),
                        3 | 4 | 6 | 8
                    )
                })
                .then_some("hex color"),
            TokenTree::Ident(ident) => match tokens.peek() {
                Some(TokenTree::Group(arguments)) => constructor(&ident.to_string(), arguments),
                _ => None,
            },
            TokenTree::Punct(_) => None,
        };
        if let Some(kind) = kind {
            hits.push((token.span().start().line, kind));
        }
    }
}

fn constructor(name: &str, arguments: &proc_macro2::Group) -> Option<&'static str> {
    match (name, arguments.delimiter()) {
        ("rgb", Delimiter::Parenthesis) => Some("rgb"),
        ("rgba", Delimiter::Parenthesis) => Some("rgba"),
        ("hsla", Delimiter::Parenthesis) => Some("hsla"),
        ("black" | "white", Delimiter::Parenthesis) => Some("fixed color"),
        ("from_channels", Delimiter::Parenthesis) => arguments
            .stream()
            .into_iter()
            .any(|token| matches!(token, TokenTree::Literal(_)))
            .then_some("from_channels"),
        ("Hsla" | "Rgba", Delimiter::Brace) => {
            let mut fields = arguments.stream().into_iter();
            if matches!(fields.next(), Some(TokenTree::Ident(field)) if matches!(field.to_string().as_str(), "h" | "s" | "l" | "r" | "g" | "b" | "a"))
                && matches!(fields.next(), Some(TokenTree::Punct(punct)) if punct.as_char() == ':')
            {
                Some(if name == "Hsla" { "Hsla" } else { "Rgba" })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn assert_contrast(
    palette: Palette,
    foreground: super::ColorToken,
    background: super::ColorToken,
    minimum: f64,
) {
    let foreground_luminance = luminance(palette.color(foreground));
    let background_luminance = luminance(palette.color(background));
    let contrast = (foreground_luminance.max(background_luminance) + 0.05)
        / (foreground_luminance.min(background_luminance) + 0.05);
    assert!(
        contrast >= minimum,
        "{palette:?} {foreground:?} on {background:?}: {contrast:.3} < {minimum}"
    );
}

// WCAG 2.2 relative luminance for sRGB.
fn luminance(color: Rgb) -> f64 {
    let [red, green, blue] = color.srgb().map(|channel| {
        let channel = f64::from(channel) / 255.;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * red + 0.7152 * green + 0.0722 * blue
}
