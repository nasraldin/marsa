//! Bordered table card: toolbar, column headers, scroll body.

use marsa_ui::{
    ColorToken,
    gpui::{Div, ParentElement as _, Styled as _, div, px},
    gpui_component::scroll::ScrollableElement as _,
    tokens::scale::Radius,
};

use crate::screens::common::{ScreenContext, color};

/// Wrap toolbar + header + rows in the shared bordered shell.
pub(crate) fn table_shell(
    screen: &ScreenContext<'_>,
    toolbar: Div,
    column_header: Div,
    body: Div,
    scroll_id: impl Into<marsa_ui::gpui::ElementId>,
) -> Div {
    let border = color(screen.palette, ColorToken::BorderSubtle);
    div()
        .flex()
        .flex_col()
        .w_full()
        .flex_1()
        .min_h_0()
        .gap(px(12.))
        .child(toolbar)
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .flex_1()
                .min_h_0()
                .rounded(px(Radius::R12.pixels()))
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .bg(color(screen.palette, ColorToken::BgElevated))
                .child(
                    column_header
                        .bg(color(screen.palette, ColorToken::BgSubtle))
                        .border_b_1()
                        .border_color(border)
                        .flex_shrink_0(),
                )
                .child(body.overflow_y_scrollbar().id(scroll_id).flex_1().min_h_0()),
        )
}
