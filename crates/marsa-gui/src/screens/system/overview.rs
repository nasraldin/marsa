//! Workspace overview drawn from live engine and resource snapshots.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::collections::VecDeque;

use marsa_client::api::EngineState;
use marsa_ui::gpui::{Div, FontWeight, ParentElement, Styled, div};

use crate::{
    desktop::status,
    screens::common::{ScreenContext, panel, row, technical},
};

use super::{
    SystemView,
    disk::bytes,
    machine::{metric, title_text},
};

impl SystemView {
    pub(super) fn overview(
        &self,
        screen: &ScreenContext<'_>,
        history: &VecDeque<EngineState>,
    ) -> Div {
        let engine = screen.state.engine();
        let locale = screen.locale;
        let resources = if engine
            .is_some_and(|engine| matches!(engine.state(), EngineState::Ready | EngineState::Idle))
        {
            row(locale)
                .gap_3()
                .child(metric(
                    screen,
                    "Containers",
                    "الحاويات",
                    screen.state.containers().len().to_string(),
                ))
                .child(metric(
                    screen,
                    "Images",
                    "الصور",
                    screen.state.images().len().to_string(),
                ))
                .child(metric(
                    screen,
                    "Volumes",
                    "وحدات التخزين",
                    screen.state.volumes().len().to_string(),
                ))
                .child(metric(
                    screen,
                    "Networks",
                    "الشبكات",
                    screen.state.networks().len().to_string(),
                ))
        } else {
            div().child(message(
                screen.locale,
                gui::GUI_DESKTOP_START_THE_ENGINE_TO_SEE_LOCAL_RESOURCES,
            ))
        };
        let disk = self
            .disk
            .as_ref()
            .map(|disk| bytes(disk.host_allocated_bytes))
            .unwrap_or_else(|| gui::GUI_DESKTOP_UNAVAILABLE.text(locale).to_owned());
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                panel(screen)
                    .child(title_text(
                        gui::GUI_DESKTOP_LOCAL_ENGINE.text(screen.locale),
                    ))
                    .child(row(locale).justify_start().child(status::badge(
                        locale,
                        screen.palette,
                        engine.map(|value| value.state()),
                    )))
                    .child(
                        row(locale)
                            .gap_3()
                            .child(metric(screen, "Docker context", "سياق Docker", "marsa"))
                            .child(metric(
                                screen,
                                "Architecture",
                                "البنية",
                                std::env::consts::ARCH,
                            ))
                            .child(metric(
                                screen,
                                "Runtime",
                                "وقت التشغيل",
                                engine.map_or("—", |value| value.backend.as_str()),
                            ))
                            .child(metric(
                                screen,
                                "Virtual machine",
                                "الجهاز الافتراضي",
                                "Linux",
                            )),
                    ),
            )
            .child(
                row(locale)
                    .items_start()
                    .gap_4()
                    .child(
                        panel(screen)
                            .flex_1()
                            .child(title_text(gui::GUI_DESKTOP_RESOURCES.text(screen.locale)))
                            .child(resources),
                    )
                    .child(
                        panel(screen)
                            .flex_1()
                            .child(title_text(gui::GUI_DESKTOP_DISK_USAGE.text(screen.locale)))
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(technical(&disk)),
                            )
                            .child(message(
                                screen.locale,
                                gui::GUI_DESKTOP_SPACE_USED_ON_THIS_MAC,
                            )),
                    ),
            )
            .children(self.vm_stats.as_ref().map(|sample| {
                panel(screen)
                    .child(title_text(
                        gui::GUI_DESKTOP_VIRTUAL_MACHINE_USAGE.text(locale),
                    ))
                    .child(
                        row(locale)
                            .gap_3()
                            .child(metric(
                                screen,
                                "Memory used",
                                "الذاكرة المستخدمة",
                                format!(
                                    "{} / {}",
                                    bytes(sample.memory_used_bytes),
                                    bytes(sample.memory_total_bytes)
                                ),
                            ))
                            .child(metric(
                                screen,
                                "Guest disk used",
                                "القرص المستخدم في الضيف",
                                format!(
                                    "{} / {}",
                                    bytes(sample.disk_used_bytes),
                                    bytes(sample.disk_total_bytes)
                                ),
                            ))
                            .child(metric(
                                screen,
                                "Received",
                                "المستلم",
                                bytes(sample.network_rx_bytes),
                            ))
                            .child(metric(
                                screen,
                                "Sent",
                                "المرسل",
                                bytes(sample.network_tx_bytes),
                            )),
                    )
            }))
            .child(
                panel(screen)
                    .child(title_text(
                        gui::GUI_DESKTOP_RECENT_ACTIVITY.text(screen.locale),
                    ))
                    .children(history.is_empty().then(|| {
                        message(screen.locale, gui::GUI_DESKTOP_NO_EVENTS_IN_THIS_SESSION)
                    }))
                    .children(history.iter().rev().take(5).map(|state| {
                        row(locale).child(status::badge(locale, screen.palette, Some(*state)))
                    })),
            )
    }
}
