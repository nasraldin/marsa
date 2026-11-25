//! Create-container dialog opened from the Images table Run action.

use std::{cell::Cell, rc::Rc, sync::Arc};

use marsa_client::docker::{
    ContainerMountReview, ExternalEndpoint, ImageId, ImageReference, ImageRun, ResourceName,
    container_config::{ConfigMountKind, ContainerCreateOptions},
};
use marsa_ui::{
    ColorToken, Locale, Palette,
    direction::{LogicalStyle as _, isolate_technical_text},
    gpui::{
        App, Context, Div, FontWeight, ParentElement as _, PromptButton, PromptLevel, SharedString,
        Styled as _, WeakEntity, Window, div, px,
    },
    gpui_component::{
        Disableable as _, Icon, IconName, WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::InputState,
    },
    gui_keys as gui,
    tokens::{scale::Space, typography::ui_size},
};

use crate::{
    screens::{
        common::{color, dialog_actions, dialog_frame, dialog_heading, resource_input, row},
        config_options::{DraftOptions, OptionsEditor, error_line, error_text, field},
        external_inventory::dialog_scope,
    },
    services::Services,
};

use super::{
    ResourceKind, ResourceSource, ResourcesView,
    forms::FormKind,
    operations::{Outcome, SuccessKind},
};

struct Launch {
    services: Arc<Services>,
    endpoint: Option<ExternalEndpoint>,
    image: ImageId,
    reference: String,
    name: Option<ResourceName>,
    options: ContainerCreateOptions,
    pull_before: bool,
    start_after: bool,
}

impl Launch {
    fn submit(
        self,
        owner: &WeakEntity<ResourcesView>,
        badge: &SharedString,
        locale: Locale,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        owner
            .update(cx, |view, cx| {
                if view.busy.is_some() {
                    return false;
                }
                let review = self
                    .endpoint
                    .as_ref()
                    .filter(|_| self.options.has_host_mounts())
                    .map(|endpoint| {
                        let mut body = format!(
                            "{}\n\n{} · {}",
                            gui::GUI_CONFIG_REVIEW_HOST_HELP.text(locale),
                            badge,
                            isolate_technical_text(&endpoint.address()),
                        );
                        for mount in self
                            .options
                            .mounts
                            .iter()
                            .filter(|mount| mount.kind == ConfigMountKind::Host)
                        {
                            let mode = if mount.read_only {
                                gui::GUI_CONFIG_READ_ONLY
                            } else {
                                gui::GUI_CONFIG_READ_WRITE
                            };
                            use std::fmt::Write as _;
                            let _ = write!(
                                body,
                                "\n{} → {} · {}",
                                isolate_technical_text(&mount.source),
                                isolate_technical_text(&mount.target),
                                mode.text(locale),
                            );
                        }
                        window.prompt(
                            PromptLevel::Warning,
                            gui::GUI_CONFIG_REVIEW_HOST_TITLE.text(locale),
                            Some(&body),
                            &[
                                PromptButton::Ok(
                                    gui::GUI_DESKTOP_CREATE_CONTAINER.text(locale).into(),
                                ),
                                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
                            ],
                            cx,
                        )
                    });
                view.busy = Some(ResourceKind::Images);
                view.outcome = None;
                cx.spawn(async move |view, cx| {
                    let approval = if let Some(answer) = review {
                        if answer.await != Ok(0) {
                            let _ = view.update(cx, |view, cx| {
                                view.busy = None;
                                cx.refresh_windows();
                            });
                            return;
                        }
                        ContainerMountReview::ApprovedExternalHostPaths
                    } else {
                        ContainerMountReview::Unreviewed
                    };
                    self.start(&view, approval, cx).await;
                })
                .detach();
                cx.refresh_windows();
                true
            })
            .unwrap_or(false)
    }

    async fn start(
        self,
        view: &WeakEntity<ResourcesView>,
        review: ContainerMountReview,
        cx: &mut marsa_ui::gpui::AsyncApp,
    ) {
        let Self {
            services,
            endpoint,
            image,
            reference,
            name,
            options,
            pull_before,
            start_after,
        } = self;
        let docker = services.resource_docker(endpoint.clone());
        let mut operation = services.pending(async move {
            if pull_before
                && !reference.is_empty()
                && reference != image.as_str()
                && !reference.starts_with("sha256:")
            {
                let parsed = ImageReference::parse(reference.clone())?;
                let _ = docker.pull_image(&parsed).await?;
            }
            Ok(docker
                .run_image_configured(&image, name.as_ref(), &options, review, start_after)
                .await?)
        });
        let result = operation.result().await;
        let _ = view.update(cx, |view, cx| {
            view.busy = None;
            view.outcome = Some((
                ResourceKind::Images,
                match result {
                    Some(Ok(ImageRun::Started(_))) => {
                        Outcome::Success(SuccessKind::ContainerStarted)
                    }
                    Some(Ok(ImageRun::Created {
                        start_error: None, ..
                    })) => Outcome::Success(SuccessKind::ContainerCreated),
                    Some(Ok(ImageRun::Created {
                        start_error: Some(_),
                        ..
                    })) => Outcome::ImageCreated,
                    _ => Outcome::Rejected,
                },
            ));
            cx.emit(super::ExternalResourcesChanged);
            cx.refresh_windows();
        });
    }
}

impl ResourcesView {
    /// Open the full Create container dialog for a pinned image.
    pub(super) fn open_run_container(
        &mut self,
        kind: FormKind,
        source: Option<ResourceSource>,
        appearance: (Locale, Palette, marsa_ui::preferences::DisplaySettings),
        service: Arc<Services>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let FormKind::Run { id, reference } = kind else {
            return;
        };
        let Ok(image) = ImageId::parse(id.clone()) else {
            return;
        };
        if window.has_active_dialog(cx) || self.busy.is_some() {
            return;
        }
        let (locale, palette, _display) = appearance;
        let endpoint = source.as_ref().map(|source| source.endpoint.clone());
        let badge = source.as_ref().map_or_else(
            || SharedString::from("Marsa"),
            |source| source.badge.clone(),
        );
        let env_label = badge.clone();
        let image_label = if reference.is_empty() {
            isolate_technical_text(&id)
        } else {
            isolate_technical_text(&reference)
        };
        let options: DraftOptions =
            OptionsEditor::editable(&ContainerCreateOptions::default(), locale, window, cx);
        let failed = Rc::new(Cell::new(None::<&'static str>));
        let pull_before = Rc::new(Cell::new(false));
        let start_after = Rc::new(Cell::new(true));
        let name = field(
            gui::GUI_DESKTOP_CONTAINER_NAME_PLACEHOLDER.text(locale),
            "",
            window,
            cx,
        );
        let command = field(
            gui::GUI_DESKTOP_COMMAND_PLACEHOLDER.text(locale),
            "",
            window,
            cx,
        );
        let entrypoint = field(
            gui::GUI_DESKTOP_ENTRYPOINT_PLACEHOLDER.text(locale),
            "",
            window,
            cx,
        );
        let view = cx.entity().downgrade();
        let submit = {
            let name = name.clone();
            let command = command.clone();
            let entrypoint = entrypoint.clone();
            let options = options.clone();
            let failed = failed.clone();
            let pull_before = pull_before.clone();
            let start_after = start_after.clone();
            let badge = badge.clone();
            let reference = reference.clone();
            let services = service.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| -> bool {
                let name_value = name.read(cx).value();
                let parsed = (!name_value.trim().is_empty())
                    .then(|| ResourceName::parse(name_value.trim()))
                    .transpose();
                let Ok(name) = parsed else {
                    return false;
                };
                let mut options = match options.borrow().read(cx) {
                    Ok(options) => options,
                    Err(error) => {
                        failed.set(Some(error_text(error, locale)));
                        window.refresh();
                        return false;
                    }
                };
                options.command = split_argv(&command.read(cx).value());
                options.entrypoint = split_argv(&entrypoint.read(cx).value());
                if let Err(error) = options.validate() {
                    failed.set(Some(error_text(error, locale)));
                    window.refresh();
                    return false;
                }
                Launch {
                    services: services.clone(),
                    endpoint: endpoint.clone(),
                    image: image.clone(),
                    reference: reference.clone(),
                    name,
                    options,
                    pull_before: pull_before.get(),
                    start_after: start_after.get(),
                }
                .submit(&view, &badge, locale, window, cx)
            })
        };

        window.open_dialog(cx, move |dialog, _, cx| {
            let name_value = name.read(cx).value();
            let invalid =
                !name_value.trim().is_empty() && ResourceName::parse(name_value.trim()).is_err();
            let click_submit = submit.clone();
            let key_submit = submit.clone();

            // Dialog already scrolls its body — a nested scrollbar doubles the gutter.
            let body = div()
                .flex()
                .flex_col()
                .gap(px(Space::S16.pixels()))
                .text_start(cx)
                .child(dialog_scope(locale, palette, env_label.clone()))
                .child(image_banner(locale, palette, image_label.clone()))
                .child(basic_settings(
                    locale,
                    palette,
                    &name,
                    &command,
                    &entrypoint,
                    invalid,
                ))
                .child(toggle_row(
                    locale,
                    palette,
                    "pull-before-create",
                    gui::GUI_DESKTOP_PULL_BEFORE_CREATE.text(locale),
                    &pull_before,
                ))
                .child(toggle_row(
                    locale,
                    palette,
                    "start-after-create",
                    gui::GUI_DESKTOP_START_AFTER_CREATE.text(locale),
                    &start_after,
                ))
                .children(failed.get().map(|text| error_line(text, palette, cx)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(Space::S12.pixels()))
                        .border_t_1()
                        .border_color(color(palette, ColorToken::BorderSubtle))
                        .pt(px(Space::S16.pixels()))
                        .child(OptionsEditor::render(&options, locale, palette)),
                );

            let title = format!(
                "{} · {}",
                gui::GUI_DESKTOP_CREATE_CONTAINER.text(locale),
                env_label
            );

            dialog_frame(dialog, 760.)
                .overlay_closable(false)
                .title(dialog_heading(locale, palette, title))
                .child(body)
                .on_ok(move |_, window, cx| key_submit(window, cx))
                .footer(
                    dialog_actions(locale, palette)
                        .child(
                            Button::new("cancel-create-container")
                                .outline()
                                .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("confirm-create-container")
                                .primary()
                                .icon(IconName::Play)
                                .label(gui::GUI_DESKTOP_CREATE_CONTAINER.text(locale))
                                .disabled(invalid)
                                .on_click(move |_, window, cx| {
                                    if click_submit(window, cx) {
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
    }
}

fn image_banner(locale: Locale, palette: Palette, image: SharedString) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(12.))
        .border_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
        .bg(color(palette, ColorToken::BgSubtle))
        .child(
            row(locale)
                .gap_3()
                .items_center()
                .child(
                    Icon::default()
                        .path("icons/box.svg")
                        .text_color(color(palette, ColorToken::TextMuted)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(ui_size(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("{}:", gui::GUI_DESKTOP_IMAGE.text(locale))),
                        )
                        .child(
                            div()
                                .font_family("IBM Plex Mono")
                                .text_size(ui_size(12.))
                                .truncate()
                                .child(image),
                        ),
                ),
        )
}

fn basic_settings(
    locale: Locale,
    palette: Palette,
    name: &marsa_ui::gpui::Entity<InputState>,
    command: &marsa_ui::gpui::Entity<InputState>,
    entrypoint: &marsa_ui::gpui::Entity<InputState>,
    invalid: bool,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(Space::S12.pixels()))
        .child(
            div()
                .text_size(ui_size(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .pb_2()
                .border_b_1()
                .border_color(color(palette, ColorToken::BorderSubtle))
                .child(gui::GUI_DESKTOP_BASIC_SETTINGS.text(locale)),
        )
        .child(labeled(
            gui::GUI_DESKTOP_CONTAINER_NAME_REQUIRED.text(locale),
            resource_input(name).aria_label(gui::GUI_DESKTOP_CONTAINER_NAME_REQUIRED.text(locale)),
        ))
        .children(invalid.then(|| {
            div()
                .text_size(ui_size(12.))
                .text_color(color(palette, ColorToken::StatusError))
                .child(gui::GUI_DESKTOP_CONTAINERS_INVALID_NAME.text(locale))
        }))
        .child(labeled(
            gui::GUI_DESKTOP_COMMAND_OPTIONAL.text(locale),
            resource_input(command).aria_label(gui::GUI_DESKTOP_COMMAND_OPTIONAL.text(locale)),
        ))
        .child(labeled(
            gui::GUI_DESKTOP_ENTRYPOINT_OPTIONAL.text(locale),
            resource_input(entrypoint)
                .aria_label(gui::GUI_DESKTOP_ENTRYPOINT_OPTIONAL.text(locale)),
        ))
}

fn toggle_row(
    locale: Locale,
    palette: Palette,
    id: &'static str,
    label: &'static str,
    state: &Rc<Cell<bool>>,
) -> Div {
    let enabled = state.get();
    let toggle = state.clone();
    row(locale)
        .w_full()
        .items_center()
        .justify_between()
        .gap_3()
        .child(div().text_size(ui_size(12.)).child(label))
        .child(
            Button::new(id)
                .ghost()
                .compact()
                .icon(Icon::default().path(if enabled {
                    "icons/circle-check.svg"
                } else {
                    "icons/circle-off.svg"
                }))
                .label(if enabled {
                    gui::GUI_DESKTOP_TOGGLE_ON.text(locale)
                } else {
                    gui::GUI_DESKTOP_TOGGLE_OFF.text(locale)
                })
                .text_color(color(
                    palette,
                    if enabled {
                        ColorToken::StatusRunning
                    } else {
                        ColorToken::TextMuted
                    },
                ))
                .on_click(move |_, window, _| {
                    toggle.set(!toggle.get());
                    window.refresh();
                }),
        )
}

fn labeled(label: &'static str, field: impl marsa_ui::gpui::IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(Space::S4.pixels()))
        .child(
            div()
                .text_size(ui_size(12.))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
        .child(field)
}

fn split_argv(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}
