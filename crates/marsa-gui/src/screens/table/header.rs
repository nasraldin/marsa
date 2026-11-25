//! Idle and selection toolbars for resource tables.

use marsa_ui::{
    ColorToken, Locale,
    gpui::{App, Div, ParentElement as _, SharedString, Styled as _, div, px},
    gpui_component::{
        Disableable as _, Icon,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use crate::screens::common::{ScreenContext, color, message, row};

use super::{
    ColumnsPickerState, FilterMenuItem, SortMenuState, TableViewMode, UsageFilter, columns_picker,
    filter_dropdown, sort_dropdown, view_mode_rail,
};

/// Selection summary and bulk actions when rows are checked.
#[derive(Clone, Copy)]
pub(crate) struct SelectionToolbar {
    pub selected: usize,
    pub total: usize,
    pub select_all_checked: bool,
    pub select_all_indeterminate: bool,
}

/// Idle toolbar: usage filter, sort, columns, view rail, reset, trailing actions.
///
/// Select-all lives in the table header column, aligned with row checkboxes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn idle_toolbar(
    screen: &ScreenContext<'_>,
    usage: UsageFilter,
    sort: SortMenuState,
    columns: ColumnsPickerState,
    view_mode: TableViewMode,
    search: Div,
    trailing: Div,
    on_usage: impl Fn(UsageFilter, &mut App) + Clone + 'static,
    on_sort: impl Fn(SortMenuState, &mut App) + Clone + 'static,
    on_columns: impl Fn(ColumnsPickerState, &mut App) + Clone + 'static,
    on_view: impl Fn(TableViewMode, &mut App) + Clone + 'static,
    on_reset: impl Fn(&mut App) + 'static,
) -> Div {
    let locale = screen.locale;
    let defaults = usage == UsageFilter::All
        && sort == SortMenuState::default()
        && columns == ColumnsPickerState::default()
        && view_mode == TableViewMode::default();
    let filter_items = UsageFilter::ALL
        .into_iter()
        .map(|filter| {
            let (icon, icon_color) = filter.icon();
            FilterMenuItem {
                id: match filter {
                    UsageFilter::All => "all",
                    UsageFilter::InUse => "in-use",
                    UsageFilter::SomeUnused => "some-unused",
                    UsageFilter::Unused => "unused",
                },
                label: filter.label(locale),
                icon,
                icon_color,
                selected: filter == usage,
            }
        })
        .collect::<Vec<_>>();
    let reset_label = message(locale, gui::GUI_DESKTOP_RESET_FILTERS);
    row(locale)
        .w_full()
        .justify_between()
        .items_center()
        .flex_wrap()
        .gap_3()
        .flex_shrink_0()
        .child(
            row(locale)
                .items_center()
                .flex_wrap()
                .gap_2()
                .child(search)
                .child(filter_dropdown(
                    "image-usage-filter",
                    screen,
                    usage.label(locale),
                    usage.icon(),
                    message(locale, gui::GUI_DESKTOP_FILTER_IMAGES_BY_USAGE),
                    filter_items,
                    move |id, cx| {
                        let filter = match id {
                            "in-use" => UsageFilter::InUse,
                            "some-unused" => UsageFilter::SomeUnused,
                            "unused" => UsageFilter::Unused,
                            _ => UsageFilter::All,
                        };
                        on_usage(filter, cx);
                    },
                ))
                .child(sort_dropdown("image-sort", screen, sort, on_sort))
                .child(columns_picker("image-columns", screen, columns, on_columns))
                .child(view_mode_rail("image-view", view_mode, screen, on_view))
                .child(
                    Button::new("reset-image-filters")
                        .outline()
                        .size(px(36.))
                        .p_0()
                        .icon(Icon::default().path("icons/rotate-ccw.svg"))
                        .accessibility_label(reset_label.clone())
                        .tooltip(reset_label)
                        .disabled(defaults)
                        .on_click(move |_, _, cx| on_reset(cx)),
                ),
        )
        .child(trailing)
}

/// Replace the idle controls with a selection summary and bulk remove.
pub(crate) fn selection_toolbar(
    screen: &ScreenContext<'_>,
    selection: SelectionToolbar,
    on_clear: impl Fn(&mut App) + 'static,
    bulk_actions: Div,
) -> Div {
    let locale = screen.locale;
    let summary = selected_summary(locale, selection.selected, selection.total);
    row(locale)
        .w_full()
        .justify_between()
        .items_center()
        .flex_wrap()
        .gap_3()
        .flex_shrink_0()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .bg(color(screen.palette, ColorToken::AccentSubtle))
        .child(
            row(locale)
                .items_center()
                .gap_2()
                .child(
                    screen
                        .typography(Role::Small)
                        .apply(div())
                        .font_weight(marsa_ui::gpui::FontWeight::MEDIUM)
                        .child(summary),
                )
                .child(
                    Button::new("clear-image-selection")
                        .ghost()
                        .compact()
                        .label(message(locale, gui::GUI_DESKTOP_CLEAR_SELECTION))
                        .on_click(move |_, _, cx| on_clear(cx)),
                ),
        )
        .child(bulk_actions)
}

fn selected_summary(locale: Locale, selected: usize, total: usize) -> SharedString {
    SharedString::from(match locale {
        Locale::EnglishUs => format!("{selected} of {total} selected"),
        Locale::Arabic => format!("{selected} من {total} محدَّد"),
    })
}
