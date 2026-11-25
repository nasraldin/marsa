//! Images table: usage filter, sort, columns, multi-select, and row actions.

use std::{collections::BTreeSet, sync::Arc};

use marsa_ui::{
    ColorToken, Locale,
    gpui::{
        Context, Div, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
        div, px,
    },
    gpui_component::{
        Disableable, Icon, IconName, Placement,
        button::{Button, ButtonVariants as _},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
    tokens::{scale::Space, typography::Role},
};

use crate::{
    screens::{
        common::{ScreenContext, message, resource_input, row},
        table::{
            ColumnId, ColumnsPickerState, SelectionToolbar, SortField, SortMenuState, SortOrder,
            TableViewMode, UsageFilter, cell, default_flex, idle_toolbar, selection_toolbar,
            sortable_header, status_pill, table_row, table_shell, tag_badge,
        },
    },
    services::Services,
};

use super::{
    ResourceKind, ResourceSource, ResourcesView, forms::FormKind, rows::ResourceRow, text, tone,
};

/// Session-only Images table controls.
#[derive(Clone, Debug, Default)]
pub(super) struct ImagesTableState {
    pub usage: UsageFilter,
    pub sort: SortMenuState,
    pub columns: ColumnsPickerState,
    pub view: TableViewMode,
    pub selected: BTreeSet<String>,
}

impl ResourcesView {
    pub(super) fn clear_image_selection(&mut self, cx: &mut Context<Self>) {
        self.images.selected.clear();
        cx.notify();
    }

    pub(super) fn set_image_usage(&mut self, usage: UsageFilter, cx: &mut Context<Self>) {
        self.images.usage = usage;
        cx.notify();
    }

    pub(super) fn set_image_sort(&mut self, sort: SortMenuState, cx: &mut Context<Self>) {
        self.images.sort = sort;
        cx.notify();
    }

    pub(super) fn set_image_columns(
        &mut self,
        columns: ColumnsPickerState,
        cx: &mut Context<Self>,
    ) {
        self.images.columns = columns;
        cx.notify();
    }

    pub(super) fn set_image_view(&mut self, view: TableViewMode, cx: &mut Context<Self>) {
        self.images.view = view;
        cx.notify();
    }

    pub(super) fn reset_image_filters(&mut self, cx: &mut Context<Self>) {
        self.images = ImagesTableState::default();
        cx.notify();
    }

    pub(super) fn toggle_image_selection(
        &mut self,
        key: String,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if checked {
            self.images.selected.insert(key);
        } else {
            self.images.selected.remove(&key);
        }
        cx.notify();
    }

    pub(super) fn select_visible_images(
        &mut self,
        keys: &[String],
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if checked {
            self.images.selected.extend(keys.iter().cloned());
        } else {
            for key in keys {
                self.images.selected.remove(key);
            }
        }
        cx.notify();
    }
}

/// Compact trailing actions for the Images idle toolbar.
pub(super) fn action_rail(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    data: &ResourcesView,
    source: Option<ResourceSource>,
    available: bool,
) -> Div {
    let busy = data.busy.is_some();
    let disabled = screen.services.is_none() || !available || busy;
    let appearance = (screen.locale, screen.palette, screen.display);
    let service = screen.services.cloned();
    row(screen.locale)
        .items_center()
        .gap_1()
        .child(toolbar_icon(
            "refresh-images",
            "icons/refresh-cw.svg",
            message(screen.locale, gui::GUI_DESKTOP_REFRESH_IMAGES),
            disabled,
            {
                let view = view.downgrade();
                let service = service.clone();
                let external = source.is_some();
                move |_, _, cx| {
                    let Some(service) = service.clone() else {
                        return;
                    };
                    let _ = view.update(cx, |view, cx| {
                        if view.busy.is_some() {
                            return;
                        }
                        view.busy = Some(ResourceKind::Images);
                        view.outcome = None;
                        view.operation = Some(cx.spawn(async move |view, cx| {
                            if external {
                                let _ = view.update(cx, |view, cx| {
                                    view.busy = None;
                                    cx.emit(super::ExternalResourcesChanged);
                                    cx.refresh_windows();
                                });
                                return;
                            }
                            let _ = service.refresh().result().await;
                            let _ = view.update(cx, |view, cx| {
                                view.busy = None;
                                cx.refresh_windows();
                            });
                        }));
                        cx.refresh_windows();
                    });
                }
            },
        ))
        .child(toolbar_form_icon(
            screen,
            view,
            "pull-image",
            "icons/download.svg",
            message(screen.locale, gui::GUI_DESKTOP_PULL_IMAGE),
            disabled,
            FormKind::Pull,
            source.clone(),
            appearance,
            service.clone(),
        ))
        .child(toolbar_form_icon(
            screen,
            view,
            "load-image-tar",
            "icons/upload.svg",
            message(screen.locale, gui::GUI_DESKTOP_LOAD_IMAGE_FROM_TAR),
            disabled,
            FormKind::ImportImage,
            source.clone(),
            appearance,
            service.clone(),
        ))
        .child(prune_menu(
            screen, view, disabled, source, appearance, service,
        ))
}

fn toolbar_icon(
    id: &'static str,
    icon: &'static str,
    label: SharedString,
    disabled: bool,
    on_click: impl Fn(&marsa_ui::gpui::ClickEvent, &mut Window, &mut marsa_ui::gpui::App) + 'static,
) -> Button {
    Button::new(id)
        .outline()
        .size(px(Space::S36.pixels()))
        .p_0()
        .icon(Icon::default().path(icon))
        .accessibility_label(label.clone())
        .tooltip(label)
        .disabled(disabled)
        .on_click(on_click)
}

#[allow(clippy::too_many_arguments)]
fn toolbar_form_icon(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    id: &'static str,
    icon: &'static str,
    label: SharedString,
    disabled: bool,
    kind: FormKind,
    source: Option<ResourceSource>,
    appearance: (
        Locale,
        marsa_ui::Palette,
        marsa_ui::preferences::DisplaySettings,
    ),
    service: Option<Arc<Services>>,
) -> Button {
    let _ = screen;
    let state = view.clone();
    toolbar_icon(id, icon, label, disabled, move |_, window, cx| {
        if let Some(service) = &service {
            state.update(cx, |state, cx| {
                state.open_form(
                    (kind.clone(), source.clone()),
                    appearance,
                    Arc::clone(service),
                    window,
                    cx,
                );
            });
        }
    })
}

fn prune_menu(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    disabled: bool,
    source: Option<ResourceSource>,
    appearance: (
        Locale,
        marsa_ui::Palette,
        marsa_ui::preferences::DisplaySettings,
    ),
    service: Option<Arc<Services>>,
) -> impl IntoElement {
    let locale = screen.locale;
    let label = message(locale, gui::GUI_DESKTOP_PRUNE_IMAGES);
    let state = view.clone();
    Button::new("prune-images")
        .outline()
        .h(px(Space::S36.pixels()))
        .px_2()
        .disabled(disabled || service.is_none())
        .accessibility_label(label.clone())
        .tooltip(label.clone())
        .child(
            row(locale)
                .gap_1()
                .items_center()
                .child(Icon::default().path("icons/trash.svg").size(px(16.)))
                .child(Icon::new(IconName::ChevronDown).size(px(14.))),
        )
        .dropdown_menu(move |menu, _, _| {
            let unused = state.clone();
            let dangling = state.clone();
            let unused_source = source.clone();
            let dangling_source = source.clone();
            let unused_service = service.clone();
            let dangling_service = service.clone();
            menu.item(
                PopupMenuItem::new(message(locale, gui::GUI_DESKTOP_PRUNE_UNUSED_IMAGES)).on_click(
                    move |_, window, cx| {
                        if let Some(service) = &unused_service {
                            unused.update(cx, |view, cx| {
                                view.open_form(
                                    (
                                        FormKind::PruneImages { dangling: false },
                                        unused_source.clone(),
                                    ),
                                    appearance,
                                    Arc::clone(service),
                                    window,
                                    cx,
                                );
                            });
                        }
                    },
                ),
            )
            .item(
                PopupMenuItem::new(message(locale, gui::GUI_DESKTOP_PRUNE_DANGLING_IMAGES))
                    .on_click(move |_, window, cx| {
                        if let Some(service) = &dangling_service {
                            dangling.update(cx, |view, cx| {
                                view.open_form(
                                    (
                                        FormKind::PruneImages { dangling: true },
                                        dangling_source.clone(),
                                    ),
                                    appearance,
                                    Arc::clone(service),
                                    window,
                                    cx,
                                );
                            });
                        }
                    }),
            )
        })
}

pub(super) fn filter_and_sort<'a>(
    rows: &'a [ResourceRow],
    query: &str,
    usage: UsageFilter,
    sort: SortMenuState,
) -> Vec<&'a ResourceRow> {
    let mut rows = rows
        .iter()
        .filter(|row| row.matches(query))
        .filter(|row| {
            row.image
                .as_ref()
                .is_none_or(|meta| usage.matches(meta.usage))
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        let ordering = compare_rows(left, right, sort.field);
        match sort.order {
            SortOrder::Ascending => ordering,
            SortOrder::Descending => ordering.reverse(),
        }
    });
    rows
}

fn compare_rows(left: &ResourceRow, right: &ResourceRow, field: SortField) -> std::cmp::Ordering {
    let left_meta = left.image.as_ref();
    let right_meta = right.image.as_ref();
    match field {
        SortField::Name => left.display_name().cmp(right.display_name()),
        SortField::Tags => left_meta
            .map(|meta| meta.tag_count)
            .cmp(&right_meta.map(|meta| meta.tag_count)),
        SortField::Size => left_meta
            .map(|meta| meta.size)
            .cmp(&right_meta.map(|meta| meta.size)),
        SortField::Age => left_meta
            .map(|meta| meta.created)
            .cmp(&right_meta.map(|meta| meta.created)),
        SortField::State => left_meta
            .map(|meta| meta.usage)
            .cmp(&right_meta.map(|meta| meta.usage)),
    }
}

pub(super) fn render_table(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    data: &ResourcesView,
    rows: &[&ResourceRow],
    trailing: Div,
    show_sources: bool,
) -> Div {
    let selected = &data.images.selected;
    let visible_keys = rows
        .iter()
        .map(|row| row.key().to_owned())
        .collect::<Vec<_>>();
    let selected_visible = visible_keys
        .iter()
        .filter(|key| selected.contains(key.as_str()))
        .count();
    let selection = SelectionToolbar {
        selected: selected_visible,
        total: visible_keys.len(),
        select_all_checked: !visible_keys.is_empty()
            && visible_keys.iter().all(|key| selected.contains(key)),
        select_all_indeterminate: selected_visible > 0 && selected_visible < visible_keys.len(),
    };
    let search = div().w(px(224.)).child(
        resource_input(&data.filter)
            .cleanable(true)
            .prefix(IconName::Search)
            .aria_label(gui::GUI_DESKTOP_FILTER_RESOURCES.text(screen.locale)),
    );

    let toolbar = if selected_visible > 0 {
        let clear = view.downgrade();
        let bulk = bulk_remove_button(screen, view, rows, selected, data.busy.is_some());
        selection_toolbar(
            screen,
            selection,
            move |cx| {
                let _ = clear.update(cx, |view, cx| view.clear_image_selection(cx));
            },
            bulk,
        )
    } else {
        let usage_view = view.downgrade();
        let sort_view = view.downgrade();
        let columns_view = view.downgrade();
        let mode_view = view.downgrade();
        let reset_view = view.downgrade();
        idle_toolbar(
            screen,
            data.images.usage,
            data.images.sort,
            data.images.columns.clone(),
            data.images.view,
            search,
            trailing,
            move |usage, cx| {
                let _ = usage_view.update(cx, |view, cx| view.set_image_usage(usage, cx));
            },
            move |sort, cx| {
                let _ = sort_view.update(cx, |view, cx| view.set_image_sort(sort, cx));
            },
            move |columns, cx| {
                let _ = columns_view.update(cx, |view, cx| view.set_image_columns(columns, cx));
            },
            move |mode, cx| {
                let _ = mode_view.update(cx, |view, cx| view.set_image_view(mode, cx));
            },
            move |cx| {
                let _ = reset_view.update(cx, |view, cx| view.reset_image_filters(cx));
            },
        )
    };

    let columns = data.images.columns.visible_ids();
    let sort = data.images.sort;
    let sort_view = view.downgrade();
    let select_view = view.downgrade();
    let select_keys = visible_keys.clone();
    let header = row(screen.locale)
        .items_center()
        .w_full()
        .gap(px(Space::S8.pixels()))
        .px(px(Space::S12.pixels()))
        .py(px(Space::S8.pixels()))
        .child(
            marsa_ui::gpui_component::checkbox::Checkbox::new("select-all-images")
                .checked(selection.select_all_checked)
                .indeterminate(selection.select_all_indeterminate)
                .accessibility_label(message(screen.locale, gui::GUI_DESKTOP_SELECT_ALL_IMAGES))
                .on_click(move |checked, _, cx| {
                    let _ = select_view.update(cx, |view, cx| {
                        view.select_visible_images(&select_keys, *checked, cx);
                    });
                }),
        )
        .children(columns.iter().map(|&column| {
            let flex = default_flex(column);
            let sort_view = sort_view.clone();
            sortable_header(screen, column, sort, flex, move |next, cx| {
                let _ = sort_view.update(cx, |view, cx| view.set_image_sort(next, cx));
            })
        }));

    let body = match data.images.view {
        TableViewMode::Table => div().flex().flex_col().children(rows.iter().map(|entry| {
            image_table_row(screen, view, entry, selected, &columns, show_sources, data)
        })),
        TableViewMode::List => div().flex().flex_col().children(rows.iter().map(|entry| {
            image_list_row(screen, view, entry, selected, &columns, show_sources, data)
        })),
    };

    table_shell(screen, toolbar, header, body, "images-table-rows")
}

fn bulk_remove_button(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    rows: &[&ResourceRow],
    selected: &BTreeSet<String>,
    busy: bool,
) -> Div {
    let items = rows
        .iter()
        .filter(|row| selected.contains(row.key()))
        .map(|row| (row.id.clone(), row.action_target.clone(), row.users.clone()))
        .collect::<Vec<_>>();
    let source = rows
        .iter()
        .find(|row| selected.contains(row.key()))
        .and_then(|row| row.source().cloned());
    let state = view.clone();
    let appearance = (screen.locale, screen.palette, screen.display);
    let service = screen.services.cloned();
    let empty = items.is_empty();
    row(screen.locale).child(
        Button::new("bulk-remove-images")
            .danger()
            .h(px(Space::S36.pixels()))
            .icon(Icon::default().path("icons/trash.svg"))
            .label(message(screen.locale, gui::GUI_DESKTOP_REMOVE))
            .disabled(service.is_none() || busy || empty)
            .on_click(move |_, window, cx| {
                let Some(service) = &service else {
                    return;
                };
                let kind = if items.len() == 1 {
                    let (id, reference, users) = items[0].clone();
                    FormKind::RemoveImage {
                        id,
                        reference,
                        users,
                    }
                } else {
                    FormKind::RemoveImages {
                        items: items.clone(),
                    }
                };
                state.update(cx, |state, cx| {
                    state.images.selected.clear();
                    state.open_form(
                        (kind, source.clone()),
                        appearance,
                        Arc::clone(service),
                        window,
                        cx,
                    );
                });
            }),
    )
}

fn image_table_row(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    entry: &ResourceRow,
    selected: &BTreeSet<String>,
    columns: &[ColumnId],
    show_sources: bool,
    data: &ResourcesView,
) -> impl IntoElement {
    let key = entry.key().to_owned();
    let checked = selected.contains(&key);
    let is_selected = data.selection.as_ref().is_some_and(|(kind, selected_key)| {
        *kind == ResourceKind::Images
            && entry.selected(selected_key)
            && entry.endpoint() == data.selected_endpoint.as_ref()
    });
    let toggle_view = view.downgrade();
    let open_view = view.downgrade();
    let open_entry = entry.clone();
    let open_service = screen.services.cloned();
    let appearance = (screen.locale, screen.palette, screen.display);
    let cells = columns
        .iter()
        .map(|&column| {
            cell(
                default_flex(column),
                image_cell(screen, view, entry, column, show_sources, data),
            )
        })
        .collect::<Vec<_>>();
    table_row(
        screen,
        key.clone(),
        is_selected,
        checked,
        message(screen.locale, gui::GUI_DESKTOP_SELECT_IMAGE),
        move |checked, cx| {
            let _ = toggle_view.update(cx, |view, cx| {
                view.toggle_image_selection(key.clone(), checked, cx);
            });
        },
        move |window, cx| {
            let Some(service) = open_service.clone() else {
                return;
            };
            let _ = open_view.update(cx, |view, cx| {
                view.open_image_detail(&open_entry, appearance, service, window, cx);
            });
        },
        cells,
    )
}

fn image_list_row(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    entry: &ResourceRow,
    selected: &BTreeSet<String>,
    columns: &[ColumnId],
    show_sources: bool,
    data: &ResourcesView,
) -> impl IntoElement {
    let meta_bits = columns
        .iter()
        .copied()
        .filter(|column| !matches!(column, ColumnId::Image | ColumnId::Actions))
        .filter_map(|column| list_meta(screen, entry, column))
        .collect::<Vec<_>>();
    let subtitle = meta_bits.join(" · ");
    let show_actions = columns.contains(&ColumnId::Actions);
    let body = row(screen.locale)
        .w_full()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(image_identity(screen, entry, show_sources))
                .child(
                    text(screen, Role::Small, subtitle)
                        .text_color(tone(screen, ColorToken::TextMuted))
                        .truncate(),
                ),
        )
        .children(show_actions.then(|| actions_cell(screen, view, entry, data)));
    let key = entry.key().to_owned();
    let checked = selected.contains(&key);
    let is_selected = data.selection.as_ref().is_some_and(|(kind, selected_key)| {
        *kind == ResourceKind::Images
            && entry.selected(selected_key)
            && entry.endpoint() == data.selected_endpoint.as_ref()
    });
    let toggle_view = view.downgrade();
    let open_view = view.downgrade();
    let open_entry = entry.clone();
    let open_service = screen.services.cloned();
    let appearance = (screen.locale, screen.palette, screen.display);
    table_row(
        screen,
        key.clone(),
        is_selected,
        checked,
        message(screen.locale, gui::GUI_DESKTOP_SELECT_IMAGE),
        move |checked, cx| {
            let _ = toggle_view.update(cx, |view, cx| {
                view.toggle_image_selection(key.clone(), checked, cx);
            });
        },
        move |window, cx| {
            let Some(service) = open_service.clone() else {
                return;
            };
            let _ = open_view.update(cx, |view, cx| {
                view.open_image_detail(&open_entry, appearance, service, window, cx);
            });
        },
        [div().flex_1().min_w_0().child(body)],
    )
}

fn list_meta(screen: &ScreenContext<'_>, entry: &ResourceRow, column: ColumnId) -> Option<String> {
    let meta = entry.image.as_ref()?;
    Some(match column {
        ColumnId::Tags => meta.tag_count.to_string(),
        ColumnId::Size => entry.cells.get(2).cloned().unwrap_or_default(),
        ColumnId::Age => entry.cells.get(3).cloned().unwrap_or_default(),
        ColumnId::State => meta.usage.label(screen.locale).to_string(),
        ColumnId::Image | ColumnId::Actions => return None,
    })
}

fn image_cell(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    entry: &ResourceRow,
    column: ColumnId,
    show_sources: bool,
    data: &ResourcesView,
) -> Div {
    match column {
        ColumnId::Image => image_identity(screen, entry, show_sources),
        ColumnId::Tags => tags_cell(screen, entry),
        ColumnId::Size => muted_text(screen, entry.cells.get(2).cloned().unwrap_or_default()),
        ColumnId::Age => age_cell(screen, entry),
        ColumnId::State => {
            let meta = entry.image.as_ref();
            let (fg, bg) = meta
                .map(|meta| meta.usage.tone())
                .unwrap_or((ColorToken::StatusStopped, ColorToken::StatusStoppedBg));
            let label = meta
                .map(|meta| meta.usage.label(screen.locale))
                .unwrap_or_else(|| message(screen.locale, gui::GUI_DESKTOP_UNUSED));
            status_pill(screen, label, fg, bg)
        }
        ColumnId::Actions => actions_cell(screen, view, entry, data),
    }
}

fn age_cell(screen: &ScreenContext<'_>, entry: &ResourceRow) -> Div {
    let relative = entry.cells.get(3).cloned().unwrap_or_default();
    let absolute = entry
        .image
        .as_ref()
        .and_then(|meta| super::rows::format_created_timestamp(meta.created))
        .unwrap_or_else(|| relative.clone());
    let key = SharedString::from(entry.key().to_owned());
    // Ghost button tooltip above the cell; custom child keeps the date start-aligned.
    div().w_full().child(
        Button::new((marsa_ui::gpui::ElementId::from("image-age"), key))
            .ghost()
            .compact()
            .p_0()
            .w_full()
            .tooltip(absolute)
            .tooltip_placement(Placement::Top)
            .child(
                div()
                    .w_full()
                    .text_color(tone(screen, ColorToken::TextMuted))
                    .child(text(screen, Role::Small, relative)),
            ),
    )
}

fn image_identity(screen: &ScreenContext<'_>, entry: &ResourceRow, show_sources: bool) -> Div {
    row(screen.locale)
        .gap_2()
        .items_center()
        .min_w_0()
        .child(
            text(screen, Role::Small, entry.display_name().to_owned())
                .font_family("IBM Plex Mono")
                .flex_1()
                .min_w_0()
                .truncate(),
        )
        .children(show_sources.then(|| {
            crate::screens::external_inventory::source_badge(
                screen.palette,
                entry
                    .source()
                    .map_or_else(|| "Marsa".into(), |source| source.badge.clone()),
            )
        }))
}

fn tags_cell(screen: &ScreenContext<'_>, entry: &ResourceRow) -> Div {
    let Some(meta) = entry.image.as_ref() else {
        return muted_text(screen, "0".into());
    };
    let count = SharedString::from(meta.tag_count.to_string());
    row(screen.locale)
        .gap_1()
        .items_center()
        .min_w_0()
        .child(tag_badge(
            screen,
            count,
            ColorToken::TextPrimary,
            ColorToken::BgSubtle,
        ))
        .children(meta.in_use_tag.as_ref().map(|tag| {
            tag_badge(
                screen,
                SharedString::from(tag.clone()),
                ColorToken::StatusRunning,
                ColorToken::StatusRunningBg,
            )
        }))
}

fn muted_text(screen: &ScreenContext<'_>, value: String) -> Div {
    text(screen, Role::Small, value)
        .text_color(tone(screen, ColorToken::TextMuted))
        .truncate()
}

/// Parent-table actions: run · push · delete. Layers live in the details modal.
fn actions_cell(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    entry: &ResourceRow,
    data: &ResourcesView,
) -> Div {
    let busy = data.busy.is_some();
    let appearance = (screen.locale, screen.palette, screen.display);
    let service = screen.services.cloned();
    let source = entry.source().cloned();
    let run = FormKind::Run {
        id: entry.id.clone(),
        reference: entry.action_target.clone(),
    };
    let push = FormKind::PushImage {
        id: entry.id.clone(),
        reference: entry.action_target.clone(),
    };
    let remove = FormKind::RemoveImage {
        id: entry.id.clone(),
        reference: entry.action_target.clone(),
        users: entry.users.clone(),
    };
    let can_push = !entry.action_target.is_empty()
        && entry.action_target != entry.id
        && !entry.action_target.contains('@');
    let can_run = !entry.action_target.is_empty();
    row(screen.locale)
        .gap_1()
        .items_center()
        .child(action_icon(
            entry,
            "run",
            "icons/play.svg",
            message(screen.locale, gui::GUI_DESKTOP_RUN_IMAGE),
            service.is_none() || busy || !can_run,
            view,
            run,
            source.clone(),
            appearance,
            service.clone(),
        ))
        .child(action_icon(
            entry,
            "push",
            "icons/upload.svg",
            message(screen.locale, gui::GUI_DESKTOP_PUSH_IMAGE),
            service.is_none() || busy || !can_push,
            view,
            push,
            source.clone(),
            appearance,
            service.clone(),
        ))
        .child(action_icon(
            entry,
            "remove",
            "icons/trash.svg",
            message(screen.locale, gui::GUI_DESKTOP_REMOVE_IMAGE),
            service.is_none() || busy,
            view,
            remove,
            source,
            appearance,
            service,
        ))
}

#[allow(clippy::too_many_arguments)]
fn action_icon(
    entry: &ResourceRow,
    id: &'static str,
    icon: &'static str,
    label: SharedString,
    disabled: bool,
    view: &Entity<ResourcesView>,
    kind: FormKind,
    source: Option<ResourceSource>,
    appearance: (
        Locale,
        marsa_ui::Palette,
        marsa_ui::preferences::DisplaySettings,
    ),
    service: Option<Arc<Services>>,
) -> Button {
    let state = view.clone();
    let row_key = SharedString::from(format!("{id}:{}", entry.key()));
    Button::new(row_key)
        .ghost()
        .size(px(28.))
        .p_0()
        .icon(Icon::default().path(icon))
        .accessibility_label(label.clone())
        .tooltip(label)
        .disabled(disabled)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            if let Some(service) = &service {
                state.update(cx, |state, cx| {
                    state.open_form(
                        (kind.clone(), source.clone()),
                        appearance,
                        Arc::clone(service),
                        window,
                        cx,
                    );
                });
            }
        })
}
