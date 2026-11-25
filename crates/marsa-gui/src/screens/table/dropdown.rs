//! Reusable filter and sort dropdowns for resource tables.

use marsa_ui::{
    ColorToken, Locale,
    gpui::{App, IntoElement, ParentElement as _, SharedString, Styled as _, px, svg},
    gpui_component::{
        Icon, IconName, Side,
        button::Button,
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
};

use crate::screens::common::{ScreenContext, color, message, row};

/// One option in a filter dropdown, with a tinted icon.
#[derive(Clone)]
pub(crate) struct FilterMenuItem {
    pub id: &'static str,
    pub label: SharedString,
    pub icon: &'static str,
    pub icon_color: ColorToken,
    pub selected: bool,
}

/// Sortable field shown in the Sort by section.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SortField {
    Name,
    Tags,
    Size,
    Age,
    State,
}

impl SortField {
    pub(crate) const ALL: [Self; 5] = [Self::Name, Self::Tags, Self::Size, Self::Age, Self::State];

    pub(crate) fn label(self, locale: Locale) -> SharedString {
        message(
            locale,
            match self {
                Self::Name => gui::GUI_DESKTOP_IMAGE,
                Self::Tags => gui::GUI_DESKTOP_TAGS,
                Self::Size => gui::GUI_DESKTOP_SIZE,
                Self::Age => gui::GUI_DESKTOP_AGE,
                Self::State => gui::GUI_DESKTOP_STATE,
            },
        )
    }
}

/// Ascending / descending (Newest / Oldest for time and size fields).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SortOrder {
    Ascending,
    #[default]
    Descending,
}

impl SortOrder {
    pub(crate) fn toggle(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    /// Short Asc / Des label used in combined menu items.
    pub(crate) fn short(self, locale: Locale) -> SharedString {
        message(
            locale,
            match self {
                Self::Ascending => gui::GUI_DESKTOP_ASC,
                Self::Descending => gui::GUI_DESKTOP_DES,
            },
        )
    }
}

/// Combined sort state for header clicks and the sort menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SortMenuState {
    pub field: SortField,
    pub order: SortOrder,
}

impl Default for SortMenuState {
    fn default() -> Self {
        Self {
            field: SortField::Age,
            order: SortOrder::Descending,
        }
    }
}

impl SortMenuState {
    pub(crate) fn click_header(mut self, field: SortField) -> Self {
        if self.field == field {
            self.order = self.order.toggle();
        } else {
            self.field = field;
            self.order = match field {
                SortField::Age | SortField::Size => SortOrder::Descending,
                _ => SortOrder::Ascending,
            };
        }
        self
    }

    /// Trigger / item caption, for example `Tags (Des)`.
    pub(crate) fn caption(self, locale: Locale) -> SharedString {
        SharedString::from(format!(
            "{} ({})",
            self.field.label(locale),
            self.order.short(locale)
        ))
    }
}

/// Outline filter trigger with per-item icons.
pub(crate) fn filter_dropdown(
    id: &'static str,
    screen: &ScreenContext<'_>,
    label: SharedString,
    trigger_icon: (&'static str, ColorToken),
    a11y: SharedString,
    items: Vec<FilterMenuItem>,
    on_select: impl Fn(&'static str, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let palette = screen.palette;
    let (icon, icon_color) = trigger_icon;
    Button::new(id)
        .outline()
        .h(px(36.))
        .accessibility_label(a11y)
        .child(
            row(screen.locale)
                .gap_2()
                .items_center()
                .child(
                    Icon::default()
                        .path(icon)
                        .size(px(16.))
                        .text_color(color(palette, icon_color)),
                )
                .child(label)
                .child(Icon::new(IconName::ChevronDown).size(px(14.))),
        )
        .dropdown_menu(move |menu, _, _| {
            // Keep icons on the start; put the check on the end so "All" still
            // shows its glyph like every other row.
            let menu = menu.check_side(Side::Right);
            let items = items.clone();
            items.into_iter().fold(menu, |menu, item| {
                let select = on_select.clone();
                let id = item.id;
                menu.item(
                    PopupMenuItem::new(item.label)
                        .icon(
                            Icon::default()
                                .path(item.icon)
                                .text_color(color(palette, item.icon_color)),
                        )
                        .checked(item.selected)
                        .on_click(move |_, _, cx| select(id, cx)),
                )
            })
        })
}

/// Sort menu: field names once, then separate Asc / Des. Trigger shows `Age (Des)`.
pub(crate) fn sort_dropdown(
    id: &'static str,
    screen: &ScreenContext<'_>,
    state: SortMenuState,
    on_change: impl Fn(SortMenuState, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let locale = screen.locale;
    Button::new(id)
        .outline()
        .h(px(36.))
        .accessibility_label(message(locale, gui::GUI_DESKTOP_SORT_BY))
        .child(
            row(locale)
                .gap_2()
                .child(
                    svg()
                        .path("icons/arrow-down-narrow-wide.svg")
                        .size(px(16.))
                        .text_color(color(screen.palette, ColorToken::TextMuted)),
                )
                .child(state.caption(locale))
                .child(Icon::new(IconName::ChevronDown).size(px(14.))),
        )
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu.item(PopupMenuItem::label(message(
                locale,
                gui::GUI_DESKTOP_SORT_BY,
            )));
            for field in SortField::ALL {
                let change = on_change.clone();
                let next = SortMenuState {
                    field,
                    order: state.order,
                };
                menu = menu.item(
                    PopupMenuItem::new(field.label(locale))
                        .checked(state.field == field)
                        .on_click(move |_, _, cx| change(next, cx)),
                );
            }
            menu = menu.separator();
            menu = menu.item(PopupMenuItem::label(message(
                locale,
                gui::GUI_DESKTOP_ORDER_BY,
            )));
            for order in [SortOrder::Ascending, SortOrder::Descending] {
                let change = on_change.clone();
                let next = SortMenuState {
                    field: state.field,
                    order,
                };
                menu = menu.item(
                    PopupMenuItem::new(order.short(locale))
                        .checked(state.order == order)
                        .on_click(move |_, _, cx| change(next, cx)),
                );
            }
            menu
        })
}
