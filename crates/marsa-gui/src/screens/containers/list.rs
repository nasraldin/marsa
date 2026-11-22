//! Searchable, virtualized container table and selection controls.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use marsa_client::{
    api::EngineState,
    docker::{ContainerAction, ContainerId, ContainerSummary},
};
use marsa_ui::{
    ColorToken, Locale, Palette,
    direction::{LogicalStyle as _, isolate_technical_text, row},
    gpui::{
        AnyElement, App, Context, Div, ElementId, FontWeight, IntoElement, ParentElement as _,
        SharedString, Styled as _, div, px, uniform_list,
    },
    gpui_component::{
        Disableable as _, Icon, IconName, Sizable as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        input::Input,
        menu::{DropdownMenu as _, PopupMenuItem},
    },
};

use crate::screens::common::{ScreenContext, copy, label, panel};

use super::{ContainerView, StatusFilter, action_label, color, eligible, name, state_label};

const ROW_HEIGHT: f32 = 43.;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ViewMode {
    #[default]
    Table,
    List,
    Cards,
    Tree,
}

#[derive(Clone)]
enum TableRow {
    Group {
        project: Option<String>,
        count: usize,
    },
    Container(ContainerSummary),
}

pub(super) fn render(view: &ContainerView, cx: &mut Context<ContainerView>) -> AnyElement {
    let screen = view.screen();
    let locale = screen.locale;
    let palette = screen.palette;
    let mode = view.mode;
    let source = view.state.containers();
    let rows = collect_rows(view, &source);
    let visible_ids = rows
        .iter()
        .filter_map(|entry| match entry {
            TableRow::Container(item) => Some(item.id.clone()),
            TableRow::Group { .. } => None,
        })
        .collect::<Vec<_>>();
    let total = if matches!(mode, ViewMode::Table | ViewMode::Tree) {
        rows.iter()
            .filter_map(|entry| match entry {
                TableRow::Group { count, .. } => Some(count),
                TableRow::Container(_) => None,
            })
            .sum()
    } else {
        visible_ids.len()
    };
    let selected = view.selected.clone();
    let busy = view.command_pending;
    let collapsed = view.collapsed.clone();
    let entity = cx.entity();
    let item_count = if mode == ViewMode::Cards {
        rows.len().div_ceil(3)
    } else {
        rows.len()
    };
    let rows = Arc::new(rows);
    let summary = match locale {
        Locale::EnglishUs if total == 1 => "1 container".into(),
        Locale::EnglishUs => format!("{total} containers"),
        Locale::Arabic => format!("{total} حاوية"),
    };

    let mut toolbar = row(cx)
        .w_full()
        .gap_2()
        .items_center()
        .px_2()
        .py_2()
        .rounded_lg()
        .border_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
        .bg(color(palette, ColorToken::BgSidebar));
    if let Some(input) = view.input.as_ref() {
        toolbar = toolbar.child(
            div()
                .w(px(216.))
                .child(Input::new(input).small().cleanable(true)),
        );
    }

    let filter_view = entity.downgrade();
    toolbar = toolbar
        .child(
            Button::new("state-filter")
                .outline()
                .small()
                .icon(IconName::ChevronDown)
                .label(filter_label(locale, view.filter))
                .dropdown_menu(move |menu, _, _| {
                    [
                        StatusFilter::All,
                        StatusFilter::Running,
                        StatusFilter::Paused,
                        StatusFilter::Stopped,
                    ]
                    .into_iter()
                    .fold(menu, |menu, filter| {
                        let weak = filter_view.clone();
                        menu.item(PopupMenuItem::new(filter_label(locale, filter)).on_click(
                            move |_, _, cx| {
                                let _ = weak.update(cx, |view, cx| {
                                    view.filter = filter;
                                    cx.notify();
                                });
                            },
                        ))
                    })
                }),
        )
        .child(
            row(cx)
                .gap_1()
                .p_1()
                .items_center()
                .rounded_md()
                .border_1()
                .border_color(color(palette, ColorToken::BorderSubtle))
                .children(
                    [
                        (
                            ViewMode::Table,
                            "grouped-table",
                            "Grouped table",
                            "جدول مجمّع",
                            "icons/table-2.svg",
                        ),
                        (
                            ViewMode::List,
                            "flat-list",
                            "List",
                            "قائمة",
                            "icons/list.svg",
                        ),
                        (
                            ViewMode::Cards,
                            "cards",
                            "Cards",
                            "بطاقات",
                            "icons/layout-grid.svg",
                        ),
                        (
                            ViewMode::Tree,
                            "project-tree",
                            "Project tree",
                            "شجرة المشاريع",
                            "icons/git-fork.svg",
                        ),
                    ]
                    .map(|(target, id, en, ar, icon)| {
                        let title = label(&screen, en, ar);
                        let control = Button::new(id)
                            .small()
                            .compact()
                            .icon(Icon::default().path(icon))
                            .accessibility_label(title.clone())
                            .tooltip(title);
                        let control = if mode == target {
                            control.secondary()
                        } else {
                            control.ghost()
                        };
                        control.on_click(cx.listener(move |view, _, _, cx| {
                            view.mode = target;
                            cx.notify();
                        }))
                    }),
                ),
        )
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(12.))
                .text_color(color(palette, ColorToken::TextMuted))
                .child(summary),
        );

    let mut body = div()
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .gap_3()
        .child(toolbar);

    if view.command_failed {
        body = body.child(
            panel(&screen)
                .text_color(color(palette, ColorToken::StatusError))
                .child(message(
                    screen.locale,
                    gui::GUI_DESKTOP_THE_CONTAINER_ACTION_COULDN_T_BE_COMPLETED,
                )),
        );
    }
    if view.command_pending {
        body = body.child(div().text_size(px(12.)).child(message(
            screen.locale,
            gui::GUI_DESKTOP_APPLYING_CONTAINER_ACTIONS,
        )));
    }
    if !view.selected.is_empty() {
        body = body.child(bulk_toolbar(view, &screen, cx));
    }

    if item_count == 0
        || !matches!(
            view.state
                .engine()
                .and_then(|status| EngineState::try_from(status.state).ok()),
            Some(EngineState::Ready | EngineState::Idle)
        )
    {
        return body
            .child(empty_state(view, &screen, cx))
            .into_any_element();
    }

    let all_selected =
        !visible_ids.is_empty() && visible_ids.iter().all(|id| selected.contains(id));
    let columns = cell_row(palette, cx)
        .bg(color(palette, ColorToken::BgSubtle))
        .text_color(color(palette, ColorToken::TextMuted))
        .child(
            div().w(px(28.)).child(
                Checkbox::new("select-visible")
                    .accessibility_label(message(
                        screen.locale,
                        gui::GUI_DESKTOP_SELECT_VISIBLE_CONTAINERS,
                    ))
                    .checked(all_selected)
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
        .child(fixed_column(
            78.,
            message(screen.locale, gui::GUI_DESKTOP_AGE),
        ))
        .child(div().w(px(80.)).flex_shrink_0());

    let items = uniform_list("containers-table", item_count, move |range, _, cx| {
        if mode == ViewMode::Cards {
            return range
                .map(|index| {
                    let start = 3 * index;
                    let entries = rows.iter().skip(start).take(3);
                    let mut cards = row(cx).h(px(150.)).w_full().gap_3().pb_3();
                    let mut count = 0;
                    for row in entries {
                        if let TableRow::Container(entry) = row {
                            cards = cards.child(container_row(
                                entry,
                                selected.contains(&entry.id),
                                busy,
                                palette,
                                locale,
                                mode,
                                &entity,
                                cx,
                            ));
                            count += 1;
                        }
                    }
                    cards
                        .children((count..3).map(|_| div().flex_1()))
                        .into_any_element()
                })
                .collect();
        }
        range
            .filter_map(|index| rows.get(index))
            .map(|entry| match entry {
                TableRow::Group { project, count } => group_row(
                    project.as_deref(),
                    *count,
                    palette,
                    locale,
                    collapsed.contains(project),
                    &entity,
                    cx,
                )
                .into_any_element(),
                TableRow::Container(entry) => container_row(
                    entry,
                    selected.contains(&entry.id),
                    busy,
                    palette,
                    locale,
                    mode,
                    &entity,
                    cx,
                )
                .into_any_element(),
            })
            .collect()
    })
    .flex_1()
    .min_h_0();
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
) -> Vec<TableRow> {
    let mut projects: BTreeMap<Option<&str>, Vec<&ContainerSummary>> = BTreeMap::new();
    for item in source.values() {
        let name_matches = view.query.is_empty()
            || name(item).to_lowercase().contains(&view.query)
            || item.image.to_lowercase().contains(&view.query)
            || item.id.as_str().contains(&view.query)
            || item
                .compose_project()
                .is_some_and(|project| project.to_lowercase().contains(&view.query));
        if name_matches && view.filter.accepts(&item.state) {
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
            let project = project.map(str::to_owned);
            let grouped = matches!(view.mode, ViewMode::Table | ViewMode::Tree);
            let collapsed = grouped && view.collapsed.contains(&project);
            let heading = grouped.then(|| TableRow::Group {
                project,
                count: items.len(),
            });
            let entries = (!collapsed)
                .then_some(items)
                .into_iter()
                .flatten()
                .cloned()
                .map(TableRow::Container);
            heading.into_iter().chain(entries)
        })
        .collect()
}

fn cell_row(palette: Palette, cx: &App) -> Div {
    row(cx)
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_3()
        .gap_2()
        .items_center()
        .text_size(px(12.))
        .text_start(cx)
        .border_b_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
}

fn group_row(
    project: Option<&str>,
    count: usize,
    palette: Palette,
    locale: Locale,
    collapsed: bool,
    view: &marsa_ui::gpui::Entity<ContainerView>,
    cx: &App,
) -> Div {
    let count = match locale {
        Locale::EnglishUs if count == 1 => "1 container".into(),
        Locale::Arabic => format!("{count} حاوية"),
        _ => format!("{count} containers"),
    };
    let group = project.map(str::to_owned);
    let id = SharedString::from(project.map_or_else(
        || "standalone".to_string(),
        |name| format!("project:{name}"),
    ));
    let weak = view.downgrade();
    cell_row(palette, cx)
        .bg(color(palette, ColorToken::BgSubtle))
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
        .child(Icon::default().path("icons/panels-top-left.svg").small())
        .child(
            project
                .map(isolate_technical_text)
                .unwrap_or_else(|| gui::GUI_DESKTOP_STANDALONE.text(locale).into()),
        )
        .child(
            div()
                .text_color(color(palette, ColorToken::TextMuted))
                .child(count),
        )
}

#[allow(clippy::too_many_arguments)]
fn container_row(
    entry: &ContainerSummary,
    checked: bool,
    busy: bool,
    palette: Palette,
    locale: Locale,
    mode: ViewMode,
    view: &marsa_ui::gpui::Entity<ContainerView>,
    cx: &App,
) -> Div {
    let id = entry.id.clone();
    let check_view = view.downgrade();
    let check_id = id.clone();
    let detail_view = view.downgrade();
    let detail_id = id.clone();
    let menu_view = view.downgrade();
    let menu_id = id.clone();
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
    let check = Checkbox::new((
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
    });
    let name_button = Button::new((
        ElementId::from("container-details"),
        SharedString::from(id.as_str().to_owned()),
    ))
    .ghost()
    .small()
    .compact()
    .label(isolate_technical_text(name(entry)))
    .on_click(move |_, _, cx| {
        let _ = detail_view.update(cx, |view, cx| view.open_detail(detail_id.clone(), cx));
    });
    let more = Button::new((
        ElementId::from("container-menu"),
        SharedString::from(id.as_str().to_owned()),
    ))
    .ghost()
    .small()
    .icon(IconName::Ellipsis)
    .accessibility_label(gui::GUI_DESKTOP_CONTAINER_ACTIONS.text(locale))
    .tooltip(gui::GUI_DESKTOP_CONTAINER_ACTIONS.text(locale))
    .dropdown_menu(move |menu, _, _| {
        let remove_view = menu_view.clone();
        let remove_id = menu_id.clone();
        let menu = [
            ContainerAction::Start,
            ContainerAction::Stop,
            ContainerAction::Restart,
        ]
        .into_iter()
        .fold(menu, |menu, action| {
            let weak = menu_view.clone();
            let id = menu_id.clone();
            let enabled = match action {
                ContainerAction::Start => can_start,
                ContainerAction::Stop => can_stop,
                ContainerAction::Restart => can_restart,
            };
            let (en, ar) = action_label(action);
            menu.item(
                PopupMenuItem::new(copy(locale, en, ar))
                    .disabled(busy || !enabled)
                    .on_click(move |_, _, cx| {
                        let _ =
                            weak.update(cx, |view, cx| view.dispatch(vec![id.clone()], action, cx));
                    }),
            )
        });
        menu.item(
            PopupMenuItem::new(gui::GUI_DESKTOP_REMOVE.text(locale))
                .disabled(busy)
                .on_click(move |_, window, cx| {
                    let _ = remove_view.update(cx, |view, cx| {
                        view.request_remove(vec![remove_id.clone()], window, cx)
                    });
                }),
        )
    });
    let lifecycle = action_button(entry, action, busy, locale, view);
    let image = isolate_technical_text(&entry.image);
    let tiny_image = || {
        div()
            .min_w_0()
            .truncate()
            .font_family("IBM Plex Mono")
            .text_size(px(11.))
            .text_color(color(palette, ColorToken::TextMuted))
            .child(image.clone())
    };
    let pill = state_pill(&entry.state, palette, locale, cx);
    let ports = port_text(entry);
    if mode == ViewMode::Cards {
        return div()
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
                row(cx)
                    .items_center()
                    .gap_2()
                    .child(check)
                    .child(div().min_w_0().overflow_hidden().child(name_button)),
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
                    .child(
                        div()
                            .font_family("IBM Plex Mono")
                            .text_size(px(11.))
                            .text_color(color(palette, ColorToken::AccentDefault))
                            .child(ports),
                    ),
            )
            .child(
                row(cx)
                    .gap_2()
                    .justify_row_end()
                    .child(lifecycle)
                    .child(more),
            );
    }
    let mut result = cell_row(palette, cx);
    if mode == ViewMode::Table || checked {
        result = result.bg(color(palette, row_background));
    }
    if mode == ViewMode::Tree {
        result = result.padding_start(px(28.), cx);
    }
    if mode == ViewMode::List {
        result = result
            .h(px(61.))
            .child(check)
            .child(Icon::default().path("icons/box.svg").small())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().min_w_0().overflow_hidden().child(name_button))
                    .child(tiny_image()),
            );
    } else {
        result = result
            .child(div().w(px(28.)).child(check))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(name_button),
            )
            .child(column(image).font_family("IBM Plex Mono"));
    }
    result
        .child(div().w(px(120.)).flex_shrink_0().child(pill))
        .child(
            fixed_column(112., ports)
                .font_family("IBM Plex Mono")
                .text_color(color(palette, ColorToken::AccentDefault)),
        )
        .child(fixed_column(78., age_text(entry.created, locale)))
        .child(lifecycle)
        .child(more)
}

pub(super) fn action_button(
    entry: &ContainerSummary,
    action: ContainerAction,
    busy: bool,
    locale: Locale,
    entity: &marsa_ui::gpui::Entity<ContainerView>,
) -> Button {
    let id = entry.id.clone();
    let weak = entity.downgrade();
    let icon = match action {
        ContainerAction::Start => IconName::Play,
        ContainerAction::Stop => IconName::Square,
        ContainerAction::Restart => IconName::RefreshCw,
    };
    let (en, ar) = action_label(action);
    Button::new((
        ElementId::from(en),
        SharedString::from(id.as_str().to_owned()),
    ))
    .outline()
    .small()
    .icon(icon)
    .tooltip(copy(locale, en, ar))
    .accessibility_label(copy(locale, en, ar))
    .disabled(busy || !eligible(entry, action))
    .on_click(move |_, _, cx| {
        let _ = weak.update(cx, |view, cx| view.dispatch(vec![id.clone()], action, cx));
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
        .text_size(px(11.))
        .child(
            div()
                .size(px(8.))
                .rounded_full()
                .bg(color(palette, foreground)),
        )
        .child(copy(locale, en, ar))
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

fn filter_label(locale: Locale, filter: StatusFilter) -> &'static str {
    let (en, ar) = match filter {
        StatusFilter::All => ("All states", "كل الحالات"),
        StatusFilter::Running => ("Running", "قيد التشغيل"),
        StatusFilter::Paused => ("Paused", "متوقفة مؤقتًا"),
        StatusFilter::Stopped => ("Stopped", "متوقفة"),
    };
    copy(locale, en, ar)
}

fn bulk_toolbar(
    view: &ContainerView,
    screen: &ScreenContext<'_>,
    cx: &Context<ContainerView>,
) -> Div {
    let count = view.selected.len();
    let summary = match screen.locale {
        Locale::EnglishUs => format!("{count} selected"),
        Locale::Arabic => format!("{count} محددة"),
    };
    row(cx)
        .w_full()
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(color(screen.palette, ColorToken::AccentSubtle))
        .items_center()
        .gap_2()
        .child(summary)
        .child(div().flex_1())
        .children(
            [
                ContainerAction::Start,
                ContainerAction::Stop,
                ContainerAction::Restart,
            ]
            .map(|action| {
                let (en, ar) = action_label(action);
                let enabled = view.selected.iter().any(|id| {
                    view.state
                        .container(id)
                        .is_some_and(|entry| eligible(entry, action))
                });
                Button::new(en)
                    .outline()
                    .small()
                    .label(label(screen, en, ar))
                    .disabled(view.command_pending || !enabled)
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.dispatch(view.selected.iter().cloned().collect(), action, cx);
                    }))
            }),
        )
        .child(
            Button::new("remove-selected-containers")
                .danger()
                .small()
                .label(message(screen.locale, gui::GUI_DESKTOP_REMOVE))
                .disabled(view.command_pending)
                .on_click(cx.listener(|view, _, window, cx| {
                    view.request_remove(view.selected.iter().cloned().collect(), window, cx);
                })),
        )
        .child(
            Button::new("clear-container-selection")
                .ghost()
                .small()
                .label(message(screen.locale, gui::GUI_DESKTOP_CLEAR_SELECTION))
                .on_click(cx.listener(|view, _, _, cx| {
                    view.selected.clear();
                    cx.notify();
                })),
        )
}

fn empty_state(view: &ContainerView, screen: &ScreenContext<'_>, cx: &App) -> Div {
    let engine = view
        .state
        .engine()
        .and_then(|status| EngineState::try_from(status.state).ok());
    let title = match (view.services.is_some(), engine) {
        (false, _) => ("Can’t connect to Marsa", "يتعذّر الاتصال بمرسى"),
        (true, None | Some(EngineState::Starting | EngineState::Booting)) => {
            ("Loading containers…", "جارٍ تحميل الحاويات…")
        }
        (true, Some(EngineState::Ready | EngineState::Idle))
            if !view.query.is_empty() || view.filter != StatusFilter::All =>
        {
            ("No matching containers", "لا توجد حاويات مطابقة")
        }
        (true, Some(EngineState::Ready | EngineState::Idle)) => {
            ("No containers yet", "لا توجد حاويات بعد")
        }
        (true, Some(EngineState::Failed)) => ("The engine couldn’t start", "تعذّر بدء المحرك"),
        _ => ("The engine is stopped", "المحرك متوقف"),
    };
    panel(screen)
        .w_full()
        .min_h(px(250.))
        .flex()
        .items_center()
        .justify_center()
        .text_start(cx)
        .child(Icon::default().path("icons/box.svg").small())
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(label(screen, title.0, title.1)),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(color(screen.palette, ColorToken::TextMuted))
                .child(match title.0 {
                    "No containers yet" => message(
                        screen.locale,
                        gui::GUI_DESKTOP_START_A_CONTAINER_WITH_DOCKER_AND_IT_WILL_APPEAR_HERE,
                    ),
                    "No matching containers" => message(
                        screen.locale,
                        gui::GUI_DESKTOP_CHANGE_THE_SEARCH_OR_STATE_FILTER,
                    ),
                    "The engine is stopped" => message(
                        screen.locale,
                        gui::GUI_DESKTOP_START_THE_MARSA_ENGINE_FROM_MACHINE,
                    ),
                    _ => "".into(),
                }),
        )
}
