//! Launch a container from an existing image on the pinned environment.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use marsa_client::docker::{
    ContainerMountReview, ExternalEndpoint, ImageId, ImageRun, ResourceName,
    container_config::{ConfigMountKind, ContainerCreateOptions},
};
use marsa_ui::{
    ColorToken, Locale,
    direction::{LogicalStyle as _, isolate_technical_text, row},
    gpui::{
        App, Context, InteractiveElement as _, ParentElement as _, PromptButton, PromptLevel,
        SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, px,
    },
    gpui_component::{
        Disableable as _, IconName, WindowExt as _,
        button::{Button, ButtonVariants as _},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
};

use crate::{
    screens::{
        common::resource_input,
        config_options::{OptionsEditor, error_line, error_text, field},
    },
    services::Services,
};

use super::{ContainerView, color};

struct Launch {
    services: Arc<Services>,
    endpoint: Option<ExternalEndpoint>,
    image: ImageId,
    name: Option<ResourceName>,
    options: ContainerCreateOptions,
}

impl Launch {
    fn submit(
        self,
        owner: &WeakEntity<ContainerView>,
        badge: &SharedString,
        locale: Locale,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        owner
            .update(cx, |view, cx| {
                if view.command_pending {
                    return false;
                }
                view.command_pending = true;
                view.command_failed = false;
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
                                PromptButton::Ok(gui::GUI_DESKTOP_RUN_IMAGE.text(locale).into()),
                                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
                            ],
                            cx,
                        )
                    });
                cx.spawn(async move |view, cx| {
                    let approval = if let Some(answer) = review {
                        if answer.await != Ok(0) {
                            let _ = view.update(cx, |view, cx| {
                                view.command_pending = false;
                                cx.notify();
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
                cx.notify();
                true
            })
            .unwrap_or(false)
    }

    async fn start(
        self,
        view: &WeakEntity<ContainerView>,
        review: ContainerMountReview,
        cx: &mut marsa_ui::gpui::AsyncApp,
    ) {
        let Self {
            services,
            endpoint,
            image,
            name,
            options,
        } = self;
        let docker = services.resource_docker(endpoint.clone());
        let mut launch = services.pending(async move {
            Ok(docker
                .run_image_configured(&image, name.as_ref(), &options, review)
                .await?)
        });
        let result = launch.result().await;
        let replacement = if let Some(endpoint) = &endpoint {
            let mut pending = services.external_containers(endpoint.clone());
            pending.result().await.and_then(Result::ok)
        } else {
            let _ = services.refresh().result().await;
            None
        };
        let _ = view.update(cx, |view, cx| {
            if let (Some(endpoint), Some(containers)) = (endpoint.as_ref(), replacement) {
                view.replace_external_containers(endpoint, containers);
            }
            view.command_pending = false;
            view.command_failed = !matches!(result, Some(Ok(ImageRun::Started(_))));
            cx.notify();
        });
    }
}

pub(super) fn open(view: &ContainerView, window: &mut Window, cx: &mut Context<ContainerView>) {
    let Some(services) = view.services.clone() else {
        return;
    };
    if view.command_pending || window.has_active_dialog(cx) {
        return;
    }
    let locale = view.locale;
    let palette = view.palette;
    let filter = view.environment.read(cx);
    let source = view
        .external_inventory
        .iter()
        .find(|workspace| filter.includes_external(workspace));
    let source = if filter.includes_marsa() {
        None
    } else {
        source.filter(|workspace| workspace.reachable).cloned()
    };
    if !filter.includes_marsa() && source.is_none() {
        return;
    }
    let endpoint = source.as_ref().map(|source| source.endpoint.clone());
    let badge = source.as_ref().map_or_else(
        || "Marsa".into(),
        |source| isolate_technical_text(&source.source),
    );
    let snapshot = source
        .as_ref()
        .map_or(&view.state, |workspace| &workspace.state);
    let entries = snapshot
        .images()
        .values()
        .filter_map(|entry| {
            let id = ImageId::parse(entry.id.clone()).ok()?;
            let display = entry
                .repo_tags
                .iter()
                .find(|tag| tag.as_str() != "<none>:<none>")
                .map(String::as_str)
                .unwrap_or(&entry.id);
            Some((id, isolate_technical_text(display)))
        })
        .collect::<Vec<_>>();
    let entries = Rc::new(entries);
    let selected = Rc::new(RefCell::new(None::<(ImageId, SharedString)>));
    let sets = Rc::new(view.config_sets.clone());
    let set_index = Rc::new(Cell::new(None::<usize>));
    let options = Rc::new(RefCell::new(OptionsEditor::editable(
        &ContainerCreateOptions::default(),
        locale,
        window,
        cx,
    )));
    let show_options = Rc::new(Cell::new(false));
    let failed = Rc::new(Cell::new(None::<&'static str>));
    let name = field(
        gui::GUI_DESKTOP_CONTAINER_NAME_OPTIONAL.text(locale),
        "",
        window,
        cx,
    );
    let view = cx.entity().downgrade();
    let submit = {
        let selected = selected.clone();
        let name = name.clone();
        let options = options.clone();
        let show_options = show_options.clone();
        let failed = failed.clone();
        let badge = badge.clone();
        Rc::new(move |window: &mut Window, cx: &mut App| {
            let Some((image, _)) = selected.borrow().clone() else {
                return false;
            };
            let name = name.read(cx).value();
            let parsed = (!name.trim().is_empty())
                .then(|| ResourceName::parse(name.trim()))
                .transpose();
            let Ok(name) = parsed else {
                return false;
            };
            let options = match options.borrow().borrow().read(cx) {
                Ok(options) => options,
                Err(error) => {
                    failed.set(Some(error_text(error, locale)));
                    show_options.set(true);
                    window.refresh();
                    return false;
                }
            };
            Launch {
                services: services.clone(),
                endpoint: endpoint.clone(),
                image,
                name,
                options,
            }
            .submit(&view, &badge, locale, window, cx)
        })
    };
    window.open_dialog(cx, move |dialog, _, cx| {
        let value = name.read(cx).value();
        let invalid = !value.trim().is_empty() && ResourceName::parse(value.trim()).is_err();
        let can_submit = selected.borrow().is_some() && !invalid;
        let click_submit = submit.clone();
        let key_submit = submit.clone();
        let mut body = div()
            .id("create-container-options")
            .max_h(px(460.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .px_1()
            .text_start(cx)
            .child(
                row(cx).child(super::super::external_inventory::source_badge(
                    palette,
                    badge.clone(),
                )),
            );
        if entries.is_empty() {
            body = body.child(gui::GUI_DESKTOP_CONTAINERS_NEED_IMAGE.text(locale));
        } else {
            let choices = entries.clone();
            let selection = selected.clone();
            let label = selected
                .borrow()
                .as_ref()
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| gui::GUI_DESKTOP_CONTAINERS_CHOOSE_IMAGE.text(locale).into());
            body = body
                .child(
                    Button::new("choose-new-container-image")
                        .outline()
                        .icon(IconName::ChevronDown)
                        .label(label)
                        .dropdown_menu(move |menu, _, _| {
                            choices.iter().fold(menu, |menu, (id, name)| {
                                let selection = selection.clone();
                                let chosen = (id.clone(), name.clone());
                                menu.item(PopupMenuItem::new(name.clone()).on_click(
                                    move |_, window, _| {
                                        *selection.borrow_mut() = Some(chosen.clone());
                                        window.refresh();
                                    },
                                ))
                            })
                        }),
                )
                .child(resource_input(&name));
            if invalid {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(color(palette, ColorToken::StatusError))
                        .child(gui::GUI_DESKTOP_CONTAINERS_INVALID_NAME.text(locale)),
                );
            }
            let enable_options = show_options.clone();
            if !sets.is_empty() {
                let current = set_index.get();
                let options = options.clone();
                let sets = sets.clone();
                let selected_set = set_index.clone();
                let show_options = show_options.clone();
                let label = current
                    .and_then(|index| sets.get(index))
                    .map(|set| isolate_technical_text(&set.name))
                    .unwrap_or_else(|| gui::GUI_CONFIG_CHOOSE_SET.text(locale).into());
                body = body.child(
                    Button::new("choose-container-config-set")
                        .outline()
                        .label(label)
                        .icon(IconName::ChevronDown)
                        .accessibility_label(gui::GUI_CONFIG_CHOOSE_SET.text(locale))
                        .dropdown_menu(move |menu, _, _| {
                            std::iter::once((None, gui::GUI_CONFIG_NO_SET.text(locale).into()))
                                .chain(sets.iter().enumerate().map(|(index, set)| {
                                    (Some(index), isolate_technical_text(&set.name))
                                }))
                                .fold(menu, |menu, (index, label)| {
                                    let options = options.clone();
                                    let sets = sets.clone();
                                    let selected_set = selected_set.clone();
                                    let show_options = show_options.clone();
                                    menu.item(
                                        PopupMenuItem::new(label)
                                            .checked(index == current)
                                            .on_click(move |_, window, cx| {
                                                selected_set.set(index);
                                                let defaults = ContainerCreateOptions::default();
                                                let selected = index
                                                    .and_then(|index| sets.get(index))
                                                    .map_or(&defaults, |set| &set.options);
                                                *options.borrow_mut() = OptionsEditor::editable(
                                                    selected, locale, window, cx,
                                                );
                                                show_options.set(index.is_some());
                                                window.refresh();
                                            }),
                                    )
                                })
                        }),
                );
            }
            body = body
                .child(
                    Button::new("edit-container-options")
                        .ghost()
                        .label(if show_options.get() {
                            gui::GUI_CONFIG_HIDE_OPTIONS.text(locale)
                        } else {
                            gui::GUI_CONFIG_EDIT_OPTIONS.text(locale)
                        })
                        .on_click(move |_, window, _| {
                            enable_options.set(!enable_options.get());
                            window.refresh();
                        }),
                )
                .children(failed.get().map(|text| error_line(text, palette, cx)))
                .children(
                    show_options
                        .get()
                        .then(|| OptionsEditor::render(&options.borrow(), locale, palette, cx)),
                );
        }
        dialog
            .centered()
            .width(px(600.))
            .title(gui::GUI_DESKTOP_NEW_CONTAINER.text(locale))
            .child(body)
            .on_ok(move |_, window, cx| key_submit(window, cx))
            .footer(
                row(cx)
                    .justify_row_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-create-container")
                            .outline()
                            .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("run-new-container")
                            .primary()
                            .label(gui::GUI_DESKTOP_RUN_IMAGE.text(locale))
                            .disabled(!can_submit)
                            .on_click(move |_, window, cx| {
                                if click_submit(window, cx) {
                                    window.close_dialog(cx);
                                }
                            }),
                    ),
            )
    });
}
