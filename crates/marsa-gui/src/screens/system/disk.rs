//! Storage accounting, exact cleanup preview and explicit space reclaim.

use crate::screens::common::message;
use marsa_client::api::{
    DiskCategory, DiskReclaimRequest, DiskUsageRequest, PruneApplyCapability, PrunePreviewRequest,
};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, FontWeight, ParentElement, Styled, div, px},
    gpui_component::{
        Disableable,
        button::{Button, ButtonVariants},
        checkbox::Checkbox,
    },
};

use crate::screens::common::{ScreenContext, color, panel, row, technical};

use super::{SystemView, machine::title_text};

const CATEGORIES: [DiskCategory; 4] = [
    DiskCategory::Containers,
    DiskCategory::Images,
    DiskCategory::Volumes,
    DiskCategory::BuildCache,
];

impl SystemView {
    pub(super) fn load_disk(&mut self, cx: &Context<Self>) {
        let Some(services) = self.services.as_ref() else {
            return;
        };
        let mut pending = services.request(|client| async move {
            Ok(client.disk().usage(DiskUsageRequest {}).await?.into_inner())
        });
        self.route_load = Some(cx.spawn(async move |this, cx| {
            let result = pending.result().await;
            let _ = this.update(cx, |this, cx| {
                this.disk = result.and_then(Result::ok);
                cx.notify();
            });
        }));
    }

    fn preview_cleanup(&mut self, cx: &mut Context<Self>) {
        let categories = self
            .cleanup_categories
            .iter()
            .zip(CATEGORIES)
            .filter_map(|(enabled, category)| enabled.then_some(category as i32))
            .collect();
        self.request(
            move |client| async move {
                Ok(client
                    .disk()
                    .prune_preview(PrunePreviewRequest {
                        categories,
                        volume_names: Vec::new(),
                    })
                    .await?
                    .into_inner())
            },
            |this, preview, _| this.preview = Some(preview),
            cx,
        );
    }

    fn reclaim(&mut self, cx: &mut Context<Self>) {
        let Some(request_id) = self.mutation_id(cx) else {
            return;
        };
        self.request(
            move |client| async move {
                Ok(client
                    .disk()
                    .reclaim(DiskReclaimRequest { request_id })
                    .await?
                    .into_inner())
            },
            |this, result, _| {
                this.reclaimed = Some(result.host_reclaimed_bytes);
                this.disk = result.usage;
            },
            cx,
        );
    }

    pub(super) fn disk(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let disk = self.disk.as_ref();
        let connected = screen.state.daemon().is_some();
        let ready = screen.state.engine().is_some_and(|engine| {
            matches!(
                engine.state(),
                marsa_client::api::EngineState::Ready | marsa_client::api::EngineState::Idle
            )
        });
        let breakdown = disk.map(|usage| {
            let total = usage.categories.iter().fold(0_u64, |sum, category| {
                sum.saturating_add(category.used_bytes)
            });
            div()
                .flex()
                .flex_col()
                .gap_3()
                .children((total > 0).then(|| {
                    row(locale).gap_1().h(px(12.)).children(
                        usage
                            .categories
                            .iter()
                            .filter(|entry| entry.used_bytes > 0)
                            .map(|entry| {
                                div()
                                    .h_full()
                                    .flex_1()
                                    .flex_grow((entry.used_bytes as f64 / total as f64) as f32)
                                    .min_w(px(1.))
                                    .rounded_sm()
                                    .bg(color(screen.palette, category_color(entry.category())))
                            }),
                    )
                }))
                .child(
                    row(locale)
                        .flex_wrap()
                        .gap_3()
                        .children(usage.categories.iter().map(|entry| {
                            let category = entry.category();
                            row(locale)
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(7.))
                                        .rounded_full()
                                        .bg(color(screen.palette, category_color(category))),
                                )
                                .child(category_name(screen, category))
                                .child(technical(&bytes(entry.used_bytes)))
                        })),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                panel(screen)
                    .child(
                        row(locale)
                            .justify_between()
                            .child(
                                div()
                                    .child(title_text(
                                        gui::GUI_DESKTOP_DISK_USAGE.text(screen.locale),
                                    ))
                                    .child(message(
                                        screen.locale,
                                        gui::GUI_DESKTOP_PHYSICAL_SPACE_USED_ON_THIS_MAC,
                                    )),
                            )
                            .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child(
                                disk.map_or_else(
                                    || message(screen.locale, gui::GUI_DESKTOP_UNAVAILABLE),
                                    |usage| technical(&bytes(usage.host_allocated_bytes)),
                                ),
                            )),
                    )
                    .children(breakdown)
                    .children(
                        disk.is_some_and(|usage| !usage.guest_usage_available)
                            .then(|| {
                                message(
                                    screen.locale,
                                    gui::GUI_DESKTOP_START_THE_MACHINE_TO_SEE_GUEST_STORAGE_USAGE,
                                )
                            }),
                    ),
            )
            .child(
                panel(screen)
                    .child(title_text(
                        gui::GUI_DESKTOP_CLEANUP_CANDIDATES.text(screen.locale),
                    ))
                    .child(message(
                        screen.locale,
                        gui::GUI_DESKTOP_CHOOSE_CATEGORIES_THEN_REVIEW_THE_EXACT_RESOURCES,
                    ))
                    .children(CATEGORIES.into_iter().enumerate().map(|(index, category)| {
                        div()
                            .border_t_1()
                            .border_color(color(screen.palette, ColorToken::BorderSubtle))
                            .py_2()
                            .child(
                                Checkbox::new(("disk-category", index))
                                    .checked(self.cleanup_categories[index])
                                    .label(category_name(screen, category))
                                    .disabled(self.busy() || !ready)
                                    .on_click(cx.listener(move |this, value: &bool, _, cx| {
                                        this.cleanup_categories[index] = *value;
                                        this.preview = None;
                                        cx.notify();
                                    })),
                            )
                    }))
                    .child(
                        row(locale).justify_end().child(
                            Button::new("preview-cleanup")
                                .primary()
                                .disabled(
                                    self.busy()
                                        || !ready
                                        || !self.cleanup_categories.contains(&true),
                                )
                                .label(message(screen.locale, gui::GUI_DESKTOP_PREVIEW_CLEANUP))
                                .on_click(cx.listener(|this, _, _, cx| this.preview_cleanup(cx))),
                        ),
                    ),
            )
            .children(self.preview.as_ref().map(|preview| {
                panel(screen)
                    .child(title_text(
                        gui::GUI_DESKTOP_CLEANUP_PREVIEW.text(screen.locale),
                    ))
                    .child(message(
                        locale,
                        match preview.apply_capability() {
                            PruneApplyCapability::ExactDeletionUnsupported => {
                                gui::GUI_DESKTOP_CLEANUP_APPLY_ENGINE_UNSUPPORTED
                            }
                            PruneApplyCapability::Unspecified => {
                                gui::GUI_DESKTOP_CLEANUP_PREVIEW_ONLY
                            }
                        },
                    ))
                    .children(preview.candidates.is_empty().then(|| {
                        message(screen.locale, gui::GUI_DESKTOP_NO_UNUSED_RESOURCES_FOUND)
                    }))
                    .children(preview.candidates.iter().map(|candidate| {
                        row(locale)
                            .justify_between()
                            .gap_3()
                            .child(category_name(screen, candidate.category()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(technical(if candidate.name.is_empty() {
                                        &candidate.id
                                    } else {
                                        &candidate.name
                                    }))
                                    .children(
                                        (!candidate.name.is_empty()
                                            && candidate.name != candidate.id)
                                            .then(|| {
                                                div().text_xs().child(technical(&candidate.id))
                                            }),
                                    ),
                            )
                            .child(candidate.size_bytes.map_or_else(
                                || message(screen.locale, gui::GUI_DESKTOP_UNKNOWN_SIZE),
                                |size| technical(&bytes(size)),
                            ))
                    }))
                    .children(preview.estimated_reclaimable_bytes.map(|size| {
                        row(locale)
                            .gap_2()
                            .child(message(
                                screen.locale,
                                gui::GUI_DESKTOP_ESTIMATED_GUEST_SPACE,
                            ))
                            .child(technical(&bytes(size)))
                    }))
                    .children((!preview.unavailable_categories.is_empty()).then(|| {
                        row(locale)
                            .gap_2()
                            .flex_wrap()
                            .child(message(
                                screen.locale,
                                gui::GUI_DESKTOP_PREVIEW_UNAVAILABLE_FOR,
                            ))
                            .children(
                                preview
                                    .unavailable_categories
                                    .iter()
                                    .filter_map(|category| DiskCategory::try_from(*category).ok())
                                    .map(|category| category_name(screen, category)),
                            )
                    }))
            }))
            .child(
                panel(screen)
                    .child(
                        row(locale)
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .child(title_text(
                                        gui::GUI_DESKTOP_RECLAIM_UNUSED_SPACE.text(screen.locale),
                                    ))
                                    .child(message(
                                        screen.locale,
                                        gui::GUI_DESKTOP_RETURN_UNUSED_BLOCKS_TO_MACOS,
                                    )),
                            )
                            .child(
                                Button::new("reclaim-space")
                                    .outline()
                                    .disabled(self.busy() || !connected || !ready)
                                    .label(message(screen.locale, gui::GUI_DESKTOP_RECLAIM_SPACE))
                                    .on_click(cx.listener(|this, _, _, cx| this.reclaim(cx))),
                            ),
                    )
                    .children(self.reclaimed.map(|size| {
                        row(locale)
                            .gap_2()
                            .child(message(
                                screen.locale,
                                gui::GUI_DESKTOP_SPACE_RETURNED_TO_THE_MAC,
                            ))
                            .child(technical(&bytes(size)))
                    })),
            )
    }
}

fn category_name(
    screen: &ScreenContext<'_>,
    category: DiskCategory,
) -> marsa_ui::gpui::SharedString {
    match category {
        DiskCategory::Containers => message(screen.locale, gui::GUI_DESKTOP_CONTAINERS),
        DiskCategory::Images => message(screen.locale, gui::GUI_DESKTOP_IMAGES),
        DiskCategory::Volumes => message(screen.locale, gui::GUI_DESKTOP_VOLUMES),
        DiskCategory::Networks => message(screen.locale, gui::GUI_DESKTOP_NETWORKS),
        DiskCategory::BuildCache => message(screen.locale, gui::GUI_DESKTOP_BUILD_CACHE),
        DiskCategory::Unspecified => message(screen.locale, gui::GUI_DESKTOP_OTHER),
    }
}

fn category_color(category: DiskCategory) -> ColorToken {
    match category {
        DiskCategory::Images => ColorToken::ChartCpu,
        DiskCategory::Volumes => ColorToken::ChartMemory,
        DiskCategory::BuildCache => ColorToken::ChartNetwork,
        DiskCategory::Containers => ColorToken::ChartDisk,
        _ => ColorToken::StatusStopped,
    }
}

pub(super) fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024. && unit + 1 < UNITS.len() {
        amount /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{amount:.1} {}", UNITS[unit])
    }
}
