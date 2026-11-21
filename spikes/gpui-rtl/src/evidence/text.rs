//! Tests Arabic shaping and mixed-direction technical values with actual CoreText glyphs.

use gpui_kit::{Font, GlyphId, Result, ShapedLine, SharedString, TextRun, Window, font, px};

use crate::view::{ARABIC_FAMILY, IMAGE, Language, NAME, PATH, TYPE_SCALE, mixed_sentence};

use super::require;

pub(super) fn run(window: &Window) -> Result<()> {
    mixed_order(window)?;
    joining_across_scale(window)
}

fn mixed_order(window: &Window) -> Result<()> {
    let sentence = mixed_sentence(Language::Arabic);
    let shaped = shape(window, sentence.clone(), 17., font(ARABIC_FAMILY));
    let mut token_origins = Vec::new();

    for token in [NAME, IMAGE, PATH] {
        let start = sentence
            .find(token)
            .ok_or_else(|| std::io::Error::other("fixture token missing"))?;
        let mut glyphs: Vec<_> = shaped
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .filter(|glyph| (start..start + token.len()).contains(&glyph.index))
            .collect();
        glyphs.sort_by_key(|glyph| glyph.index);
        require(
            glyphs.len() == token.len(),
            "a technical token was clipped or combined unexpectedly",
        )?;
        require(
            glyphs
                .windows(2)
                .all(|pair| pair[0].position.x < pair[1].position.x),
            "text inside an isolated technical token runs in the wrong visual direction",
        )?;
        token_origins.push(f32::from(glyphs[0].position.x));
    }
    println!(
        "Arabic paragraph token x positions in logical order [name, image, path]: {token_origins:?}"
    );
    require(
        token_origins.windows(2).all(|pair| pair[0] > pair[1]),
        "Arabic technical cells were not placed in right-to-left visual order",
    )
}

fn joining_across_scale(window: &Window) -> Result<()> {
    let face = font(ARABIC_FAMILY);
    require(
        window
            .text_system()
            .all_font_names()
            .iter()
            .any(|name| name == ARABIC_FAMILY),
        "IBM Plex Sans Arabic is required for a valid shaping capture",
    )?;

    let mut faces = Vec::new();
    for &(size, _, weight) in TYPE_SCALE {
        let face = Font {
            weight,
            ..face.clone()
        };
        let joined = shape(window, "عرب".into(), size, face.clone());
        let isolated = shape(window, "ع\u{200c}ر\u{200c}ب".into(), size, face);

        let font_id = joined
            .runs
            .first()
            .ok_or_else(|| std::io::Error::other("Arabic font run missing"))?
            .font_id;
        if !faces.contains(&font_id) {
            faces.push(font_id);
        }

        let joined_ids = glyph_ids(&joined, &[0, 2, 4]);
        let isolated_ids = glyph_ids(&isolated, &[0, 5, 10]);
        require(
            joined_ids.len() == 3
                && isolated_ids.len() == 3
                && joined_ids[0] != isolated_ids[0]
                && joined_ids[1] != isolated_ids[1]
                && joined_ids[2] == isolated_ids[2]
                && joined_ids.iter().all(|glyph| glyph.0 != 0),
            "an Arabic face did not render the expected connected glyph forms",
        )?;
        println!(
            "Arabic shaping {size:>2} px, weight {weight:?}: joined {joined_ids:?}, isolated {isolated_ids:?}"
        );
    }
    require(
        faces.len() == 3,
        "Arabic font weights resolved to fewer than three distinct faces",
    )
}

fn glyph_ids(shaped: &ShapedLine, offsets: &[usize]) -> Vec<GlyphId> {
    let glyphs: Vec<_> = shaped.runs.iter().flat_map(|run| &run.glyphs).collect();
    offsets
        .iter()
        .filter_map(|index| {
            glyphs
                .iter()
                .find(|glyph| glyph.index == *index)
                .map(|glyph| glyph.id)
        })
        .collect()
}

fn shape(window: &Window, value: SharedString, size: f32, face: Font) -> ShapedLine {
    let runs = [TextRun {
        len: value.len(),
        font: face,
        ..TextRun::default()
    }];
    window
        .text_system()
        .shape_line(value, px(size), &runs, None)
}
