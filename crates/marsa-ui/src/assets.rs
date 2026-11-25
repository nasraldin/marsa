//! Embedded Marsa branding and the component icon catalog.

use std::borrow::Cow;

use gpui::{AssetSource, Global, SharedString};
use gpui_kit_assets::AllAssets;

const MARSA_ASSETS: &[(&str, &[u8])] = &[
    (
        "marsa/docker-engine.svg",
        include_bytes!("../../../assets/ui/docker-engine.svg"),
    ),
    (
        "marsa/docker-stack.svg",
        include_bytes!("../../../assets/ui/docker-stack.svg"),
    ),
    (
        "marsa/podman.svg",
        include_bytes!("../../../assets/ui/podman.svg"),
    ),
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

/// The proprietary optional faces are available only when installed in macOS.
pub(crate) struct InstalledMonoFonts(Vec<String>);

impl Global for InstalledMonoFonts {}

impl InstalledMonoFonts {
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.0.iter().any(|available| available == name)
    }
}

pub(crate) fn register_fonts(cx: &mut gpui::App) -> gpui::Result<()> {
    let fonts: &[&'static [u8]] = &[
        include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexSansArabic-Regular.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexSansArabic-Medium.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexSansArabic-SemiBold.ttf"),
        include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf"),
        include_bytes!("../../../assets/fonts/catalog/AnonymousPro-Regular.ttf"),
        include_bytes!("../../../assets/fonts/catalog/CascadiaCode.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Comfortaa.ttf"),
        include_bytes!("../../../assets/fonts/catalog/DMMono-Regular.ttf"),
        include_bytes!("../../../assets/fonts/catalog/DMSans.ttf"),
        include_bytes!("../../../assets/fonts/catalog/FiraCode.ttf"),
        include_bytes!("../../../assets/fonts/catalog/FiraSans-Medium.ttf"),
        include_bytes!("../../../assets/fonts/catalog/FiraSans-Regular.ttf"),
        include_bytes!("../../../assets/fonts/catalog/FiraSans-SemiBold.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Geist.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Inconsolata.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Inter.ttf"),
        include_bytes!("../../../assets/fonts/catalog/JetBrainsMono.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Lato-Medium.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Lato-Regular.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Lato-SemiBold.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Manrope.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Montserrat.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Nunito.ttf"),
        include_bytes!("../../../assets/fonts/catalog/OpenSans.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Outfit.ttf"),
        include_bytes!("../../../assets/fonts/catalog/PlusJakartaSans.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Poppins-Medium.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Poppins-Regular.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Poppins-SemiBold.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Quicksand.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Raleway.ttf"),
        include_bytes!("../../../assets/fonts/catalog/RedHatMono.ttf"),
        include_bytes!("../../../assets/fonts/catalog/Roboto.ttf"),
        include_bytes!("../../../assets/fonts/catalog/RobotoMono.ttf"),
        include_bytes!("../../../assets/fonts/catalog/SofiaSans.ttf"),
        include_bytes!("../../../assets/fonts/catalog/SourceCodePro.ttf"),
        include_bytes!("../../../assets/fonts/catalog/SourceSans3.ttf"),
        include_bytes!("../../../assets/fonts/catalog/SpaceGrotesk.ttf"),
        include_bytes!("../../../assets/fonts/catalog/SpaceMono-Regular.ttf"),
        include_bytes!("../../../assets/fonts/catalog/UbuntuMono-Regular.ttf"),
        include_bytes!("../../../assets/fonts/catalog/WorkSans.ttf"),
    ];
    let text = cx.text_system();
    text.add_fonts(fonts.iter().copied().map(Cow::Borrowed).collect())?;
    let optional = text
        .all_font_names()
        .into_iter()
        .filter(|name| matches!(name.as_str(), "Consolas" | "SF Mono"))
        .collect();
    cx.set_global(InstalledMonoFonts(optional));
    Ok(())
}
