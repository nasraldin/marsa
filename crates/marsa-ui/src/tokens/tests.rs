use std::{fs, path::Path};

use marsa_core::settings::AccentColor;

use super::{ColorToken::*, Palette, Rgb};

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
                for foreground in [TextPrimary, TextMuted] {
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
        for (name, delimiter) in [
            ("rgb", '('),
            ("rgba", '('),
            ("hsla", '('),
            ("Hsla", '{'),
            ("Rgba", '{'),
        ] {
            for (index, _) in source.match_indices(name) {
                let prefix = &source[..index];
                let identifier = |ch: char| ch.is_alphanumeric() || ch == '_';
                let suffix = source[index + name.len()..].trim_start();
                if !prefix.ends_with(identifier)
                    && !prefix.trim_end().ends_with("->")
                    && suffix.starts_with(delimiter)
                    && !prefix.rsplit('\n').next().unwrap().contains("//")
                {
                    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
                    violations.push(format!("{}:{line}: {name}", path.display()));
                }
            }
        }
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
