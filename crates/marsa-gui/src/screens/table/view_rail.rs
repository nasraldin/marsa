//! List / table view mode segmented control.

use marsa_ui::{
    ColorToken, TextKey,
    gpui::{App, Div, ParentElement as _, Styled as _, px},
    gpui_component::{
        Icon,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
};

use crate::screens::common::{ScreenContext, color, message, row};

/// Compact presentation for the Images resource list.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TableViewMode {
    #[default]
    Table,
    List,
}

impl TableViewMode {
    pub(super) const OPTIONS: [(Self, &str, TextKey, &str); 2] = [
        (
            Self::Table,
            "image-view-table",
            gui::GUI_DESKTOP_GROUPED_TABLE,
            "icons/table-2.svg",
        ),
        (
            Self::List,
            "image-view-list",
            gui::GUI_DESKTOP_LIST,
            "icons/list.svg",
        ),
    ];
}

/// Segmented view switcher with the options supported by the current screen.
pub(crate) fn view_mode_rail<Mode: Copy + PartialEq + 'static>(
    mode: Mode,
    options: &[(Mode, &'static str, TextKey, &'static str)],
    screen: &ScreenContext<'_>,
    on_change: impl Fn(Mode, &mut App) + Clone + 'static,
) -> Div {
    row(screen.locale)
        .gap_1()
        .px_1()
        .items_center()
        .rounded_md()
        .h(px(36.))
        .border_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .children(
            options
                .iter()
                .copied()
                .map(|(target, id, label_key, icon)| {
                    let title = message(screen.locale, label_key);
                    let control = Button::new(id)
                        .size(px(30.))
                        .p_0()
                        .icon(Icon::default().path(icon))
                        .toggled(mode == target)
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
                    let change = on_change.clone();
                    control.on_click(move |_, _, cx| change(target, cx))
                }),
        )
}
