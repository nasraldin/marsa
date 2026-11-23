//! Launch a container from a local image selected in the Containers workspace.

use std::{cell::RefCell, rc::Rc};

use marsa_client::docker::{ImageId, ImageRun, ResourceName};
use marsa_ui::{
    ColorToken,
    direction::{LogicalStyle as _, isolate_technical_text, row},
    gpui::{
        App, AppContext as _, Context, ParentElement as _, SharedString, Styled as _, Window, div,
        px,
    },
    gpui_component::{
        Disableable as _, IconName, WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::{InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
};

use super::{ContainerView, color};

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
    let name = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(gui::GUI_DESKTOP_CONTAINER_NAME_OPTIONAL.text(locale))
    });
    cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.refresh_windows())
        .detach();
    let view = cx.entity().downgrade();
    window.open_dialog(cx, move |dialog, _, cx| {
        let value = name.read(cx).value();
        let typed = value.trim();
        let parsed = (!typed.is_empty()).then(|| ResourceName::parse(typed));
        let invalid = parsed.as_ref().is_some_and(Result::is_err);
        let selected_value = selected.borrow().clone();
        let can_submit = selected_value.is_some() && !invalid;
        let owner = view.clone();
        let services = services.clone();
        let endpoint = endpoint.clone();
        let submit = Rc::new(move |cx: &mut App| {
            let Some((id, _)) = selected_value.clone().filter(|_| can_submit) else {
                return false;
            };
            let name = parsed
                .as_ref()
                .and_then(|parsed| parsed.as_ref().ok())
                .cloned();
            let services = services.clone();
            owner
                .update(cx, |view, cx| {
                    if view.command_pending {
                        return false;
                    }
                    let endpoint = endpoint.clone();
                    let docker = services.resource_docker(endpoint.clone());
                    view.command_pending = true;
                    view.command_failed = false;
                    cx.spawn(async move |view, cx| {
                        let mut launch = services.pending(async move {
                            Ok(docker.run_image(&id, name.as_ref()).await?)
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
                            if let (Some(endpoint), Some(containers)) =
                                (endpoint.as_ref(), replacement)
                            {
                                view.replace_external_containers(endpoint, containers);
                            }
                            view.command_pending = false;
                            view.command_failed = !matches!(result, Some(Ok(ImageRun::Started(_))));
                            cx.notify();
                        });
                    })
                    .detach();
                    cx.notify();
                    true
                })
                .unwrap_or(false)
        });
        let click_submit = submit.clone();
        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
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
            let image_label = selected
                .borrow()
                .as_ref()
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| gui::GUI_DESKTOP_CONTAINERS_CHOOSE_IMAGE.text(locale).into());
            body = body
                .child(
                    Button::new("choose-new-container-image")
                        .outline()
                        .icon(IconName::ChevronDown)
                        .label(image_label)
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
                .child(super::super::common::resource_input(&name));
            if invalid {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(color(palette, ColorToken::StatusError))
                        .child(gui::GUI_DESKTOP_CONTAINERS_INVALID_NAME.text(locale)),
                );
            }
        }
        dialog
            .centered()
            .width(px(480.))
            .title(gui::GUI_DESKTOP_NEW_CONTAINER.text(locale))
            .child(body)
            .on_ok(move |_, _, cx| submit(cx))
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
                                if click_submit(cx) {
                                    window.close_dialog(cx);
                                }
                            }),
                    ),
            )
    });
}
