//! Embedded Marsa branding and the component icon catalog.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};
use gpui_kit_assets::AllAssets;

const MARSA_ASSETS: &[(&str, &[u8])] = &[
    (
        "marsa/harbor.png",
        include_bytes!("../../../assets/ui/harbor.png"),
    ),
    (
        "marsa/workspace-light.png",
        include_bytes!("../../../assets/ui/workspace-light.png"),
    ),
    (
        "marsa/workspace-dark.png",
        include_bytes!("../../../assets/ui/workspace-dark.png"),
    ),
];

pub(crate) struct MarsaAssets;

impl AssetSource for MarsaAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = MARSA_ASSETS.iter().find(|(name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        AllAssets.load(path)
    }

    fn list(&self, prefix: &str) -> gpui::Result<Vec<SharedString>> {
        let mut paths = AllAssets.list(prefix)?;
        paths.extend(
            MARSA_ASSETS
                .iter()
                .filter_map(|(name, _)| name.starts_with(prefix).then_some((*name).into())),
        );
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
