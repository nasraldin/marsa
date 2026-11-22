//! Embedded Marsa branding and the component icon catalog.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};
use gpui_kit_assets::AllAssets;

const BRAND: &str = "marsa/harbor.svg";

pub(crate) struct MarsaAssets;

impl AssetSource for MarsaAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if path == BRAND {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../../../assets/icons/harbor/128.svg"
            ))));
        }
        AllAssets.load(path)
    }

    fn list(&self, prefix: &str) -> gpui::Result<Vec<SharedString>> {
        let mut paths = AllAssets.list(prefix)?;
        if BRAND.starts_with(prefix) {
            paths.push(BRAND.into());
        }
        Ok(paths)
    }
}

pub(crate) fn register_fonts(cx: &gpui::App) -> gpui::Result<()> {
    let fonts: &[&'static [u8]] = &[
        include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexSansArabic-Regular.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexSansArabic-Medium.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexSansArabic-SemiBold.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf"),
    ];
    cx.text_system()
        .add_fonts(fonts.iter().copied().map(Cow::Borrowed).collect())
}
