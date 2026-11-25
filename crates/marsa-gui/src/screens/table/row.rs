//! Leading checkbox + cell slots for table rows.

use marsa_ui::{
    ColorToken,
    gpui::{
        App, Div, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
        SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
    },
    gpui_component::checkbox::Checkbox,
    tokens::scale::{Radius, Space},
};

use crate::screens::common::{ScreenContext, color, row};

/// Row chrome with an optional leading checkbox and clickable body.
#[allow(clippy::too_many_arguments)]
pub(crate) fn table_row(
    screen: &ScreenContext<'_>,
    id: impl Into<SharedString>,
    selected: bool,
    checked: bool,
    select_label: SharedString,
    on_toggle: impl Fn(bool, &mut App) + 'static,
    on_open: impl Fn(&mut Window, &mut App) + 'static,
    cells: impl IntoIterator<Item = Div>,
) -> impl IntoElement {
    let id = id.into();
    let background = if checked {
        ColorToken::AccentSubtle
    } else if selected {
        ColorToken::BgSubtle
    } else {
        ColorToken::BgElevated
    };
    let check_id = id.clone();
    row(screen.locale)
        .id((ElementId::from("table-row"), id.clone()))
        .w_full()
        .items_center()
        .gap(px(Space::S8.pixels()))
        .px(px(Space::S12.pixels()))
        .py(px(Space::S8.pixels()))
        .border_b_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .bg(color(screen.palette, background))
        .hover(|style| style.bg(color(screen.palette, ColorToken::BgSubtle)))
        .child(
            Checkbox::new((ElementId::from("select-row"), check_id))
                .checked(checked)
                .accessibility_label(select_label)
                .on_click(move |checked, _, cx| on_toggle(*checked, cx)),
        )
        .child(
            row(screen.locale)
                .id((ElementId::from("table-row-body"), id))
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(Space::S8.pixels()))
                .cursor_pointer()
                .on_click(move |_, window, cx| on_open(window, cx))
                .children(cells),
        )
}

/// Status / tag pill matching the existing State chip style.
pub(crate) fn status_pill(
    screen: &ScreenContext<'_>,
    label: SharedString,
    foreground: ColorToken,
    background: ColorToken,
) -> Div {
    row(screen.locale)
        .gap(px(Space::S8.pixels()))
        .items_center()
        .rounded(px(Radius::Full.pixels()))
        .px(px(Space::S8.pixels()))
        .py(px(Space::S4.pixels()))
        .bg(color(screen.palette, background))
        .child(
            div()
                .size(px(Space::S8.pixels()))
                .rounded_full()
                .bg(color(screen.palette, foreground)),
        )
        .child(
            div()
                .text_sm()
                .text_color(color(screen.palette, foreground))
                .child(label),
        )
}

/// Compact badge without the status dot (tag count / tag name).
pub(crate) fn tag_badge(
    screen: &ScreenContext<'_>,
    label: SharedString,
    foreground: ColorToken,
    background: ColorToken,
) -> Div {
    row(screen.locale)
        .items_center()
        .min_w_0()
        .max_w_full()
        .rounded(px(Radius::Full.pixels()))
        .px(px(Space::S4.pixels()))
        .py(px(2.))
        .bg(color(screen.palette, background))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_xs()
                .font_family("IBM Plex Mono")
                .text_color(color(screen.palette, foreground))
                .child(label),
        )
}

/// Flex cell wrapper matching the Foundations column growth model.
pub(crate) fn cell(flex: f32, child: Div) -> Div {
    div()
        .min_w_0()
        .overflow_hidden()
        .flex_basis(px(0.))
        .flex_grow(flex)
        .child(child)
}
