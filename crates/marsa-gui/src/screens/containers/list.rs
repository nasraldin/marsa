//! Searchable, virtualized container table and selection controls.

use crate::screens::common::message;
use marsa_ui::gpui_component::scroll::ScrollableElement as _;
use marsa_ui::{gui_keys as gui, tokens::typography::ui_size};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use marsa_client::docker::{ContainerAction, ContainerId, ContainerSummary};
use marsa_ui::{
    ColorToken, Locale, Palette,
    direction::{LogicalStyle as _, isolate_technical_text, row},
    gpui::{
        AnyElement, App, Context, Div, ElementId, InteractiveElement as _, IntoElement,
        ParentElement as _, SharedString, Styled as _, Window, div, px, uniform_list,
    },
    gpui_component::{
        Disableable as _, Icon, IconName, Sizable as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    tokens::{scale::Space, typography::Role},
};

use crate::screens::common::{ScreenContext, copy, label, resource_input};

use super::{
    ContainerTarget, ContainerView, StatusFilter, action_label, color, eligible, name, state_label,
    states,
};

const ROW_HEIGHT: f32 = 43.;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ViewMode {
    #[default]
    Table,
    List,
    Cards,
    Tree,
}

pub(super) type GroupKey = (Option<String>, Option<String>);

#[derive(Clone)]
struct RowSource {
    key: String,
    badge: SharedString,
    endpoint: marsa_client::docker::ExternalEndpoint,
}

#[derive(Clone)]
enum TableRow {
    Group {
        key: GroupKey,
        count: usize,
        source: Option<Arc<RowSource>>,
    },
    Container(Box<ContainerSummary>),
    External(Box<ContainerSummary>, Arc<RowSource>),
}

pub(super) fn render(
    view: &ContainerView,
    window: &Window,
    cx: &mut Context<ContainerView>,
) -> AnyElement {
    let screen = view.screen();
    let locale = screen.locale;
    let palette = screen.palette;
    let mode = view.mode;
    let compact = window.viewport_size().width < px(1120.);
    let card_columns = if compact { 2 } else { 3 };
    let filter = view.environment.read(cx);
    let mut rows = if filter.includes_marsa() {
        collect_rows(view, &view.state.containers(), None)
    } else {
        Vec::new()
    };
    let local_status = filter
        .includes_marsa()
        .then(|| states::TableState::from(view, !rows.is_empty()));
    if local_status != Some(states::TableState::Rows) {
        rows.clear();
    }
    for (index, inventory) in view.external_inventory.iter().enumerate() {
        if !filter.includes_external(inventory) {
            continue;
        }
        let source = Arc::new(RowSource {
            key: format!("{index}:{}", inventory.source),
            badge: isolate_technical_text(&inventory.source),
            endpoint: inventory.endpoint.clone(),
        });
        rows.extend(collect_rows(
            view,
            &inventory.state.containers(),
            Some(source),
        ));
    }
    let grouped = matches!(mode, ViewMode::Table | ViewMode::Tree);
    let external_count = rows
        .iter()
        .map(|row| match row {
            TableRow::Group {
                key: (Some(_), _),
                count,
                ..
            } => *count,
            TableRow::External(..) if !grouped => 1,
            _ => 0,
        })
        .sum::<usize>();

    let visible_ids = rows
        .iter()
        .filter_map(|entry| match entry {
            TableRow::Container(item) => Some(item.id.clone()),
            TableRow::Group { .. } | TableRow::External(..) => None,
        })
        .collect::<Vec<_>>();
    let status = if external_count != 0 || !rows.is_empty() {
        states::TableState::Rows
    } else if !filter.includes_marsa()
        && !view
            .external_inventory
            .iter()
            .any(|workspace| workspace.reachable && filter.includes_external(workspace))
    {
        states::TableState::ExternalOffline
    } else if !filter.includes_marsa()
        || (!view.external_inventory.is_empty()
            && (!view.query.is_empty() || view.filter != StatusFilter::All))
    {
        if view.query.is_empty() && view.filter == StatusFilter::All {
            states::TableState::Empty
        } else {
            states::TableState::NoMatches
        }
    } else {
        local_status.unwrap_or(states::TableState::Empty)
    };
    let total = if grouped {
        rows.iter()
            .filter_map(|entry| match entry {
                TableRow::Group { count, .. } => Some(count),
                TableRow::Container(_) | TableRow::External(..) => None,
            })
            .sum::<usize>()
    } else {
        visible_ids.len() + external_count
    };
    let selected = view.selected.clone();
    let busy = view.command_pending;
    let collapsed = view.collapsed.clone();
    let entity = cx.entity();
    let item_count = if mode == ViewMode::Cards {
        rows.len().div_ceil(card_columns)
    } else {
        rows.len()
    };
    let rows = Arc::new(rows);
    let summary = container_count(total, locale);

    let loaded = status == states::TableState::Rows;
    let mut toolbar = row(cx)
        .w_full()
        .gap_2()
        .items_center()
        .flex_wrap()
        .flex_shrink_0()
        .min_h(px(46.));
    let groups = rows
        .iter()
        .filter_map(|entry| match entry {
            TableRow::Group { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if loaded {
        toolbar = toolbar
            .p_2()
            .rounded(px(12.))
            .border_1()
            .border_color(color(palette, ColorToken::BorderSubtle))
            .bg(color(palette, ColorToken::BgSidebar));
    }
    if let Some(input) = view.input.as_ref() {
        toolbar = toolbar.child(
            div().w(px(216.)).child(
                resource_input(input)
                    .cleanable(true)
                    .prefix(IconName::Search)
                    .aria_label(gui::GUI_DESKTOP_SEARCH_CONTAINERS.text(locale)),
            ),
        );
    }

    let filter_view = entity.downgrade();
    toolbar = toolbar.child(
        Button::new("state-filter")
            .outline()
            .h(px(Space::S36.pixels()))
            .icon(IconName::ChevronDown)
            .label(view.filter.label(locale))
            .dropdown_menu(move |menu, _, _| {
                StatusFilter::ALL.into_iter().fold(menu, |menu, filter| {
                    let weak = filter_view.clone();
                    menu.item(
                        PopupMenuItem::new(filter.label(locale)).on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |view, cx| {
                                view.set_status_filter(filter, cx);
                            });
                        }),
                    )
                })
            }),
    );
    if loaded {
        toolbar = toolbar
            .child(view_mode_rail(mode, &screen, cx))
            .children((!groups.is_empty()).then(|| {
                let weak = entity.downgrade();
                Button::new("container-group-actions")
                    .outline()
                    .size(px(Space::S36.pixels()))
                    .icon(IconName::Ellipsis)
                    .accessibility_label(message(locale, gui::GUI_DESKTOP_GROUPS))
                    .dropdown_menu(move |menu, _, _| {
                        [true, false].into_iter().fold(menu, |menu, expand| {
                            let weak = weak.clone();
                            let groups = groups.clone();
                            let key = if expand {
                                gui::GUI_DESKTOP_EXPAND_ALL_GROUPS
                            } else {
                                gui::GUI_DESKTOP_COLLAPSE_ALL_GROUPS
                            };
                            menu.item(PopupMenuItem::new(message(locale, key)).on_click(
                                move |_, _, cx| {
                                    let _ = weak.update(cx, |view, cx| {
                                        if expand {
                                            for group in &groups {
                                                view.collapsed.remove(group);
                                            }
                                        } else {
                                            view.collapsed.extend(groups.iter().cloned());
                                        }
                                        cx.notify();
                                    });
                                },
                            ))
                        })
                    })
            }))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(ui_size(12.))
                    .text_color(color(palette, ColorToken::TextMuted))
                    .child(summary),
            );
    }

    let header = row(cx)
        .w_full()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_start(cx)
                .child(
                    screen
                        .typography(Role::Title)
                        .apply(div())
                        .child(message(locale, gui::GUI_DESKTOP_CONTAINERS)),
                )
                .child(
                    screen
                        .typography(Role::Small)
                        .apply(div())
                        .text_color(color(palette, ColorToken::TextMuted))
                        .child(states::subtitle(status, locale)),
                ),
        )
        .child(
            Button::new("open-container-global-search")
                .ghost()
                .size(px(36.))
                .p_0()
                .bg(color(palette, ColorToken::BgSubtle))
                .icon(IconName::Search)
                .accessibility_label(gui::GUI_DESKTOP_SEARCH_COMMANDS.text(locale))
                .tooltip(gui::GUI_DESKTOP_SEARCH_K.text(locale))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::desktop::OpenCommandPalette), cx);
                }),
        )
        .child(
            Button::new("new-container")
                .primary()
                .h(px(36.))
                .icon(IconName::Plus)
                .label(gui::GUI_DESKTOP_NEW_CONTAINER.text(locale))
                .disabled(
                    view.services.is_none()
                        || !(local_status.is_some_and(states::TableState::can_create)
                            || !filter.includes_marsa()
                                && view.external_inventory.iter().any(|workspace| {
                                    workspace.reachable && filter.includes_external(workspace)
                                })),
                )
                .on_click(cx.listener(|view, _, window, cx| super::create::open(view, window, cx))),
        );

    let body = div()
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .gap_3()
        .child(header.flex_shrink_0())
        .children(
            matches!(
                status,
                states::TableState::Rows | states::TableState::NoMatches
            )
            .then_some(toolbar),
        )
        .children((loaded && !view.selected.is_empty()).then(|| bulk_toolbar(view, &screen, cx)));

    if !loaded {
        return body
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(states::render(view, status, &screen, cx)),
            )
            .into_any_element();
    }

    let all_selected =
        !visible_ids.is_empty() && visible_ids.iter().all(|id| selected.contains(id));
    let columns = cell_row(palette, cx)
        .flex_shrink_0()
        .bg(color(palette, ColorToken::BgSubtle))
        .text_color(color(palette, ColorToken::TextMuted))
        .child(
            div().w(px(28.)).child(
                Checkbox::new("select-visible")
                    .disabled(visible_ids.is_empty())
                    .accessibility_label(message(
                        screen.locale,
                        gui::GUI_DESKTOP_SELECT_VISIBLE_CONTAINERS,
                    ))
                    .checked(all_selected)
                    .indeterminate(
                        !all_selected && visible_ids.iter().any(|id| selected.contains(id)),
                    )
                    .on_click(cx.listener(move |view, checked, _, cx| {
                        for id in &visible_ids {
                            if *checked {
                                view.selected.insert(id.clone());
                            } else {
                                view.selected.remove(id);
                            }
                        }
                        cx.notify();
                    })),
            ),
        )
        .child(
            div().flex_1().min_w_0().child(
                Button::new("sort-container-names")
                    .ghost()
                    .small()
                    .label(message(screen.locale, gui::GUI_DESKTOP_NAME))
                    .icon(if view.descending {
                        IconName::ArrowDown
                    } else {
                        IconName::ArrowUp
                    })
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.descending = !view.descending;
                        cx.notify();
                    })),
            ),
        )
        .child(column(message(screen.locale, gui::GUI_DESKTOP_IMAGE)))
        .child(fixed_column(
            120.,
            message(screen.locale, gui::GUI_DESKTOP_STATE),
        ))
        .child(fixed_column(
            112.,
            message(screen.locale, gui::GUI_DESKTOP_PORTS),
        ))
        .children(
            (!compact).then(|| fixed_column(78., message(screen.locale, gui::GUI_DESKTOP_AGE))),
        )
        .child(div().w(px(80.)).flex_shrink_0());

    let local_source = (!view.external_inventory.is_empty()).then(|| SharedString::from("Marsa"));
    let presentation = RowPresentation {
        palette,
        locale,
        mode,
        compact,
        view: entity,
    };
    let items = uniform_list("containers-table", item_count, move |range, _, cx| {
        if mode == ViewMode::Cards {
            return range
                .map(|index| {
                    let start = card_columns * index;
                    let entries = rows.iter().skip(start).take(card_columns);
                    let mut cards = row(cx).h(px(150.)).w_full().gap_3().pb_3();
                    let mut count = 0;
                    for row in entries {
                        if let Some(card) = render_entry(
                            row,
                            &selected,
                            busy,
                            local_source.as_ref(),
                            &presentation,
                            cx,
                        ) {
                            cards = cards.child(card);
                            count += 1;
                        }
                    }
                    cards
                        .children((count..card_columns).map(|_| div().flex_1()))
                        .into_any_element()
                })
                .collect();
        }
        range
            .filter_map(|index| rows.get(index))
            .map(|entry| match entry {
                TableRow::Group { key, count, source } => group_row(
                    key,
                    *count,
                    source
                        .as_ref()
                        .map(|source| &source.badge)
                        .or(local_source.as_ref()),
                    collapsed.contains(key),
                    &presentation,
                    cx,
                )
                .into_any_element(),
                entry => render_entry(
                    entry,
                    &selected,
                    busy,
                    local_source.as_ref(),
                    &presentation,
                    cx,
                )
                .unwrap_or_else(|| div().into_any_element()),
            })
            .collect()
    })
    .track_scroll(&view.list_scroll)
    .flex_1()
    .min_h_0();
    let items = div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(items)
        .vertical_scrollbar(&view.list_scroll);
    let table = div().flex().flex_col().flex_1().min_h_0().overflow_hidden();
    let table = if mode == ViewMode::Table {
        table
            .rounded_lg()
            .border_1()
            .border_color(color(palette, ColorToken::BorderSubtle))
            .bg(color(palette, ColorToken::BgElevated))
            .child(columns)
    } else {
        table
    };
    body.child(table.child(items)).into_any_element()
}

fn collect_rows(
    view: &ContainerView,
    source: &BTreeMap<ContainerId, ContainerSummary>,
    engine: Option<Arc<RowSource>>,
) -> Vec<TableRow> {
    let mut projects: BTreeMap<Option<&str>, Vec<&ContainerSummary>> = BTreeMap::new();
    for item in source.values() {
        if matches_filter(view, item) {
            projects
                .entry(item.compose_project())
                .or_default()
                .push(item);
        }
    }
    // Named Compose projects precede the standalone group.
    let standalone = projects.remove(&None);
    projects
        .into_iter()
        .chain(standalone.map(|entries| (None, entries)))
        .flat_map(|(project, mut items)| {
            items.sort_unstable_by(|a, b| {
                let order = name(a).cmp(name(b)).then_with(|| a.id.cmp(&b.id));
                if view.descending {
                    order.reverse()
                } else {
                    order
                }
            });
            let key = (
                engine.as_ref().map(|source| source.key.clone()),
                project.map(str::to_owned),
            );
            let grouped = matches!(view.mode, ViewMode::Table | ViewMode::Tree);
            let collapsed = grouped && view.collapsed.contains(&key);
            let heading = grouped.then_some(TableRow::Group {
                key,
                count: items.len(),
                source: engine.clone(),
            });
            let engine = engine.clone();
            let entries = (!collapsed)
                .then_some(items)
                .into_iter()
                .flatten()
                .cloned()
                .map(move |item| match &engine {
                    Some(engine) => TableRow::External(Box::new(item), engine.clone()),
                    None => TableRow::Container(Box::new(item)),
                });
            heading.into_iter().chain(entries)
        })
        .collect()
}

fn matches_filter(view: &ContainerView, item: &ContainerSummary) -> bool {
    view.filter.accepts(item)
        && (view.query.is_empty()
            || name(item).to_lowercase().contains(&view.query)
            || item.image.to_lowercase().contains(&view.query)
            || item.id.as_str().contains(&view.query)
            || item
                .compose_project()
                .is_some_and(|project| project.to_lowercase().contains(&view.query)))
}

struct RowPresentation {
    palette: Palette,
    locale: Locale,
    mode: ViewMode,
    compact: bool,
    view: marsa_ui::gpui::Entity<ContainerView>,
}

fn render_entry(
    row: &TableRow,
    selected: &std::collections::BTreeSet<ContainerId>,
    busy: bool,
    local_source: Option<&SharedString>,
    presentation: &RowPresentation,
    cx: &App,
) -> Option<AnyElement> {
    let (entry, source, local, target) = match row {
        TableRow::Container(entry) => (
            entry,
            local_source,
            Some(selected.contains(&entry.id)),
            ContainerTarget::Marsa(entry.id.clone()),
        ),
        TableRow::External(entry, engine) => (
            entry,
            Some(&engine.badge),
            None,
            ContainerTarget::External {
                id: entry.id.clone(),
                endpoint: engine.endpoint.clone(),
            },
        ),
        TableRow::Group { .. } => return None,
    };
    Some(container_row(
        entry,
        target,
        source,
        local,
        busy,
        presentation,
        cx,
    ))
}

fn cell_row(palette: Palette, cx: &App) -> Div {
    row(cx)
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_3()
        .gap_2()
        .items_center()
        .text_size(ui_size(12.))
        .text_start(cx)
        .border_b_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
}

fn container_count(count: usize, locale: Locale) -> String {
    match locale {
        Locale::EnglishUs if count == 1 => "1 container".into(),
        Locale::Arabic if count == 1 => "حاوية واحدة".into(),
        Locale::Arabic if count == 2 => "حاويتان".into(),
        Locale::Arabic if (3..=10).contains(&count) => format!("{count} حاويات"),
        Locale::Arabic => format!("{count} حاوية"),
        _ => format!("{count} containers"),
    }
}

fn group_row(
    key: &GroupKey,
    count: usize,
    source: Option<&SharedString>,
    collapsed: bool,
    presentation: &RowPresentation,
    cx: &App,
) -> Div {
    let RowPresentation {
        locale,
        palette,
        mode,
        view,
        ..
    } = presentation;
    let (locale, palette, mode) = (*locale, *palette, *mode);
    let (external, project) = key;
    let project = project.as_deref();
    let count = container_count(count, locale);
    let group = key.clone();
    let id = (
        ElementId::from(SharedString::from(
            external.as_deref().unwrap_or("marsa-group").to_owned(),
        )),
        SharedString::from(
            project.map_or_else(|| "standalone".into(), |name| format!("project:{name}")),
        ),
    );
    let weak = view.downgrade();
    cell_row(palette, cx)
        .bg(color(palette, ColorToken::BgSubtle))
        .rounded(px(if mode == ViewMode::Tree { 8. } else { 0. }))
        .child(
            Button::new(id)
                .ghost()
                .small()
                .compact()
                .icon(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .accessibility_label(if collapsed {
                    gui::GUI_DESKTOP_EXPAND_GROUP.text(locale)
                } else {
                    gui::GUI_DESKTOP_COLLAPSE_GROUP.text(locale)
                })
                .on_click(move |_, _, cx| {
                    let _ = weak.update(cx, |view, cx| {
                        if !view.collapsed.remove(&group) {
                            view.collapsed.insert(group.clone());
                        }
                        cx.notify();
                    });
                }),
        )
        .child(
            Icon::empty()
                .path(if project.is_none() && mode == ViewMode::Tree {
                    "icons/box.svg"
                } else {
                    "icons/panels-top-left.svg"
                })
                .small(),
        )
        .child(
            project
                .map(isolate_technical_text)
                .unwrap_or_else(|| gui::GUI_DESKTOP_STANDALONE.text(locale).into()),
        )
        .children(source.map(|source| {
            crate::screens::external_inventory::source_badge(palette, source.clone())
        }))
        .child(
            div()
                .text_color(color(palette, ColorToken::TextMuted))
                .child(count),
        )
}

fn container_row(
    entry: &ContainerSummary,
    target: ContainerTarget,
    source: Option<&SharedString>,
    local: Option<bool>,
    busy: bool,
    presentation: &RowPresentation,
    cx: &App,
) -> AnyElement {
    let RowPresentation {
        palette,
        locale,
        mode,
        view,
        ..
    } = presentation;
    let (palette, locale, mode) = (*palette, *locale, *mode);
    let checked = local.unwrap_or_default();
    let id = entry.id.clone();
    let check_view = view.downgrade();
    let check_id = id.clone();
    let detail_view = view.downgrade();
    let detail_target = target.clone();
    let menu_view = view.downgrade();
    let menu_target = target.clone();
    let action = if eligible(entry, ContainerAction::Stop) {
        ContainerAction::Stop
    } else {
        ContainerAction::Start
    };
    let can_start = eligible(entry, ContainerAction::Start);
    let can_stop = eligible(entry, ContainerAction::Stop);
    let can_restart = eligible(entry, ContainerAction::Restart);
    let row_background = if checked {
        ColorToken::AccentSubtle
    } else {
        ColorToken::BgElevated
    };
    let check = local.map(|_| {
        Checkbox::new((
            ElementId::from("select-container"),
            SharedString::from(id.as_str().to_owned()),
        ))
        .checked(checked)
        .accessibility_label(format!(
            "{} {}",
            gui::GUI_DESKTOP_SELECT.text(locale),
            name(entry)
        ))
        .on_click(move |selected, _, cx| {
            let _ = check_view.update(cx, |view, cx| {
                if *selected {
                    view.selected.insert(check_id.clone());
                } else {
                    view.selected.remove(&check_id);
                }
                cx.notify();
            });
        })
    });
    let name_button = Button::new((
        ElementId::from(if local.is_some() {
            "container-details"
        } else {
            "external-details"
        }),
        SharedString::from(id.as_str().to_owned()),
    ))
    .ghost()
    .small()
    .compact()
    .label(isolate_technical_text(name(entry)))
    .on_click(move |_, _, cx| {
        let _ = detail_view.update(cx, |view, cx| view.open_detail(detail_target.clone(), cx));
    });
    let name_cell = || {
        row(cx)
            .min_w_0()
            .gap_1()
            .child(div().flex_1().min_w_0().truncate().child(name_button))
            .children(source.map(|source| {
                crate::screens::external_inventory::source_badge(palette, source.clone())
            }))
    };
    let menu_name = isolate_technical_text(name(entry));
    let state = entry.state.clone();
    let more = {
        Button::new((
            ElementId::from(if local.is_some() {
                "container-menu"
            } else {
                "external-container-menu"
            }),
            SharedString::from(id.as_str().to_owned()),
        ))
        .ghost()
        .small()
        .icon(IconName::Ellipsis)
        .accessibility_label(gui::GUI_DESKTOP_CONTAINER_ACTIONS.text(locale))
        .tooltip(gui::GUI_DESKTOP_CONTAINER_ACTIONS.text(locale))
        .dropdown_menu(move |menu, _, _| {
            let remove_view = menu_view.clone();
            let remove_target = menu_target.clone();
            let detail_view = menu_view.clone();
            let detail_target = menu_target.clone();
            let first = menu.label(menu_name.clone()).separator().item(
                PopupMenuItem::new(gui::GUI_DESKTOP_OPEN_DETAILS.text(locale))
                    .icon(IconName::ChevronLeft)
                    .on_click(move |_, _, cx| {
                        let _ = detail_view
                            .update(cx, |view, cx| view.open_detail(detail_target.clone(), cx));
                    }),
            );
            let menu = [
                ContainerAction::Start,
                ContainerAction::Stop,
                ContainerAction::Restart,
                ContainerAction::Pause,
                ContainerAction::Unpause,
            ]
            .into_iter()
            .fold(first, |menu, action| {
                let weak = menu_view.clone();
                let target = menu_target.clone();
                let enabled = match action {
                    ContainerAction::Start => can_start,
                    ContainerAction::Stop => can_stop,
                    ContainerAction::Restart => can_restart,
                    ContainerAction::Pause => state == "running",
                    ContainerAction::Unpause => state == "paused",
                };
                if !enabled {
                    return menu;
                }
                let icon = action_icon(action);
                let (en, ar) = action_label(action);
                menu.item(
                    PopupMenuItem::new(copy(locale, en, ar))
                        .icon(icon)
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |view, cx| {
                                view.dispatch_target(target.clone(), action, cx)
                            });
                        }),
                )
            });
            menu.item(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .text_color(color(palette, ColorToken::StatusError))
                        .child(gui::GUI_DESKTOP_CONTAINERS_REMOVE_MENU.text(locale))
                })
                .icon(Icon::empty().path("icons/trash-2.svg"))
                .disabled(busy)
                .on_click(move |_, window, cx| {
                    let _ = remove_view.update(cx, |view, cx| {
                        super::removal::open(
                            view,
                            vec![remove_target.id().clone()],
                            remove_target.endpoint().cloned(),
                            window,
                            cx,
                        )
                    });
                }),
            )
        })
    };
    let public_ip = target.endpoint().and_then(|endpoint| endpoint.public_ip());
    let ports = published_ports(entry, public_ip, palette, locale);
    let lifecycle = action_button(entry, target, action, busy, (locale, palette), view)
        .ghost()
        .bg(color(palette, ColorToken::GlassControl));
    let image = isolate_technical_text(&entry.image);
    let tiny_image = || {
        div()
            .min_w_0()
            .truncate()
            .font_family("IBM Plex Mono")
            .text_size(ui_size(11.))
            .text_color(color(palette, ColorToken::TextMuted))
            .child(image.clone())
    };
    let pill = state_pill(&entry.state, palette, locale, cx);
    let row_id = format!(
        "{}:{}",
        source.map_or("marsa", |source| source.as_ref()),
        id.as_str()
    );
    if mode == ViewMode::Cards {
        return div()
            .id(SharedString::from(row_id))
            .hover(|style| style.bg(color(palette, ColorToken::BgHover)))
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .bg(color(palette, row_background))
            .border_1()
            .border_color(color(palette, ColorToken::BorderSubtle))
            .child(
                row(cx).items_center().gap_2().children(check).child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .child(name_cell()),
                ),
            )
            .child(tiny_image())
            .child(
                div()
                    .w_full()
                    .border_b_1()
                    .border_color(color(palette, ColorToken::BorderSubtle)),
            )
            .child(
                row(cx)
                    .items_center()
                    .gap_2()
                    .child(pill)
                    .child(div().flex_1())
                    .child(ports),
            )
            .child(
                row(cx)
                    .gap_2()
                    .justify_row_end()
                    .child(lifecycle)
                    .child(more),
            )
            .into_any_element();
    }
    let mut result = cell_row(palette, cx)
        .id(SharedString::from(row_id))
        .hover(|style| style.bg(color(palette, ColorToken::BgHover)));
    if mode == ViewMode::Table || checked {
        result = result.bg(color(palette, row_background));
    }
    if mode == ViewMode::Tree {
        result = result.padding_start(px(28.), cx);
    }
    if mode == ViewMode::List {
        result = result
            .h(px(61.))
            .children(check)
            .child(Icon::default().path("icons/box.svg").small())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().min_w_0().overflow_hidden().child(name_cell()))
                    .child(tiny_image()),
            );
    } else {
        result = result.child(div().w(px(28.)).children(check));
        if mode == ViewMode::Tree {
            result = result.child(Icon::empty().path("icons/box.svg").small());
        }
        result = result
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(name_cell()),
            )
            .child(column(image).font_family("IBM Plex Mono"));
    }
    result = result
        .child(div().w(px(120.)).flex_shrink_0().child(pill))
        .child(
            div()
                .w(px(112.))
                .flex_shrink_0()
                .overflow_hidden()
                .child(ports),
        );
    if mode != ViewMode::Tree && !presentation.compact {
        result = result.child(fixed_column(78., age_text(entry.created, locale)));
    }
    result.child(lifecycle).child(more).into_any_element()
}

fn action_icon(action: ContainerAction) -> IconName {
    match action {
        ContainerAction::Start | ContainerAction::Unpause => IconName::Play,
        ContainerAction::Stop => IconName::Square,
        ContainerAction::Restart => IconName::RefreshCw,
        ContainerAction::Pause => IconName::Pause,
    }
}

pub(super) fn action_button(
    entry: &ContainerSummary,
    target: ContainerTarget,
    action: ContainerAction,
    busy: bool,
    appearance: (Locale, Palette),
    entity: &marsa_ui::gpui::Entity<ContainerView>,
) -> Button {
    let (locale, palette) = appearance;
    let id = entry.id.as_str().to_owned();
    let external = target.endpoint().is_some();
    let weak = entity.downgrade();
    let icon = action_icon(action);
    let tint = match action {
        ContainerAction::Start | ContainerAction::Unpause => ColorToken::StatusRunning,
        ContainerAction::Stop | ContainerAction::Pause => ColorToken::StatusWarning,
        ContainerAction::Restart => ColorToken::AccentDefault,
    };
    let (en, ar) = action_label(action);
    Button::new((
        ElementId::from(if external {
            SharedString::from(format!("external-{en}"))
        } else {
            SharedString::from(en)
        }),
        SharedString::from(id),
    ))
    .outline()
    .size(px(36.))
    .p_0()
    .icon(Icon::new(icon).text_color(color(
        palette,
        if busy || !eligible(entry, action) {
            ColorToken::TextMuted
        } else {
            tint
        },
    )))
    .tooltip(copy(locale, en, ar))
    .accessibility_label(copy(locale, en, ar))
    .disabled(busy || !eligible(entry, action))
    .on_click(move |_, _, cx| {
        let _ = weak.update(cx, |view, cx| {
            view.dispatch_target(target.clone(), action, cx)
        });
    })
}

pub(super) fn state_pill(state: &str, palette: Palette, locale: Locale, cx: &App) -> Div {
    let (foreground, background) = match state {
        "running" => (ColorToken::StatusRunning, ColorToken::StatusRunningBg),
        "paused" => (ColorToken::StatusPaused, ColorToken::StatusPausedBg),
        "restarting" => (ColorToken::StatusStarting, ColorToken::StatusStartingBg),
        _ => (ColorToken::StatusStopped, ColorToken::StatusStoppedBg),
    };
    let (en, ar) = state_label(state);
    row(cx)
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_full()
        .bg(color(palette, background))
        .text_color(color(palette, foreground))
        .text_size(ui_size(11.))
        .child(
            div()
                .size(px(8.))
                .rounded_full()
                .bg(color(palette, foreground)),
        )
        .child(copy(locale, en, ar))
}

fn published_ports(
    entry: &ContainerSummary,
    public_ip: Option<IpAddr>,
    palette: Palette,
    locale: Locale,
) -> Div {
    let ports = port_text(entry);
    let cell = div()
        .min_w_0()
        .truncate()
        .font_family("IBM Plex Mono")
        .text_size(ui_size(11.))
        .text_color(color(palette, ColorToken::AccentDefault));
    let Some(ip) = public_ip.filter(|ip| !ip.is_unspecified() && !ip.is_multicast()) else {
        return cell.child(ports);
    };
    let targets: BTreeSet<_> = entry
        .ports
        .iter()
        .filter(|port| port.protocol == "tcp")
        .filter_map(|port| port.public_port.filter(|port| *port != 0))
        .map(|port| {
            let scheme = if port == 443 { "https" } else { "http" };
            format!("{scheme}://{}", SocketAddr::new(ip, port))
        })
        .collect();
    if targets.is_empty() {
        return cell.child(ports);
    }
    let help = gui::GUI_ENVIRONMENT_OPEN_PUBLISHED_PORT.text(locale);
    cell.child(
        Button::new((
            ElementId::from("published-port-links"),
            SharedString::from(entry.id.as_str().to_owned()),
        ))
        .ghost()
        .small()
        .label(ports)
        .tooltip(help)
        .accessibility_label(help)
        .dropdown_menu(move |menu, _, _| {
            targets.iter().fold(menu, |menu, url| {
                let url = url.clone();
                menu.item(PopupMenuItem::new(isolate_technical_text(&url)).on_click(
                    move |_, _, cx| {
                        cx.open_url(&url);
                    },
                ))
            })
        }),
    )
}

pub(super) fn port_text(entry: &ContainerSummary) -> SharedString {
    let ports = entry
        .ports
        .iter()
        .filter_map(|port| {
            port.public_port
                .map(|published| format!("{published}:{}", port.private_port))
        })
        .collect::<Vec<_>>();
    if ports.is_empty() {
        "—".into()
    } else {
        isolate_technical_text(&ports.join(", "))
    }
}

pub(super) fn age_text(created: i64, locale: Locale) -> SharedString {
    let Ok(created) = u64::try_from(created) else {
        return "—".into();
    };
    if created == 0 {
        return "—".into();
    }
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return "—".into();
    };
    let seconds = now.as_secs().saturating_sub(created);
    let (amount, en, ar) = match seconds {
        0..60 => (seconds, "s", "ث"),
        60..3600 => (seconds / 60, "min", "د"),
        3600..86400 => (seconds / 3600, "h", "س"),
        _ => (seconds / 86400, "d", "ي"),
    };
    format!("{amount} {}", copy(locale, en, ar)).into()
}

fn column(value: SharedString) -> Div {
    div().flex_1().min_w_0().truncate().child(value)
}

fn fixed_column(width: f32, value: SharedString) -> Div {
    div().w(px(width)).flex_shrink_0().truncate().child(value)
}

fn view_mode_rail(mode: ViewMode, screen: &ScreenContext<'_>, cx: &Context<ContainerView>) -> Div {
    row(cx)
        .gap_1()
        .px_1()
        .items_center()
        .rounded_md()
        .h(px(36.))
        .border_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .children(
            [
                (
                    ViewMode::Table,
                    "grouped-table",
                    gui::GUI_DESKTOP_GROUPED_TABLE,
                    "icons/table-2.svg",
                ),
                (
                    ViewMode::List,
                    "flat-list",
                    gui::GUI_DESKTOP_LIST,
                    "icons/list.svg",
                ),
                (
                    ViewMode::Cards,
                    "cards",
                    gui::GUI_DESKTOP_CARDS,
                    "icons/layout-grid.svg",
                ),
                (
                    ViewMode::Tree,
                    "project-tree",
                    gui::GUI_DESKTOP_PROJECT_TREE,
                    "icons/git-fork.svg",
                ),
            ]
            .map(|(target, id, label_key, icon)| {
                let title = message(screen.locale, label_key);
                let control = Button::new(id)
                    .size(px(30.))
                    .p_0()
                    .icon(Icon::default().path(icon))
                    .accessibility_label(title.clone())
                    .tooltip(title);
                let control = if mode == target {
                    control
                        .ghost()
                        .bg(color(screen.palette, ColorToken::AccentSubtle))
                        .text_color(color(screen.palette, ColorToken::AccentDefault))
                } else {
                    control.ghost()
                };
                control.on_click(cx.listener(move |view, _, _, cx| {
                    view.mode = target;
                    cx.notify();
                }))
            }),
        )
}

fn bulk_toolbar(
    view: &ContainerView,
    screen: &ScreenContext<'_>,
    cx: &Context<ContainerView>,
) -> Div {
    let count = view.selected.len();
    let summary = match screen.locale {
        Locale::EnglishUs => format!("{count} selected"),
        Locale::Arabic if count == 1 => "عنصر واحد محدد".into(),
        Locale::Arabic if count == 2 => "عنصران محددان".into(),
        Locale::Arabic => format!("{count} عناصر محددة"),
    };
    let mut tools = row(cx)
        .items_center()
        .gap_2()
        .flex_wrap()
        .p_2()
        .rounded_md()
        .bg(color(screen.palette, ColorToken::AccentSubtle))
        .child(
            div()
                .text_size(ui_size(12.))
                .text_color(color(screen.palette, ColorToken::AccentDefault))
                .child(summary),
        );
    if count == 1 {
        tools = tools.child(
            Button::new("open-selected-container")
                .outline()
                .size(px(36.))
                .p_0()
                .icon(Icon::default().path("icons/arrow-up-left.svg"))
                .accessibility_label(gui::GUI_DESKTOP_OPEN_DETAILS.text(screen.locale))
                .tooltip(gui::GUI_DESKTOP_OPEN_DETAILS.text(screen.locale))
                .on_click(cx.listener(|view, _, _, cx| {
                    if let Some(id) = view.selected.iter().next().cloned() {
                        view.open_detail(ContainerTarget::Marsa(id), cx);
                    }
                })),
        );
    }
    for (action, icon) in [
        (ContainerAction::Start, IconName::Play),
        (ContainerAction::Stop, IconName::Square),
        (ContainerAction::Restart, IconName::RefreshCw),
    ] {
        let (en, ar) = action_label(action);
        let enabled = view.selected.iter().any(|id| {
            view.state
                .container(id)
                .is_some_and(|entry| eligible(entry, action))
        });
        if !enabled && action != ContainerAction::Restart {
            continue;
        }
        tools = tools.child(
            Button::new(en)
                .outline()
                .size(px(36.))
                .p_0()
                .icon(icon)
                .accessibility_label(label(screen, en, ar))
                .tooltip(label(screen, en, ar))
                .disabled(view.command_pending || !enabled)
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.dispatch(view.selected.iter().cloned().collect(), action, cx)
                })),
        );
    }
    tools
        .child(
            Button::new("remove-selected-containers")
                .outline()
                .size(px(36.))
                .p_0()
                .icon(Icon::default().path("icons/trash-2.svg"))
                .bg(color(screen.palette, ColorToken::StatusErrorBg))
                .text_color(color(screen.palette, ColorToken::StatusError))
                .accessibility_label(gui::GUI_DESKTOP_REMOVE.text(screen.locale))
                .tooltip(gui::GUI_DESKTOP_REMOVE.text(screen.locale))
                .disabled(view.command_pending)
                .on_click(cx.listener(|view, _, window, cx| {
                    view.request_remove(view.selected.iter().cloned().collect(), window, cx)
                })),
        )
        .child(
            Button::new("clear-container-selection")
                .outline()
                .size(px(36.))
                .p_0()
                .icon(IconName::Close)
                .accessibility_label(gui::GUI_DESKTOP_CLEAR_SELECTION.text(screen.locale))
                .tooltip(gui::GUI_DESKTOP_CLEAR_SELECTION.text(screen.locale))
                .on_click(cx.listener(|view, _, _, cx| {
                    view.selected.clear();
                    cx.notify();
                })),
        )
}
