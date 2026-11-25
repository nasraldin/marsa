//! Expandable image layer stack — rendered inside the image details modal.

use std::collections::BTreeSet;

use marsa_client::docker::ImageHistoryLayer;
use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{
        Div, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
        relative,
    },
    gpui_component::{
        Icon, IconName,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
    tokens::{
        scale::{Radius, Space},
        typography::ui_size,
    },
};

use crate::screens::common::{color, message, row};

use super::ResourcesView;

/// Layer stack body for the image details modal.
pub(super) fn render_layers_section(
    locale: Locale,
    palette: Palette,
    image_id: &str,
    layers: Option<&[ImageHistoryLayer]>,
    failed: bool,
    expanded: &BTreeSet<usize>,
    view: &marsa_ui::gpui::Entity<ResourcesView>,
) -> impl IntoElement {
    let short_id =
        marsa_ui::direction::isolate_technical_text(image_id.get(..12).unwrap_or(image_id));
    let content = if failed {
        div()
            .text_size(ui_size(13.))
            .text_color(color(palette, ColorToken::StatusWarning))
            .child(message(locale, gui::GUI_DESKTOP_IMAGE_LAYERS_UNAVAILABLE))
    } else if let Some(layers) = layers {
        let total_size = layers
            .iter()
            .map(|layer| layer.size.max(0) as u64)
            .sum::<u64>();
        div()
            .flex()
            .flex_col()
            .gap(px(Space::S12.pixels()))
            .w_full()
            .child(summary_card(
                locale,
                palette,
                layers.len(),
                total_size,
                short_id,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Space::S8.pixels()))
                    .w_full()
                    .child(
                        div()
                            .text_size(ui_size(12.))
                            .text_color(color(palette, ColorToken::TextMuted))
                            .child(message(locale, gui::GUI_DESKTOP_LAYER_STACK_HELP)),
                    )
                    .children(layers.iter().enumerate().map(|(index, layer)| {
                        let number = layers.len() - index;
                        layer_row(
                            locale,
                            palette,
                            number,
                            index,
                            layer,
                            total_size,
                            expanded.contains(&index),
                            view.clone(),
                        )
                    })),
            )
    } else {
        div()
            .text_size(ui_size(13.))
            .text_color(color(palette, ColorToken::TextMuted))
            .child(message(locale, gui::GUI_DESKTOP_LOADING_IMAGE_LAYERS))
    };

    // Title lives on the dialog; body is summary + stack only (screenshot layout).
    content
}

fn summary_card(
    locale: Locale,
    palette: Palette,
    count: usize,
    total_size: u64,
    short_id: SharedString,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(12.))
        .border_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
        .bg(color(palette, ColorToken::BgSubtle))
        .child(
            row(locale)
                .w_full()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_size(ui_size(12.)).child(format!(
                            "{}: {count}",
                            gui::GUI_DESKTOP_TOTAL_LAYERS.text(locale)
                        )))
                        .child(div().text_size(ui_size(12.)).child(format!(
                            "{}: {}",
                            gui::GUI_DESKTOP_TOTAL_SIZE.text(locale),
                            format_bytes(locale, total_size)
                        ))),
                )
                .child(
                    div()
                        .font_family("IBM Plex Mono")
                        .text_size(ui_size(12.))
                        .text_color(color(palette, ColorToken::TextMuted))
                        .child(short_id),
                ),
        )
}

#[allow(clippy::too_many_arguments)]
fn layer_row(
    locale: Locale,
    palette: Palette,
    number: usize,
    index: usize,
    layer: &ImageHistoryLayer,
    total_size: u64,
    expanded: bool,
    view: marsa_ui::gpui::Entity<ResourcesView>,
) -> Div {
    let size = layer.size.max(0) as u64;
    let opcode = layer_opcode(&layer.created_by);
    let (badge_color, badge_text) = opcode_colors(opcode.as_ref());
    let fraction = if total_size == 0 {
        0.
    } else {
        (size as f32 / total_size as f32).clamp(0., 1.)
    };
    let bar_fraction = if size == 0 { 0.12 } else { fraction.max(0.16) };
    let large = size >= 50_000_000 || (total_size > 0 && size * 10 >= total_size);
    let status = if large {
        (
            message(locale, gui::GUI_DESKTOP_LAYER_LARGE),
            ColorToken::StatusError,
            ColorToken::StatusErrorBg,
        )
    } else {
        (
            message(locale, gui::GUI_DESKTOP_LAYER_NORMAL),
            ColorToken::TextMuted,
            ColorToken::BgSubtle,
        )
    };

    let header = Button::new(("image-layer-row", index))
        .ghost()
        .w_full()
        .p_0()
        .child(
            row(locale)
                .w_full()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(14.))
                    .text_color(color(palette, ColorToken::TextMuted)),
                )
                .child(
                    div()
                        .w(px(28.))
                        .flex_shrink_0()
                        .text_size(ui_size(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("#{number}")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(28.))
                        .rounded(px(Radius::R8.pixels()))
                        .bg(color(palette, ColorToken::BgSubtle))
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(relative(bar_fraction))
                                .min_w(px(88.))
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded(px(Radius::R8.pixels()))
                                .bg(color(palette, badge_color))
                                .child(
                                    div()
                                        .text_size(ui_size(11.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(color(palette, badge_text))
                                        .truncate()
                                        .child(format!("{} {opcode}", format_bytes(locale, size))),
                                ),
                        ),
                )
                .child(
                    div()
                        .w(px(88.))
                        .flex_shrink_0()
                        .text_size(ui_size(12.))
                        .text_color(color(palette, ColorToken::TextMuted))
                        .child(format_bytes(locale, size)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .px_2()
                        .py_1()
                        .rounded(px(Radius::Full.pixels()))
                        .bg(color(palette, status.2))
                        .child(
                            div()
                                .text_size(ui_size(11.))
                                .text_color(color(palette, status.1))
                                .child(status.0),
                        ),
                ),
        )
        .on_click(move |_, window, cx| {
            view.update(cx, |view, cx| {
                if let Some(detail) = view.image_detail.as_mut()
                    && !detail.layers_expanded.insert(index)
                {
                    detail.layers_expanded.remove(&index);
                }
                cx.refresh_windows();
            });
            window.refresh();
        });

    let mut card = div().flex().flex_col().gap_2().w_full().child(header);
    if expanded {
        card = card.child(layer_details(locale, palette, layer, size));
    }
    card
}

fn layer_details(locale: Locale, palette: Palette, layer: &ImageHistoryLayer, size: u64) -> Div {
    let created = format_layer_created(layer.created);
    let comment = if layer.comment.trim().is_empty() {
        message(locale, gui::GUI_DESKTOP_NONE).to_string()
    } else {
        layer.comment.clone()
    };
    let command = display_command(&layer.created_by);
    row(locale)
        .gap_3()
        .items_start()
        .w_full()
        .child(div().w(px(24.)).flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded(px(12.))
                .border_1()
                .border_color(color(palette, ColorToken::BorderSubtle))
                .bg(color(palette, ColorToken::BgElevated))
                .child(detail_line(
                    locale,
                    palette,
                    gui::GUI_DESKTOP_CREATED.text(locale),
                    created,
                ))
                .child(detail_line(
                    locale,
                    palette,
                    gui::GUI_DESKTOP_SIZE.text(locale),
                    format_bytes(locale, size),
                ))
                .child(command_block(locale, palette, command))
                .child(detail_line(
                    locale,
                    palette,
                    gui::GUI_DESKTOP_COMMENT.text(locale),
                    comment,
                )),
        )
}

fn command_block(locale: Locale, palette: Palette, command: String) -> Div {
    row(locale)
        .gap_3()
        .items_start()
        .w_full()
        .child(
            div()
                .w(px(88.))
                .flex_shrink_0()
                .text_size(ui_size(12.))
                .text_color(color(palette, ColorToken::TextMuted))
                .child(gui::GUI_DESKTOP_COMMAND.text(locale).to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .p_2()
                .rounded(px(Radius::R8.pixels()))
                .bg(color(palette, ColorToken::BgCode))
                .font_family("IBM Plex Mono")
                .text_size(ui_size(12.))
                .child(command),
        )
}

fn detail_line(
    locale: Locale,
    palette: Palette,
    label: &str,
    value: impl Into<SharedString>,
) -> Div {
    row(locale)
        .gap_3()
        .items_start()
        .w_full()
        .child(
            div()
                .w(px(88.))
                .flex_shrink_0()
                .text_size(ui_size(12.))
                .text_color(color(palette, ColorToken::TextMuted))
                .child(label.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family("IBM Plex Mono")
                .text_size(ui_size(12.))
                .child(value.into()),
        )
}

fn layer_opcode(created_by: &str) -> SharedString {
    let rest = display_command(created_by);
    let opcode = rest
        .split_whitespace()
        .next()
        .unwrap_or("LAYER")
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
        .to_ascii_uppercase();
    if opcode.is_empty() {
        "LAYER".into()
    } else {
        opcode.into()
    }
}

fn display_command(created_by: &str) -> String {
    let trimmed = created_by.trim();
    trimmed
        .strip_prefix("/bin/sh -c #(nop) ")
        .or_else(|| trimmed.strip_prefix("/bin/sh -c #(nop)"))
        .or_else(|| trimmed.strip_prefix("/bin/sh -c "))
        .unwrap_or(trimmed)
        .trim()
        .to_owned()
}

fn opcode_colors(opcode: &str) -> (ColorToken, ColorToken) {
    use ColorToken::*;
    let fill = match opcode {
        "ADD" => StatusWarning,
        "CMD" => StatusRunning,
        "RUN" => AccentDefault,
        "COPY" => StatusStarting,
        "USER" => AccentHover,
        "ENTRYPOINT" => AccentPressed,
        _ => return (BgSubtle, TextPrimary),
    };
    (fill, TextOnAccent)
}

fn format_layer_created(created: i64) -> String {
    let Ok(total) = u64::try_from(created) else {
        return created.to_string();
    };
    let Ok(days) = i32::try_from(total / 86_400) else {
        return created.to_string();
    };
    let tod = total % 86_400;
    let hour = tod / 3_600;
    let minute = (tod % 3_600) / 60;
    let (year, month, day) = unix_days_to_ymd(days);
    format!("{day:02}.{month:02}.{year:04} {hour:02}:{minute:02}")
}

fn unix_days_to_ymd(days: i32) -> (i32, u32, u32) {
    let z = days.wrapping_add(719_468);
    let era = if z >= 0 { z } else { z.wrapping_sub(146_096) } / 146_097;
    let doe = z.wrapping_sub(era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn format_bytes(locale: Locale, bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!(
            "{:.2} {}",
            bytes as f64 / 1_000_000_000.,
            gui::GUI_DESKTOP_GB.text(locale)
        )
    } else if bytes >= 1_000_000 {
        format!(
            "{:.2} {}",
            bytes as f64 / 1_000_000.,
            gui::GUI_DESKTOP_MB.text(locale)
        )
    } else if bytes >= 1_000 {
        format!(
            "{:.0} {}",
            bytes as f64 / 1_000.,
            gui::GUI_DESKTOP_KB.text(locale)
        )
    } else {
        format!("{bytes} {}", gui::GUI_DESKTOP_B.text(locale))
    }
}
