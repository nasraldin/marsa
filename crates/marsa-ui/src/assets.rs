//! Embedded Marsa branding and the component icon catalog.
//!
//! Brand marks under `marsa/` keep their authored colors. Load them with
//! `gpui::img(...)` — never tint them with `text_color` / `Icon`.

use std::borrow::Cow;

use gpui::{AssetSource, Global, SharedString};
use gpui_kit_assets::AllAssets;

const MARSA_ASSETS: &[(&str, &[u8])] = &[
    (
        "marsa/docker.svg",
        include_bytes!("../../../assets/ui/docker.svg"),
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
        "marsa/argocd.svg",
        include_bytes!("../../../assets/ui/argocd.svg"),
    ),
    (
        "marsa/incus.svg",
        include_bytes!("../../../assets/ui/incus.svg"),
    ),
    (
        "marsa/k8s.svg",
        include_bytes!("../../../assets/ui/k8s.svg"),
    ),
    (
        "marsa/watchtower.svg",
        include_bytes!("../../../assets/ui/watchtower.svg"),
    ),
    (
        "marsa/mcp.png",
        include_bytes!("../../../assets/ui/mcp.png"),
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

/// True when `path` is a multi-color Marsa brand asset that must not be tinted.
pub fn is_brand_mark(path: &str) -> bool {
    path.starts_with("marsa/")
        && path != "marsa/workspace-light.png"
        && path != "marsa/workspace-dark.png"
}

/// Resolve a known engine / product name to a bundled brand mark.
///
/// Returns `None` when the source should keep a monochrome Lucide glyph instead.
pub fn brand_mark_for_source(source: &str) -> Option<&'static str> {
    let source = source.to_ascii_lowercase();
    if source.contains("podman") {
        Some("marsa/podman.svg")
    } else if source.contains("argocd") || source.contains("argo cd") || source.contains("argo-cd")
    {
        Some("marsa/argocd.svg")
    } else if source.contains("incus") || source.contains("lxd") {
        Some("marsa/incus.svg")
    } else if source.contains("kubernetes")
        || source.contains("k8s")
        || source.contains("k3s")
        || source.contains("kind")
        || source.contains("minikube")
    {
        Some("marsa/k8s.svg")
    } else if source.contains("watchtower") {
        Some("marsa/watchtower.svg")
    } else if source.contains("mcp") {
        Some("marsa/mcp.png")
    } else if source.contains("compose") {
        Some("marsa/docker-stack.svg")
    } else if source.contains("docker") || source == "desktop-linux" {
        Some("marsa/docker.svg")
    } else {
        None
    }
}

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

#[cfg(test)]
mod tests {
    use super::{brand_mark_for_source, is_brand_mark};

    #[test]
    fn brand_marks_map_known_engines() {
        assert_eq!(
            brand_mark_for_source("Docker Desktop"),
            Some("marsa/docker.svg")
        );
        assert_eq!(
            brand_mark_for_source("desktop-linux"),
            Some("marsa/docker.svg")
        );
        assert_eq!(brand_mark_for_source("Podman"), Some("marsa/podman.svg"));
        assert_eq!(brand_mark_for_source("kind-cluster"), Some("marsa/k8s.svg"));
        assert_eq!(brand_mark_for_source("Argo CD"), Some("marsa/argocd.svg"));
        assert_eq!(brand_mark_for_source("incus"), Some("marsa/incus.svg"));
        assert_eq!(
            brand_mark_for_source("watchtower"),
            Some("marsa/watchtower.svg")
        );
        assert_eq!(brand_mark_for_source("mcp-gateway"), Some("marsa/mcp.png"));
        assert_eq!(
            brand_mark_for_source("compose-prod"),
            Some("marsa/docker-stack.svg")
        );
        assert_eq!(
            brand_mark_for_source("Lima · docker"),
            Some("marsa/docker.svg")
        );
        assert_eq!(
            brand_mark_for_source("Colima · podman"),
            Some("marsa/podman.svg")
        );
        assert_eq!(brand_mark_for_source("custom-remote"), None);
    }

    #[test]
    fn brand_marks_are_never_workspace_backdrops() {
        assert!(is_brand_mark("marsa/docker.svg"));
        assert!(is_brand_mark("marsa/podman.svg"));
        assert!(!is_brand_mark("marsa/workspace-light.png"));
        assert!(!is_brand_mark("icons/server.svg"));
    }
}
