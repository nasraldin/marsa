//! Maps the native line's UTF-16 caret affinities to Rust grapheme boundaries.

use std::{io, ptr};

use gpui_kit::Result;
use objc2_core_foundation::{
    CFAttributedString, CFDictionary, CFRetained, CFString, CFType, CGPoint,
};
use objc2_core_text::{CTFont, CTLine, kCTFontAttributeName};
use unicode_segmentation::UnicodeSegmentation as _;

pub const EDITOR_FONT: &str = "IBM Plex Sans Arabic";
pub const EDITOR_SIZE: f32 = 14.;
const POSTSCRIPT_FONT: &str = "IBMPlexSansArabic";
const DISTINCT_X: f64 = 0.01;

#[derive(Clone, Copy, Debug)]
pub struct Caret {
    pub utf16: usize,
    pub byte: usize,
    pub primary: f64,
    pub secondary: f64,
}

pub struct Geometry {
    line: CFRetained<CTLine>,
    pub carets: Vec<Caret>,
}

impl Geometry {
    pub fn new(text: &str) -> Result<Self> {
        let font_name = CFString::from_str(POSTSCRIPT_FONT);
        let string = CFString::from_str(text);
        // SAFETY: Font and string are live CF values, nil requests the default allocator
        // and identity transform, and CoreText expects a CTFont at its font attribute key.
        let line = unsafe {
            let font = CTFont::with_name(&font_name, f64::from(EDITOR_SIZE), ptr::null());
            require(
                font.post_script_name() == font_name,
                "Install IBM Plex Sans Arabic Regular before running the probe",
            )?;
            let attributes =
                CFDictionary::<CFType, CFType>::from_slices(&[kCTFontAttributeName], &[&font]);
            let attributed =
                CFAttributedString::new(None, Some(&string), Some(attributes.as_opaque()))
                    .ok_or_else(|| {
                        io::Error::other("failed to create CoreText attributed string")
                    })?;
            CTLine::with_attributed_string(&attributed)
        };
        let mut utf16 = 0;
        let boundaries = text.grapheme_indices(true).map(|(byte, cluster)| {
            let boundary = (byte, utf16);
            utf16 += cluster.encode_utf16().count();
            boundary
        });
        let mut carets = boundaries
            .map(|(byte, offset)| Self::caret(&line, byte, offset))
            .collect::<Result<Vec<_>>>()?;
        carets.push(Self::caret(&line, text.len(), utf16)?);
        Ok(Self { line, carets })
    }

    fn caret(line: &CTLine, byte: usize, utf16: usize) -> Result<Caret> {
        let mut secondary = 0.;
        // SAFETY: The index is at a UTF-16 boundary in this line and the output
        // points to initialized local storage of CoreText's CGFloat type.
        let primary = unsafe { line.offset_for_string_index(utf16.try_into()?, &mut secondary) };
        Ok(Caret {
            utf16,
            byte,
            primary,
            secondary,
        })
    }

    pub fn hit(&self, x: f64) -> usize {
        if !x.is_finite() {
            return 0;
        }
        // SAFETY: The retained text line is live and x is a finite line-relative coordinate.
        let index = unsafe { self.line.string_index_for_position(CGPoint::new(x, 0.)) };
        let index = usize::try_from(index).ok();
        let distance = |caret: &Caret| (caret.primary - x).abs().min((caret.secondary - x).abs());
        // CoreText can identify an edge within a grapheme. Pick the closest visual
        // grapheme edge and favor its native hit index at ties.
        let caret = self.carets.iter().min_by(|a, b| {
            distance(a)
                .total_cmp(&distance(b))
                .then_with(|| (index != Some(a.utf16)).cmp(&(index != Some(b.utf16))))
        });
        caret.map_or(0, |caret| caret.utf16)
    }
}

pub fn probe() -> Result<()> {
    let arabic = Geometry::new("عرب")?;
    let arabic_x: Vec<_> = arabic.carets.iter().map(|caret| caret.primary).collect();
    require(
        arabic_x.windows(2).all(|pair| pair[0] > pair[1]),
        "the native Arabic sample is missing descending visual caret positions",
    )?;
    let hit = arabic.hit(arabic_x[0] - 0.1);
    require(
        hit == 0,
        "CoreText did not hit the Arabic logical starting edge",
    )?;
    println!(
        "CoreText pure Arabic primary x (logical order): {arabic_x:.3?}; right-edge hit UTF-16 {hit}"
    );

    let mixed = Geometry::new("مرسى api-2 /work/app بيت")?;
    let dual: Vec<_> = mixed
        .carets
        .iter()
        .filter(|caret| (caret.primary - caret.secondary).abs() > DISTINCT_X)
        .map(|caret| (caret.byte, caret.utf16, caret.primary, caret.secondary))
        .collect();
    require(
        !dual.is_empty(),
        "mixed text did not expose secondary visual caret positions",
    )?;
    println!("CoreText mixed caret affinities (byte, UTF-16, primary x, secondary x): {dual:.3?}");

    let combined = Geometry::new("نَصّ👩🏽‍💻")?;
    let boundaries: Vec<_> = combined.carets.iter().map(|caret| caret.utf16).collect();
    require(
        boundaries == [0, 2, 4, 11],
        "Arabic marks or emoji split a grapheme",
    )?;
    let hits: Vec<_> = combined
        .carets
        .iter()
        .map(|caret| combined.hit(caret.primary))
        .collect();
    require(
        hits == boundaries,
        "a CoreText hit selected the interior of a grapheme",
    )?;
    println!("Arabic plus emoji UTF-16 grapheme boundaries: {boundaries:?}; hits: {hits:?}");
    Ok(())
}

pub fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}
