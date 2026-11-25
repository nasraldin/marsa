//! Color the Figma workspace glow from the same palette as interactive controls.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use marsa_ui::{
    ColorToken, Palette,
    gpui::{Image, ImageFormat, Img, ObjectFit, Styled, StyledImage, img},
    preferences::AccentColor,
};

const LIGHT: &str = include_str!("../../../../assets/ui/workspace-light.svg");
const DARK: &str = include_str!("../../../../assets/ui/workspace-dark.svg");

type GlowCache = Mutex<HashMap<(bool, [u8; 3]), Arc<Image>>>;

pub(super) fn render(palette: Palette, accent: AccentColor) -> Img {
    static CACHE: OnceLock<GlowCache> = OnceLock::new();
    let dark = palette.is_dark();
    let color = palette.color(ColorToken::AccentDefault).srgb();
    let image = if accent == AccentColor::MarsaBlue {
        img(if dark {
            "marsa/workspace-dark.png"
        } else {
            "marsa/workspace-light.png"
        })
    } else {
        let mut cache = CACHE
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        img(cache
            .entry((dark, color))
            .or_insert_with(|| {
                Arc::new(Image::from_bytes(
                    ImageFormat::Svg,
                    tinted_svg(dark, color).into_bytes(),
                ))
            })
            .clone())
    };
    image
        .object_fit(ObjectFit::Fill)
        .absolute()
        .inset_0()
        .size_full()
        .opacity(0.42)
}

fn tinted_svg(dark: bool, color: [u8; 3]) -> String {
    let (design, palette) = if dark {
        (DARK, Palette::Dark)
    } else {
        (LIGHT, Palette::Light)
    };
    let original = palette.color(ColorToken::AccentDefault).srgb();
    design.replace(&svg_color(original), &svg_color(color))
}

fn svg_color([red, green, blue]: [u8; 3]) -> String {
    format!("#{red:02x}{green:02x}{blue:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_accent_recolors_both_glows_in_each_appearance() {
        for palette in [Palette::Light, Palette::Dark] {
            let accent = palette
                .with_accent(AccentColor::Coral)
                .color(ColorToken::AccentDefault)
                .srgb();
            let svg = tinted_svg(palette.is_dark(), accent);
            assert_eq!(
                svg.matches(&format!("stop-color=\"{}\"", svg_color(accent)))
                    .count(),
                4
            );
            assert!(svg.contains(&svg_color(palette.color(ColorToken::BgSurface).srgb())));
        }
    }
}
