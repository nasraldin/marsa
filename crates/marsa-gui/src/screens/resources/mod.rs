//! Image, volume, network, and Compose project routes.

mod compose;
mod forms;
mod logs;
mod operations;
mod pull;
mod rows;

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::{collections::BTreeMap, sync::Arc};

use marsa_client::docker::{ImageDetails, VolumeUsage};

use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{App, AppContext, Context, Div, Entity, ParentElement, Styled, Task, Window, div, px},
    gpui_component::{
        Disableable, WindowExt,
        button::{Button, ButtonVariants},
        input::{Input, InputState},
    },
    tokens::{scale::Space, typography::Role},
};

use crate::{
    screens::common::{ScreenContext, align, row, technical},
    services::Services,
};

use forms::ResourceRequest;
use forms::{Form, FormKind};
use operations::Outcome;
use rows::{Columns, ResourceRow, table};

/// Resource pages reached from the desktop navigator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResourceKind {
    Images,
    Volumes,
    Networks,
    Compose,
}

impl ResourceKind {
    fn main_action(self) -> Option<FormKind> {
        match self {
            Self::Images => Some(FormKind::Pull),
            Self::Volumes => Some(FormKind::CreateVolume),
            Self::Networks => Some(FormKind::CreateNetwork),
            Self::Compose => None,
        }
    }
}

/// Persistent controls shared by the resource routes.
pub(crate) struct ResourcesView {
    filter: Entity<InputState>,
    selection: Option<(ResourceKind, String)>,
    form: Option<Form>,
    logs: Option<logs::ProjectLogs>,
    inspect: Option<ImageDetails>,
    inspection_failed: bool,
    inspection_task: Option<Task<()>>,
    busy: Option<ResourceKind>,
    outcome: Option<(ResourceKind, Outcome)>,
    pull: Option<pull::PullState>,
    operation: Option<Task<()>>,
    compose_output: Option<compose::ComposeOutputPane>,
    volume_usage: BTreeMap<String, VolumeUsage>,
    volume_scan: Option<Task<()>>,
    volume_scanning: bool,
    volume_scan_failed: bool,
}

impl ResourcesView {
    /// Create the content state on the desktop window.
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(
            &filter,
            |_, _, _: &marsa_ui::gpui_component::input::InputEvent, cx| cx.refresh_windows(),
        )
        .detach();
        Self {
            filter,
            selection: None,
            form: None,
            logs: None,
            inspect: None,
            inspection_failed: false,
            inspection_task: None,
            busy: None,
            outcome: None,
            pull: None,
            operation: None,
            compose_output: None,
            volume_usage: BTreeMap::new(),
            volume_scan: None,
            volume_scanning: false,
            volume_scan_failed: false,
        }
    }

    /// Stop viewing resource details when leaving the route.
    pub(crate) fn leave(&mut self) {
        self.selection = None;
        self.form = None;
        self.logs = None;
        self.inspect = None;
        self.inspection_task = None;
        self.volume_scan = None;
        self.volume_scanning = false;
    }

    fn open_form(
        &mut self,
        kind: FormKind,
        locale: Locale,
        palette: Palette,
        service: Arc<Services>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = kind.title(locale);
        self.form = Some(Form::new(kind, locale, window, cx));
        let view = cx.entity();
        let snapshot = service.store().select(Clone::clone);
        let state = snapshot.current().clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let screen = ScreenContext {
                locale,
                palette,
                state: &state,
                services: Some(&service),
            };
            let body = view
                .read(cx)
                .form
                .as_ref()
                .map(|form| form.render(&screen, &view, cx));
            dialog
                .width(px(560.))
                .overlay_closable(false)
                .title(text(&screen, Role::Section, title))
                .children(body)
        });
    }
}

/// Render the selected resource page from the current application snapshot.
pub(crate) fn render(
    screen: &ScreenContext<'_>,
    kind: ResourceKind,
    view: &Entity<ResourcesView>,
    cx: &App,
) -> Div {
    let data = view.read(cx);
    let filter = data.filter.read(cx).value().to_lowercase();
    let mut toolbar = logical_row(screen.locale)
        .items_center()
        .gap(px(Space::S8.pixels()))
        .child(
            div().w(px(264.)).child(
                Input::new(&data.filter)
                    .aria_label(message(screen.locale, gui::GUI_DESKTOP_FILTER_RESOURCES)),
            ),
        );
    if let Some(action) = kind.main_action() {
        let state = view.clone();
        let locale = screen.locale;
        let palette = screen.palette;
        let service = screen.services.cloned();
        toolbar = toolbar.child(
            Button::new("primary-resource-action")
                .primary()
                .label(action.title(locale))
                .disabled(screen.services.is_none())
                .on_click(move |_, window, cx| {
                    if let Some(service) = &service {
                        state.update(cx, |state, cx| {
                            state.open_form(
                                action.clone(),
                                locale,
                                palette,
                                Arc::clone(service),
                                window,
                                cx,
                            );
                        });
                    }
                }),
        );
    }

    if kind == ResourceKind::Volumes {
        let service = screen.services.cloned();
        let view = view.clone();
        toolbar = toolbar.child(
            Button::new("measure-volume-usage")
                .outline()
                .label(message(
                    screen.locale,
                    gui::GUI_DESKTOP_MEASURE_VOLUME_SIZES,
                ))
                .disabled(service.is_none() || data.volume_scanning)
                .loading(data.volume_scanning)
                .on_click(move |_, _, cx| {
                    if let Some(service) = &service {
                        view.update(cx, |view, cx| view.measure_volumes(service, cx));
                    }
                }),
        );
    }
    let columns = Columns::for_kind(screen, kind);
    let rows = rows::snapshot_rows(screen, kind, &data.volume_usage)
        .into_iter()
        .filter(|row| row.matches(&filter))
        .collect::<Vec<_>>();
    let mut content = div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(Space::S16.pixels()))
        .child(
            logical_row(screen.locale)
                .items_center()
                .justify_between()
                .child(toolbar)
                .child(
                    text(screen, Role::Small, rows.len().to_string())
                        .text_color(tone(screen, ColorToken::TextMuted)),
                ),
        )
        .child(table(
            screen,
            kind,
            &columns,
            &rows,
            view,
            data.selection.as_ref(),
        ));
    if kind == ResourceKind::Images {
        content = content.children(pull::render(
            screen,
            view,
            data.pull.as_ref(),
            data.busy == Some(ResourceKind::Images),
        ));
    }
    if kind == ResourceKind::Compose {
        content = content.children(compose::render_output(screen, data));
    }
    if kind == ResourceKind::Volumes && data.volume_scan_failed {
        content = content.child(
            text(
                screen,
                Role::Small,
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_DOCKER_DID_NOT_REPORT_VOLUME_SIZES,
                ),
            )
            .text_color(tone(screen, ColorToken::StatusWarning)),
        );
    }
    if let Some(notice) = operations::notice(screen, kind, data) {
        content = content.child(notice);
    }
    if let Some((selected_kind, selected_id)) = &data.selection {
        if *selected_kind == kind {
            if let Some(row) = rows.iter().find(|row| row.selected(selected_id)) {
                content = content.child(details(screen, kind, row, view, data));
                if kind == ResourceKind::Compose {
                    content = content.child(logs::render(screen, row, view, data.logs.as_ref()));
                }
            }
        }
    }
    content
}

fn details(
    screen: &ScreenContext<'_>,
    kind: ResourceKind,
    row: &ResourceRow,
    view: &Entity<ResourcesView>,
    data: &ResourcesView,
) -> Div {
    let locale = screen.locale;
    let actions = row
        .actions(kind)
        .into_iter()
        .enumerate()
        .map(|(index, action)| {
            let view = view.clone();
            let palette = screen.palette;
            let service = screen.services.cloned();
            Button::new(("resource-detail-action", index))
                .label(action.title(locale))
                .disabled(screen.services.is_none())
                .on_click(move |_, window, cx| {
                    if let Some(service) = &service {
                        view.update(cx, |view, cx| {
                            view.open_form(
                                action.clone(),
                                locale,
                                palette,
                                Arc::clone(service),
                                window,
                                cx,
                            );
                        });
                    }
                })
        });
    let mut detail = crate::screens::common::panel(screen)
        .flex()
        .flex_col()
        .gap(px(Space::S12.pixels()))
        .child(text(
            screen,
            Role::Section,
            message(screen.locale, gui::GUI_DESKTOP_DETAILS),
        ))
        .child(text(screen, Role::Mono, row.id.clone()).truncate())
        .children(row.details.iter().cloned().map(|(name, value)| {
            logical_row(locale)
                .justify_between()
                .gap(px(Space::S12.pixels()))
                .child(text(screen, Role::Label, name))
                .child(text(screen, Role::Mono, value).truncate())
        }))
        .child(
            logical_row(locale)
                .gap(px(Space::S8.pixels()))
                .flex_wrap()
                .children(actions),
        );
    if kind == ResourceKind::Images {
        detail = detail.children(operations::image_metadata(screen, row, data));
    }
    detail
}

fn logical_row(locale: Locale) -> Div {
    row(locale)
}

fn text(
    screen: &ScreenContext<'_>,
    role: Role,
    value: impl Into<marsa_ui::gpui::SharedString>,
) -> Div {
    let style = if screen.locale == Locale::Arabic {
        role.arabic()
    } else {
        role.latin()
    };
    let value = value.into();
    let value = if matches!(role, Role::Mono | Role::MonoSmall) {
        technical(value.as_ref()).into()
    } else {
        value
    };
    align(style.apply(div()), screen.locale)
        .child(value)
        .text_color(tone(screen, ColorToken::TextPrimary))
}

fn tone(screen: &ScreenContext<'_>, token: ColorToken) -> marsa_ui::gpui::Hsla {
    screen.palette.color(token).into()
}
