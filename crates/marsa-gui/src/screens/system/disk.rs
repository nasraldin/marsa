//! Storage accounting, exact cleanup inventory and explicit space reclaim.

use marsa_ui::tokens::typography::ui_size;

use marsa_client::api::{
    DiskCategory, DiskCategoryUsage, DiskReclaimRequest, DiskUsageRequest, EngineState,
    PruneApplyCapability, PruneCandidate, PrunePreviewRequest, PrunePreviewResponse,
};
use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{Context, Div, FontWeight, ParentElement, Styled, Window, div, px},
    gpui_component::{
        Disableable, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        checkbox::Checkbox,
    },
    gui_keys as gui,
};

use crate::{
    desktop::Route,
    screens::common::{
        ScreenContext, align, color, dialog_actions, dialog_frame, dialog_heading, message, panel,
        row,
    },
};

pub(super) use crate::screens::common::disk_bytes as bytes;

use super::{
    SystemView,
    machine::{mono, muted, system_icon, title_text},
};

const CLEANUP: [DiskCategory; 4] = [
    DiskCategory::Containers,
    DiskCategory::Images,
    DiskCategory::BuildCache,
    DiskCategory::Volumes,
];
const BREAKDOWN: [DiskCategory; 5] = [
    DiskCategory::Images,
    DiskCategory::Volumes,
    DiskCategory::BuildCache,
    DiskCategory::Containers,
    DiskCategory::Networks,
];

impl SystemView {
    pub(super) fn load_disk(&mut self, cx: &Context<Self>) {
        let Some(services) = self.services.as_ref() else {
            return;
        };
        let full_disk_view = self.route == Route::Disk;
        let mut pending = services.request(move |client| async move {
            let usage = client
                .disk()
                .usage(DiskUsageRequest {})
                .await
                .ok()
                .map(|response| response.into_inner());
            let inventory = if full_disk_view {
                client
                    .disk()
                    .prune_preview(PrunePreviewRequest {
                        categories: CLEANUP
                            .into_iter()
                            .map(|category| category as i32)
                            .collect(),
                        volume_names: Vec::new(),
                    })
                    .await
                    .ok()
                    .map(|response| response.into_inner())
            } else {
                None
            };
            Ok((usage, inventory))
        });
        self.route_load = Some(cx.spawn(async move |this, cx| {
            let result = pending.result().await.and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                if let Some((usage, inventory)) = result {
                    this.disk = usage;
                    if full_disk_view {
                        if let Some(inventory) = &inventory {
                            this.cleanup_selection = inventory
                                .candidates
                                .iter()
                                .filter(|item| item.category() != DiskCategory::Volumes)
                                .map(candidate_key)
                                .collect();
                        } else {
                            this.cleanup_selection.clear();
                        }
                        this.preview = inventory;
                    }
                }
                this.route_load = None;
                cx.notify();
            });
        }));
    }

    fn review_cleanup(
        &self,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = &self.preview else { return };
        if window.has_active_dialog(cx) {
            return;
        }
        let candidate_rows = ordered_candidates(preview)
            .filter(|candidate| self.cleanup_selection.contains(&candidate_key(candidate)))
            .cloned()
            .collect::<Vec<_>>();
        if candidate_rows.is_empty() {
            return;
        }
        let selected_bytes = sum_estimates(candidate_rows.iter());
        let applies = match preview.apply_capability() {
            PruneApplyCapability::ExactDeletionUnsupported => {
                gui::GUI_DESKTOP_CLEANUP_APPLY_ENGINE_UNSUPPORTED
            }
            PruneApplyCapability::Unspecified => gui::GUI_DESKTOP_CLEANUP_PREVIEW_ONLY,
        };
        let volume_excluded = candidate_rows
            .iter()
            .all(|candidate| candidate.category() != DiskCategory::Volumes);
        window.open_dialog(cx, move |dialog, _, _| {
            dialog_frame(dialog, 680.)
                .title(dialog_heading(
                    locale,
                    palette,
                    gui::GUI_SYSTEM_DISK_REVIEW.text(locale),
                ))
                .child(
                    align(div().flex().flex_col().gap_4(), locale)
                        .child(muted(
                            palette,
                            gui::GUI_SYSTEM_DISK_REVIEW_EXPLANATION.text(locale),
                        ))
                        .child(cleanup_summary(
                            locale,
                            palette,
                            candidate_rows.len(),
                            selected_bytes,
                        ))
                        .child(
                            div()
                                .rounded(px(8.))
                                .px_4()
                                .py_2()
                                .border_1()
                                .border_color(color(palette, ColorToken::BorderSubtle))
                                .bg(color(palette, ColorToken::BgCode))
                                .children(candidate_rows.iter().enumerate().map(
                                    |(index, candidate)| {
                                        let line = cleanup_row(locale, palette, candidate, true);
                                        if index == 0 { line.border_t_0() } else { line }
                                    },
                                )),
                        )
                        .child(
                            row(locale)
                                .items_start()
                                .gap_3()
                                .p_4()
                                .rounded(px(8.))
                                .bg(color(palette, ColorToken::StatusStartingBg))
                                .child(system_icon(palette, "icons/info.svg"))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .children(volume_excluded.then(|| {
                                            div()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(color(
                                                    palette,
                                                    ColorToken::AccentDefault,
                                                ))
                                                .child(
                                                    gui::GUI_SYSTEM_DISK_VOLUMES_EXCLUDED
                                                        .text(locale),
                                                )
                                        }))
                                        .child(muted(palette, applies.text(locale))),
                                ),
                        ),
                )
                .footer(
                    dialog_actions(locale, palette).child(
                        Button::new("close-cleanup-review")
                            .outline()
                            .small()
                            .label(gui::GUI_DESKTOP_CLOSE.text(locale))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
                )
        });
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
        let palette = screen.palette;
        let disk = self.disk.as_ref();
        let ready = screen
            .state
            .engine()
            .is_some_and(|engine| matches!(engine.state(), EngineState::Ready | EngineState::Idle));
        let inventory = self.preview.as_ref().filter(|_| ready);
        let usage = inventory
            .filter(|preview| !preview.observed_usage.is_empty())
            .map_or_else(
                || disk.map_or(&[][..], |disk| disk.categories.as_slice()),
                |preview| preview.observed_usage.as_slice(),
            );
        let selected = inventory
            .into_iter()
            .flat_map(ordered_candidates)
            .filter(|candidate| self.cleanup_selection.contains(&candidate_key(candidate)));
        let selected_count = selected.clone().count();
        let selected_bytes = sum_estimates(selected);
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                disk_panel(screen)
                    .gap_4()
                    .child(
                        row(locale)
                            .items_start()
                            .justify_between()
                            .gap_3()
                            .flex_wrap()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(title_text(gui::GUI_DESKTOP_DISK_USAGE.text(locale)))
                                    .child(muted(
                                        palette,
                                        gui::GUI_DESKTOP_PHYSICAL_SPACE_USED_ON_THIS_MAC
                                            .text(locale),
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(ui_size(28.))
                                    .line_height(ui_size(36.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(disk.map_or_else(
                                        || message(locale, gui::GUI_DESKTOP_UNAVAILABLE),
                                        |usage| bytes(usage.host_allocated_bytes).into(),
                                    )),
                            ),
                    )
                    .child(disk_chart(locale, palette, usage))
                    .children(
                        disk.is_some_and(|usage| !usage.guest_usage_available)
                            .then(|| {
                                muted(
                                    palette,
                                    gui::GUI_DESKTOP_START_THE_MACHINE_TO_SEE_GUEST_STORAGE_USAGE
                                        .text(locale),
                                )
                            }),
                    ),
            )
            .child(
                disk_panel(screen)
                    .child(
                        row(locale)
                            .justify_between()
                            .gap_3()
                            .flex_wrap()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(title_text(
                                        gui::GUI_DESKTOP_CLEANUP_CANDIDATES.text(locale),
                                    ))
                                    .child(muted(
                                        palette,
                                        gui::GUI_SYSTEM_DISK_SELECT_ITEMS.text(locale),
                                    )),
                            )
                            .children((ready && inventory.is_none()).then(|| {
                                Button::new("reload-disk-inventory")
                                    .outline()
                                    .small()
                                    .disabled(self.route_load.is_some())
                                    .label(gui::GUI_DESKTOP_REFRESH.text(locale))
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.load_disk(cx);
                                        cx.notify();
                                    }))
                            })),
                    )
                    .children(inventory.is_none().then(|| {
                        muted(
                            palette,
                            if screen.state.daemon().is_none() {
                                gui::GUI_SYSTEM_LOCAL_DAEMON_UNAVAILABLE
                            } else if self.route_load.is_some() && ready {
                                gui::GUI_DESKTOP_WORKING
                            } else if ready {
                                gui::GUI_SYSTEM_DISK_INVENTORY_UNAVAILABLE
                            } else {
                                gui::GUI_SYSTEM_DISK_START_FOR_CANDIDATES
                            }
                            .text(locale),
                        )
                    }))
                    .children(
                        inventory
                            .is_some_and(|preview| preview.candidates.is_empty())
                            .then(|| {
                                muted(
                                    palette,
                                    gui::GUI_DESKTOP_NO_UNUSED_RESOURCES_FOUND.text(locale),
                                )
                            }),
                    )
                    .children(
                        inventory
                            .into_iter()
                            .flat_map(ordered_candidates)
                            .map(|candidate| {
                                let key = candidate_key(candidate);
                                let checked = ready && self.cleanup_selection.contains(&key);
                                let id = format!("cleanup-candidate-{}-{}", key.0, key.1);
                                row(locale)
                                    .gap_3()
                                    .border_t_1()
                                    .border_color(color(palette, ColorToken::BorderSubtle))
                                    .min_h(px(52.))
                                    .py_2()
                                    .child(
                                        Checkbox::new(id)
                                            .accessibility_label(crate::screens::common::technical(
                                                if candidate.name.is_empty() {
                                                    &candidate.id
                                                } else {
                                                    &candidate.name
                                                },
                                            ))
                                            .checked(checked)
                                            .disabled(self.busy() || !ready)
                                            .on_click(cx.listener(
                                                move |view, checked: &bool, _, cx| {
                                                    if *checked {
                                                        view.cleanup_selection.insert(key.clone());
                                                    } else {
                                                        view.cleanup_selection.remove(&key);
                                                    }
                                                    cx.notify();
                                                },
                                            )),
                                    )
                                    .child(cleanup_contents(locale, palette, candidate, false))
                            }),
                    )
                    .children(
                        inventory
                            .filter(|preview| !preview.unavailable_categories.is_empty())
                            .map(|preview| {
                                row(locale)
                                    .gap_2()
                                    .flex_wrap()
                                    .child(muted(
                                        palette,
                                        gui::GUI_DESKTOP_PREVIEW_UNAVAILABLE_FOR.text(locale),
                                    ))
                                    .children(
                                        preview
                                            .unavailable_categories
                                            .iter()
                                            .filter_map(|category| {
                                                DiskCategory::try_from(*category).ok()
                                            })
                                            .map(|category| {
                                                message(locale, category_text(category))
                                            }),
                                    )
                            }),
                    )
                    .children(
                        inventory
                            .is_some_and(|preview| !preview.candidates.is_empty())
                            .then(|| {
                                row(locale)
                                    .justify_between()
                                    .gap_3()
                                    .flex_wrap()
                                    .pt_3()
                                    .border_t_1()
                                    .border_color(color(palette, ColorToken::BorderSubtle))
                                    .child(cleanup_summary(
                                        locale,
                                        palette,
                                        selected_count,
                                        selected_bytes,
                                    ))
                                    .child(
                                        Button::new("preview-cleanup")
                                            .primary()
                                            .disabled(self.busy() || !ready || selected_count == 0)
                                            .label(gui::GUI_DESKTOP_PREVIEW_CLEANUP.text(locale))
                                            .on_click(cx.listener(move |view, _, window, cx| {
                                                view.review_cleanup(locale, palette, window, cx)
                                            })),
                                    )
                            }),
                    ),
            )
            .child(
                disk_panel(screen)
                    .child(
                        row(locale)
                            .justify_between()
                            .gap_3()
                            .flex_wrap()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(title_text(
                                        gui::GUI_DESKTOP_RECLAIM_UNUSED_SPACE.text(locale),
                                    ))
                                    .child(muted(
                                        palette,
                                        gui::GUI_DESKTOP_RETURN_UNUSED_BLOCKS_TO_MACOS.text(locale),
                                    )),
                            )
                            .child(
                                Button::new("reclaim-space")
                                    .outline()
                                    .disabled(self.busy() || !ready)
                                    .label(gui::GUI_DESKTOP_RECLAIM_SPACE.text(locale))
                                    .on_click(cx.listener(|view, _, _, cx| view.reclaim(cx))),
                            ),
                    )
                    .children(self.reclaimed.map(|size| {
                        row(locale)
                            .gap_2()
                            .child(muted(
                                palette,
                                gui::GUI_DESKTOP_SPACE_RETURNED_TO_THE_MAC.text(locale),
                            ))
                            .child(mono(&bytes(size)))
                    })),
            )
    }
}

fn disk_panel(screen: &ScreenContext<'_>) -> Div {
    panel(screen).rounded(px(8.)).pt_3()
}

fn disk_chart(locale: Locale, palette: Palette, categories: &[DiskCategoryUsage]) -> Div {
    let amounts = BREAKDOWN
        .into_iter()
        .filter_map(|category| categories.iter().find(|entry| entry.category() == category))
        .collect::<Vec<_>>();
    let total: f64 = amounts.iter().map(|entry| entry.used_bytes as f64).sum();
    let mut segments = row(locale).gap(px(2.)).h(px(16.));
    if total == 0. {
        segments = segments.child(
            div()
                .w_full()
                .h_full()
                .rounded(px(4.))
                .bg(color(palette, ColorToken::BgSubtle)),
        );
    } else {
        segments = segments.children(amounts.iter().filter(|entry| entry.used_bytes > 0).map(
            |entry| {
                div()
                    .h_full()
                    .flex_1()
                    .rounded(px(4.))
                    .flex_grow((entry.used_bytes as f64 / total) as f32)
                    .bg(color(palette, category_color(entry.category())))
            },
        ));
    }
    div()
        .flex()
        .flex_col()
        .gap_5()
        .child(segments)
        .children(amounts.is_empty().then(|| {
            muted(
                palette,
                gui::GUI_SYSTEM_DISK_BREAKDOWN_UNAVAILABLE.text(locale),
            )
        }))
        .children((!amounts.is_empty()).then(|| {
            row(locale)
                .flex_wrap()
                .gap_5()
                .children(amounts.into_iter().map(|entry| {
                    row(locale)
                        .gap_2()
                        .text_size(ui_size(13.))
                        .child(
                            div()
                                .size(px(8.))
                                .rounded_full()
                                .bg(color(palette, category_color(entry.category()))),
                        )
                        .child(category_text(entry.category()).text(locale))
                        .child(
                            mono(&bytes(entry.used_bytes))
                                .text_color(color(palette, ColorToken::TextMuted)),
                        )
                }))
        }))
}

fn ordered_candidates(
    preview: &PrunePreviewResponse,
) -> impl Iterator<Item = &PruneCandidate> + Clone {
    CLEANUP.into_iter().flat_map(move |category| {
        preview
            .candidates
            .iter()
            .filter(move |candidate| candidate.category() == category)
    })
}

fn candidate_key(candidate: &PruneCandidate) -> (i32, String) {
    (candidate.category, candidate.id.clone())
}

fn cleanup_row(locale: Locale, palette: Palette, candidate: &PruneCandidate, exact: bool) -> Div {
    row(locale)
        .min_h(px(52.))
        .py_2()
        .border_t_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
        .child(cleanup_contents(locale, palette, candidate, exact))
}

fn cleanup_contents(
    locale: Locale,
    palette: Palette,
    candidate: &PruneCandidate,
    exact: bool,
) -> Div {
    let category = match candidate.category() {
        DiskCategory::Containers => gui::GUI_SYSTEM_DISK_STOPPED_CONTAINER,
        DiskCategory::Images => gui::GUI_SYSTEM_DISK_UNUSED_IMAGE,
        DiskCategory::Volumes => gui::GUI_SYSTEM_DISK_UNUSED_VOLUME,
        DiskCategory::BuildCache => gui::GUI_SYSTEM_DISK_CACHE_RECORD,
        category => category_text(category),
    };
    let name = if candidate.name.is_empty() {
        &candidate.id
    } else {
        &candidate.name
    };
    row(locale)
        .flex_1()
        .gap_3()
        .text_size(ui_size(13.))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(mono(name))
                .child(muted(palette, category.text(locale)))
                .children((exact && name != &candidate.id).then(|| {
                    mono(&candidate.id).text_color(color(palette, ColorToken::TextMuted))
                })),
        )
        .child(
            div()
                .w(px(104.))
                .flex_shrink_0()
                .child(candidate.size_bytes.map_or_else(
                    || muted(palette, gui::GUI_DESKTOP_UNKNOWN_SIZE.text(locale)),
                    |size| mono(&bytes(size)).text_color(color(palette, ColorToken::TextMuted)),
                )),
        )
}

fn sum_estimates<'a>(mut items: impl Iterator<Item = &'a PruneCandidate>) -> Option<u64> {
    items.try_fold(0_u64, |total, candidate| {
        total.checked_add(candidate.size_bytes?)
    })
}

fn cleanup_summary(locale: Locale, palette: Palette, count: usize, estimate: Option<u64>) -> Div {
    row(locale)
        .gap_2()
        .text_size(ui_size(13.))
        .text_color(color(palette, ColorToken::TextMuted))
        .child(mono(&count.to_string()))
        .child(gui::GUI_SYSTEM_DISK_ITEMS_SELECTED.text(locale))
        .children(estimate.filter(|_| count > 0).map(|size| {
            row(locale)
                .gap_2()
                .child("·")
                .child(gui::GUI_DESKTOP_ESTIMATED_GUEST_SPACE.text(locale))
                .child(mono(&bytes(size)))
        }))
}

fn category_text(category: DiskCategory) -> marsa_ui::TextKey {
    match category {
        DiskCategory::Containers => gui::GUI_DESKTOP_CONTAINERS,
        DiskCategory::Images => gui::GUI_DESKTOP_IMAGES,
        DiskCategory::Volumes => gui::GUI_DESKTOP_VOLUMES,
        DiskCategory::Networks => gui::GUI_DESKTOP_NETWORKS,
        DiskCategory::BuildCache => gui::GUI_DESKTOP_BUILD_CACHE,
        DiskCategory::Unspecified => gui::GUI_DESKTOP_OTHER,
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
