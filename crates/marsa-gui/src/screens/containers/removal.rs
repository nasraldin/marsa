//! Explicit confirmation for stopped and running container removal.

use marsa_ui::gui_keys as gui;
use std::{cell::Cell, rc::Rc};

use marsa_client::docker::{ContainerId, ContainerRemoval, RemovalMode};
use marsa_ui::{
    ColorToken, Locale,
    direction::{LogicalStyle as _, isolate_technical_text, row},
    gpui::{
        Context, InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _,
        Styled as _, Window, div, px,
    },
    gpui_component::{
        Disableable as _, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
    },
};

use crate::services::Command;

use super::{ContainerView, color, list, name};

struct Target {
    id: ContainerId,
    name: String,
    state: String,
    requires_force: bool,
}

pub(super) fn open(
    view: &ContainerView,
    ids: Vec<ContainerId>,
    window: &mut Window,
    cx: &mut Context<ContainerView>,
) {
    let Some(services) = view.services.clone() else {
        return;
    };
    if view.command_pending || window.has_active_dialog(cx) {
        return;
    }
    let targets: Rc<Vec<_>> = Rc::new(
        ids.into_iter()
            .filter_map(|id| {
                let entry = view.state.container(&id)?;
                Some(Target {
                    id,
                    name: name(entry).to_owned(),
                    state: entry.state.clone(),
                    requires_force: !matches!(entry.state.as_str(), "created" | "exited" | "dead"),
                })
            })
            .collect(),
    );
    if targets.is_empty() {
        return;
    }
    let force_required = targets.iter().any(|target| target.requires_force);
    let force = Rc::new(Cell::new(false));
    let locale = view.locale;
    let palette = view.palette;
    let title = match locale {
        Locale::Arabic => format!("هل تريد إزالة {} من الحاويات؟", targets.len()),
        _ if targets.len() == 1 => "Remove 1 container?".to_owned(),
        _ => format!("Remove {} containers?", targets.len()),
    };
    let action = match locale {
        Locale::Arabic => format!("إزالة {} من الحاويات", targets.len()),
        _ if targets.len() == 1 => "Remove 1 container".to_owned(),
        _ => format!("Remove {} containers", targets.len()),
    };
    let view = cx.entity().downgrade();
    window.open_dialog(cx, move |dialog, _, cx| {
        let targets_for_action = targets.clone();
        let force_for_action = force.clone();
        let services = services.clone();
        let pending_view = view.clone();
        let submit = Rc::new(move |cx: &mut marsa_ui::gpui::App| {
            if force_required && !force_for_action.get() {
                return false;
            }
            pending_view
                .update(cx, |view, cx| {
                    if view.command_pending {
                        return false;
                    }
                    let commands = targets_for_action
                        .iter()
                        .map(|target| {
                            let mode = if target.requires_force {
                                RemovalMode::Force
                            } else {
                                RemovalMode::StoppedOnly
                            };
                            Command::RemoveContainer(ContainerRemoval::confirmed(
                                target.id.clone(),
                                mode,
                            ))
                        })
                        .collect();
                    view.command_pending = true;
                    view.command_failed = false;
                    view.run_commands(services.clone(), commands, cx);
                    true
                })
                .unwrap_or(false)
        });
        let click_submit = submit.clone();
        let selected_force = force.clone();
        let targets_list = div()
            .id("removal-targets")
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .max_h(px(184.))
            .overflow_y_scroll()
            .rounded_md()
            .bg(color(palette, ColorToken::BgSubtle))
            .children(targets.iter().map(|target| {
                row(cx)
                    .gap_3()
                    .items_center()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family("IBM Plex Mono")
                            .truncate()
                            .child(isolate_technical_text(&target.name)),
                    )
                    .child(list::state_pill(&target.state, palette, locale, cx))
            }));
        let description = if force_required {
            gui::GUI_DESKTOP_SOME_CONTAINERS_ARE_RUNNING_REVIEW_THE_LIST_BEFORE_YOU_CONTINU
                .text(locale)
        } else {
            gui::GUI_DESKTOP_REVIEW_THE_CONTAINERS_BEFORE_REMOVING_THEM.text(locale)
        };
        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            .text_start(cx)
            .child(description)
            .child(targets_list)
            .child(gui::GUI_DESKTOP_NAMED_VOLUMES_ARE_KEPT.text(locale));
        if force_required {
            content = content
                .child(
                    Checkbox::new("force-remove-containers")
                        .checked(force.get())
                        .label(gui::GUI_DESKTOP_FORCE_REMOVE_RUNNING_CONTAINERS.text(locale))
                        .on_click(move |checked, window, _| {
                            selected_force.set(*checked);
                            window.refresh();
                        }),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(palette, ColorToken::TextMuted))
                        .child(
                            gui::GUI_DESKTOP_STOPS_THE_AFFECTED_CONTAINERS_IMMEDIATELY.text(locale),
                        ),
                );
        }
        dialog
            .centered()
            .width(px(610.))
            .title(title.clone())
            .child(content)
            .on_ok(move |_, _, cx| submit(cx))
            .footer(
                row(cx)
                    .justify_row_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-container-removal")
                            .outline()
                            .small()
                            .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("confirm-container-removal")
                            .danger()
                            .small()
                            .label(action.clone())
                            .disabled(force_required && !force.get())
                            .on_click(move |_, window, cx| {
                                if click_submit(cx) {
                                    window.close_dialog(cx);
                                }
                            }),
                    ),
            )
    });
}
