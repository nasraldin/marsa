//! Normalize a detected engine for the existing resource views without mixing its store with Marsa's.

use std::sync::Arc;

use marsa_client::docker::{ExternalEndpoint, ExternalInventory};
use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{App, Div, Entity, ParentElement as _, SharedString, Styled as _, div, px},
    gpui_component::{
        IconName,
        button::Button,
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
};

use crate::{
    screens::common::color,
    store::{AppState, ResourceSnapshot},
};

#[derive(Clone)]
pub(crate) struct ExternalWorkspace {
    pub(crate) endpoint: ExternalEndpoint,
    pub(crate) source: String,
    pub(crate) state: AppState,
    pub(crate) reachable: bool,
    pub(crate) volumes_available: bool,
    pub(crate) networks_available: bool,
}

impl From<ExternalInventory> for ExternalWorkspace {
    fn from(inventory: ExternalInventory) -> Self {
        Self {
            endpoint: inventory.endpoint,
            source: inventory.source,
            reachable: inventory.reachable,
            volumes_available: inventory.volumes.is_some(),
            networks_available: inventory.networks.is_some(),
            state: AppState::index_resources(ResourceSnapshot {
                containers: inventory.containers,
                images: inventory.images,
                volumes: inventory.volumes.unwrap_or_default(),
                networks: inventory.networks.unwrap_or_default(),
            }),
        }
    }
}

pub(crate) type Workspaces = Arc<[Arc<ExternalWorkspace>]>;

/// The desktop's shared inventory filter. Its endpoint pins any external row selection.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) enum ContextFilter {
    #[default]
    All,
    Marsa,
    External {
        endpoint: ExternalEndpoint,
        name: Arc<str>,
    },
}

impl ContextFilter {
    pub(crate) fn includes_marsa(&self) -> bool {
        matches!(self, Self::All | Self::Marsa)
    }

    pub(crate) fn includes_external(&self, workspace: &ExternalWorkspace) -> bool {
        match self {
            Self::All => true,
            Self::External { endpoint, .. } => endpoint == &workspace.endpoint,
            Self::Marsa => false,
        }
    }

    pub(crate) fn display(&self, locale: Locale) -> SharedString {
        match self {
            Self::All => gui::GUI_DESKTOP_ALL_CONTEXTS.text(locale).into(),
            Self::Marsa => "Marsa".into(),
            Self::External { name, .. } => super::common::technical(name),
        }
    }
}

pub(crate) fn filter_button(
    locale: Locale,
    selection: &Entity<ContextFilter>,
    workspaces: &[Arc<ExternalWorkspace>],
    cx: &App,
) -> impl marsa_ui::gpui::IntoElement {
    let named = workspaces.iter().map(|workspace| ContextFilter::External {
        endpoint: workspace.endpoint.clone(),
        name: Arc::from(workspace.source.as_str()),
    });
    let choices = [ContextFilter::All, ContextFilter::Marsa]
        .into_iter()
        .chain(named)
        .collect::<Vec<_>>();
    let label = selection.read(cx).display(locale);
    let selection = selection.clone();
    Button::new("docker-context-filter")
        .outline()
        .h(px(36.))
        .icon(IconName::ChevronDown)
        .label(label)
        .accessibility_label(gui::GUI_DESKTOP_FILTER_BY_CONTEXT.text(locale))
        .dropdown_menu(move |menu, _, _| {
            choices.iter().fold(menu, |menu, choice| {
                let selection = selection.clone();
                let choice = choice.clone();
                menu.item(
                    PopupMenuItem::new(choice.display(locale)).on_click(move |_, _, cx| {
                        selection.update(cx, |filter, cx| {
                            *filter = choice.clone();
                            cx.notify();
                        });
                    }),
                )
            })
        })
}

pub(super) fn source_badge(palette: Palette, value: SharedString) -> Div {
    div()
        .max_w(px(144.))
        .flex_shrink_0()
        .truncate()
        .rounded(px(5.))
        .px_2()
        .text_xs()
        .bg(color(palette, ColorToken::AccentSubtle))
        .text_color(color(palette, ColorToken::TextPrimary))
        .child(value)
}
