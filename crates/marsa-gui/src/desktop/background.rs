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
}

fn tinted_svg(dark: bool, [red, green, blue]: [u8; 3]) -> String {
    let (design, original) = if dark {
        (DARK, "#7cc4ff")
    } else {
        (LIGHT, "#0a66ba")
    };
    design.replace(original, &format!("#{red:02x}{green:02x}{blue:02x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_accent_recolors_both_glows_in_each_appearance() {
        for dark in [false, true] {
            let svg = tinted_svg(dark, [0x90, 0x32, 0x46]);
            assert_eq!(svg.matches("stop-color=\"#903246\"").count(), 4);
            assert!(svg.contains(if dark { "#101827" } else { "#f4f7fc" }));
        }
    }
}
