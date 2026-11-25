//! Image details dialog — layers stack (screenshot design) plus image action icons.

use std::{collections::BTreeSet, sync::Arc};

use marsa_client::docker::{ImageDetails, ImageHistoryLayer, ImageId};
use marsa_ui::{
    ColorToken, Locale, Palette,
    direction::isolate_technical_text,
    gpui::{
        App, Context, Div, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
        Window, div, px,
    },
    gpui_component::{
        Disableable as _, Icon, WindowExt as _,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
    locale_format::{self, SizeFormat},
    tokens::{
        scale::Space,
        typography::{Role, ui_size},
    },
};

use crate::{
    screens::common::{
        ScreenContext, dialog_actions, dialog_frame, dialog_heading, message, panel, row,
    },
    services::Services,
};

use super::{
    ResourceSource, ResourcesView, forms::FormKind, layers, rows::ResourceRow, text, tone,
};

/// Snapshot carried while the details dialog is open.
#[derive(Clone)]
pub(super) struct ImageDetailState {
    pub key: String,
    pub id: String,
    pub title: SharedString,
    pub reference: String,
    pub users: Vec<String>,
    pub source: Option<ResourceSource>,
    pub layers: Option<Vec<ImageHistoryLayer>>,
    pub layers_failed: bool,
    pub layers_expanded: BTreeSet<usize>,
}

impl ResourcesView {
    /// Open the image details modal and load inspect + layer history.
    pub(super) fn open_image_detail(
        &mut self,
        entry: &ResourceRow,
        appearance: (Locale, Palette, marsa_ui::preferences::DisplaySettings),
        service: Arc<Services>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let (locale, palette, display) = appearance;
        let source = entry.source().cloned();
        let endpoint = entry.endpoint().cloned();
        let state = ImageDetailState {
            key: entry.key().to_owned(),
            id: entry.id.clone(),
            title: isolate_technical_text(entry.display_name()),
            reference: entry.action_target.clone(),
            users: entry.users.clone(),
            source: source.clone(),
            layers: None,
            layers_failed: false,
            layers_expanded: BTreeSet::new(),
        };
        self.form = None;
        self.inspect = None;
        self.inspection_failed = false;
        self.inspection_task = None;
        self.image_detail = Some(state.clone());

        if let Ok(id) = ImageId::parse(entry.id.clone()) {
            let mut inspect_req = service.inspect_image(id.clone(), endpoint.clone());
            let mut history_req = service.image_history(id, endpoint);
            let key = state.key.clone();
            self.inspection_task = Some(cx.spawn(async move |view, cx| {
                let inspect = inspect_req.result().await;
                let history = history_req.result().await;
                let _ = view.update(cx, |view, cx| {
                    let open = view
                        .image_detail
                        .as_ref()
                        .is_some_and(|detail| detail.key == key);
                    if !open {
                        return;
                    }
                    view.inspection_task = None;
                    match inspect {
                        Some(Ok(details)) => view.inspect = Some(details),
                        _ => view.inspection_failed = true,
                    }
                    if let Some(detail) = view.image_detail.as_mut() {
                        match history {
                            Some(Ok(layers)) => {
                                detail.layers = Some(layers);
                                detail.layers_failed = false;
                            }
                            _ => {
                                detail.layers = None;
                                detail.layers_failed = true;
                            }
                        }
                    }
                    cx.refresh_windows();
                });
            }));
        } else {
            self.inspection_failed = true;
            if let Some(detail) = self.image_detail.as_mut() {
                detail.layers_failed = true;
            }
        }

        let view = cx.entity();
        let app_state = source.as_ref().map_or_else(
            || service.store().select(Clone::clone).current().clone(),
            |source| source.state.clone(),
        );
        window.open_dialog(cx, move |dialog, _, cx| {
            let screen = ScreenContext {
                locale,
                palette,
                display,
                state: &app_state,
                services: Some(&service),
            };
            let data = view.read(cx);
            let body = data.image_detail.as_ref().map_or_else(
                || div().into_any_element(),
                |detail| render_body(&screen, detail, data, &view, cx).into_any_element(),
            );
            let title_id = data
                .image_detail
                .as_ref()
                .map(|detail| detail.id.as_str())
                .unwrap_or_default();
            dialog_frame(dialog, 980.)
                .overlay_closable(false)
                .title(dialog_heading(
                    locale,
                    palette,
                    format!(
                        "{}: {}",
                        gui::GUI_DESKTOP_IMAGE_LAYERS.text(locale),
                        title_id
                    ),
                ))
                .child(body)
                .on_ok(|_, _, _| false)
                .footer(
                    dialog_actions(locale, palette).child(
                        Button::new("close-image-detail")
                            .outline()
                            .label(message(locale, gui::GUI_DESKTOP_CLOSE))
                            .on_click({
                                let view = view.downgrade();
                                move |_, window, cx| {
                                    let _ = view.update(cx, |view, _| {
                                        view.image_detail = None;
                                        view.inspect = None;
                                        view.inspection_failed = false;
                                        view.inspection_task = None;
                                    });
                                    window.close_dialog(cx);
                                }
                            }),
                    ),
                )
        });
        cx.refresh_windows();
    }
}

fn render_body(
    screen: &ScreenContext<'_>,
    detail: &ImageDetailState,
    data: &ResourcesView,
    view: &marsa_ui::gpui::Entity<ResourcesView>,
    cx: &App,
) -> impl IntoElement {
    let _ = cx;
    div()
        .flex()
        .flex_col()
        .gap(px(Space::S16.pixels()))
        .w_full()
        .child(action_icons(screen, detail, data, view))
        .child(layers::render_layers_section(
            screen,
            &detail.id,
            detail.layers.as_deref(),
            detail.layers_failed,
            &detail.layers_expanded,
            view,
        ))
        .child(inspect_cards(screen, detail, data))
}

/// Screenshot-order action icons: run · push · export · tag · delete.
fn action_icons(
    screen: &ScreenContext<'_>,
    detail: &ImageDetailState,
    data: &ResourcesView,
    view: &marsa_ui::gpui::Entity<ResourcesView>,
) -> Div {
    let locale = screen.locale;
    let busy = data.busy.is_some();
    let appearance = (locale, screen.palette, screen.display);
    let service = screen.services.cloned();
    let source = detail.source.clone();
    let can_push = !detail.reference.is_empty()
        && detail.reference != detail.id
        && !detail.reference.contains('@');
    let can_run = !detail.reference.is_empty();

    let actions: [(&'static str, FormKind, SharedString, bool); 5] = [
        (
            "icons/play.svg",
            FormKind::Run {
                id: detail.id.clone(),
                reference: detail.reference.clone(),
            },
            message(locale, gui::GUI_DESKTOP_RUN_IMAGE),
            !can_run,
        ),
        (
            "icons/upload.svg",
            FormKind::PushImage {
                id: detail.id.clone(),
                reference: detail.reference.clone(),
            },
            message(locale, gui::GUI_DESKTOP_PUSH_IMAGE),
            !can_push,
        ),
        (
            "icons/download.svg",
            FormKind::ExportImage {
                id: detail.id.clone(),
                reference: detail.title.to_string(),
            },
            message(locale, gui::GUI_DESKTOP_EXPORT_IMAGE),
            false,
        ),
        (
            "icons/tag.svg",
            FormKind::TagImage {
                id: detail.id.clone(),
                reference: detail.reference.clone(),
            },
            message(locale, gui::GUI_DESKTOP_TAG_IMAGE),
            false,
        ),
        (
            "icons/trash.svg",
            FormKind::RemoveImage {
                id: detail.id.clone(),
                reference: detail.reference.clone(),
                users: detail.users.clone(),
            },
            message(locale, gui::GUI_DESKTOP_REMOVE_IMAGE),
            false,
        ),
    ];

    row(locale)
        .w_full()
        .items_center()
        .gap_1()
        .children(
            actions
                .into_iter()
                .enumerate()
                .map(|(index, (icon, kind, label, disabled))| {
                    let state = view.clone();
                    let service = service.clone();
                    let source = source.clone();
                    Button::new(("image-detail-icon", index))
                        .ghost()
                        .size(px(32.))
                        .p_0()
                        .icon(Icon::default().path(icon))
                        .accessibility_label(label.clone())
                        .tooltip(label)
                        .disabled(service.is_none() || busy || disabled)
                        .on_click(move |_, window, cx| {
                            let Some(service) = &service else {
                                return;
                            };
                            state.update(cx, |view, _| {
                                view.image_detail = None;
                                view.inspect = None;
                                view.inspection_failed = false;
                                view.inspection_task = None;
                            });
                            window.close_dialog(cx);
                            state.update(cx, |view, cx| {
                                view.open_form(
                                    (kind.clone(), source.clone()),
                                    appearance,
                                    Arc::clone(service),
                                    window,
                                    cx,
                                );
                            });
                        })
                }),
        )
}

fn inspect_cards(
    screen: &ScreenContext<'_>,
    detail: &ImageDetailState,
    data: &ResourcesView,
) -> Div {
    let locale = screen.locale;
    if data.inspection_task.is_some() && data.inspect.is_none() && !data.inspection_failed {
        return panel(screen).child(
            text(
                screen,
                Role::Small,
                message(locale, gui::GUI_DESKTOP_LOADING_IMAGE_METADATA),
            )
            .text_color(tone(screen, ColorToken::TextMuted)),
        );
    }
    if data.inspection_failed {
        return panel(screen).child(
            text(
                screen,
                Role::Small,
                message(locale, gui::GUI_DESKTOP_IMAGE_METADATA_IS_UNAVAILABLE),
            )
            .text_color(tone(screen, ColorToken::StatusWarning)),
        );
    }
    let Some(image) = data.inspect.as_ref().filter(|image| image.id == detail.id) else {
        return div();
    };
    div()
        .flex()
        .flex_col()
        .gap(px(Space::S12.pixels()))
        .child(metadata_card(screen, image))
        .child(config_card(screen, image))
        .children(tags_card(screen, image))
}

fn metadata_card(screen: &ScreenContext<'_>, image: &ImageDetails) -> Div {
    let created = image
        .created
        .as_deref()
        .and_then(|created| {
            locale_format::rfc3339_utc(screen.locale, screen.display.time_format, created)
        })
        .unwrap_or_else(|| message(screen.locale, gui::GUI_DESKTOP_UNAVAILABLE).to_string());
    let author = image
        .author
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| gui::GUI_DESKTOP_NONE.text(screen.locale));
    section_card(
        screen,
        message(screen.locale, gui::GUI_DESKTOP_IMAGE_METADATA),
        [
            (
                message(screen.locale, gui::GUI_DESKTOP_PLATFORM),
                SharedString::from(format!("{}/{}", image.os, image.architecture)),
            ),
            (
                message(screen.locale, gui::GUI_DESKTOP_CREATED),
                SharedString::from(created),
            ),
            (
                message(screen.locale, gui::GUI_DESKTOP_AUTHOR),
                isolate_technical_text(author),
            ),
            (
                message(screen.locale, gui::GUI_DESKTOP_SIZE),
                SharedString::from(locale_format::bytes(
                    screen.locale,
                    SizeFormat::DecimalDetail,
                    image.size,
                )),
            ),
        ],
    )
}

fn config_card(screen: &ScreenContext<'_>, image: &ImageDetails) -> Div {
    let config = image.config.as_ref();
    let cmd = config
        .map(|config| config.cmd.join(" "))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| gui::GUI_DESKTOP_NONE.text(screen.locale).to_owned());
    let entrypoint = config
        .map(|config| config.entrypoint.join(" "))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| gui::GUI_DESKTOP_NONE.text(screen.locale).to_owned());
    let workdir = config
        .and_then(|config| config.working_dir.as_deref())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| gui::GUI_DESKTOP_NONE.text(screen.locale));
    let user = config
        .and_then(|config| config.user.as_deref())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| gui::GUI_DESKTOP_NONE.text(screen.locale));
    section_card(
        screen,
        message(screen.locale, gui::GUI_DESKTOP_IMAGE_CONFIGURATION),
        [
            (
                message(screen.locale, gui::GUI_DESKTOP_COMMAND),
                isolate_technical_text(&cmd),
            ),
            (
                message(screen.locale, gui::GUI_DESKTOP_ENTRYPOINT),
                isolate_technical_text(&entrypoint),
            ),
            (
                message(screen.locale, gui::GUI_DESKTOP_WORKING_DIR),
                isolate_technical_text(workdir),
            ),
            (
                message(screen.locale, gui::GUI_DESKTOP_USER),
                isolate_technical_text(user),
            ),
        ],
    )
}

fn tags_card(screen: &ScreenContext<'_>, image: &ImageDetails) -> Option<Div> {
    if image.repo_tags.is_empty() && image.repo_digests.is_empty() {
        return None;
    }
    let mut card = section_heading(screen, message(screen.locale, gui::GUI_DESKTOP_TAGS));
    for tag in &image.repo_tags {
        card = card.child(
            div()
                .font_family("IBM Plex Mono")
                .text_size(ui_size(12.))
                .child(isolate_technical_text(tag)),
        );
    }
    for digest in &image.repo_digests {
        card = card.child(
            div()
                .font_family("IBM Plex Mono")
                .text_size(ui_size(11.))
                .text_color(tone(screen, ColorToken::TextMuted))
                .child(isolate_technical_text(digest)),
        );
    }
    Some(card)
}

fn section_card(
    screen: &ScreenContext<'_>,
    title: SharedString,
    fields: impl IntoIterator<Item = (SharedString, SharedString)>,
) -> Div {
    let mut card = section_heading(screen, title);
    for (label, value) in fields {
        card = card.child(
            row(screen.locale)
                .gap_3()
                .items_start()
                .child(
                    div()
                        .w(px(120.))
                        .flex_shrink_0()
                        .text_size(ui_size(12.))
                        .text_color(tone(screen, ColorToken::TextMuted))
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family("IBM Plex Mono")
                        .text_size(ui_size(12.))
                        .child(value),
                ),
        );
    }
    card
}

fn section_heading(screen: &ScreenContext<'_>, title: SharedString) -> Div {
    panel(screen).child(
        div()
            .text_size(ui_size(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .pb_2()
            .child(title),
    )
}
