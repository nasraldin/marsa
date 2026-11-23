//! Scan for local engines and show where the current inventory comes from.

use std::sync::Arc;

use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, ParentElement as _, Styled as _},
    gpui_component::{
        Disableable as _,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{SystemView, settings_card, settings_heading, settings_text};
use crate::screens::{
    common::{ScreenContext, row, technical},
    external_inventory::Environment,
    system::SelectContext,
};

impl SystemView {
    pub(crate) fn refresh_external_inventory(&mut self, cx: &mut Context<Self>) {
        let Some(services) = self.services.as_ref() else {
            return;
        };
        if self.external_inventory_task.is_some() {
            return;
        }
        let mut pending = services.external_inventory();
        self.external_inventory_task = Some(cx.spawn(async move |this, cx| {
            let result = pending.result().await;
            let _ = this.update(cx, |this, cx| {
                if let Some(Ok(inventories)) = result {
                    this.external_inventory = inventories
                        .into_iter()
                        .map(|inventory| Arc::new(inventory.into()))
                        .collect();
                }
                this.external_inventory_checked = true;
                this.external_inventory_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn external_docker(
        &self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let mut card = settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_DESKTOP_CONTEXTS.text(locale),
                gui::GUI_DESKTOP_EXTERNAL_DOCKER_HELP.text(locale),
            ))
            .child(
                row(locale).child(
                    Button::new("refresh-external-docker")
                        .outline()
                        .label(gui::GUI_DESKTOP_SCAN_EXTERNAL_DOCKER.text(locale))
                        .disabled(self.services.is_none() || self.external_inventory_task.is_some())
                        .on_click(
                            cx.listener(|this, _, _, cx| this.refresh_external_inventory(cx)),
                        ),
                ),
            );
        if self.external_inventory_task.is_some()
            || self.external_inventory_checked && self.external_inventory.is_empty()
        {
            let label = if self.external_inventory_task.is_some() {
                gui::GUI_DESKTOP_SCANNING_EXTERNAL_DOCKER
            } else {
                gui::GUI_DESKTOP_NO_EXTERNAL_DOCKER
            };
            card = card.child(settings_text(
                screen,
                Role::Label,
                label.text(locale),
                ColorToken::TextMuted,
            ));
        }
        for (index, inventory) in self.external_inventory.iter().enumerate() {
            let state = if inventory.reachable {
                gui::GUI_DESKTOP_RUNNING
            } else {
                gui::GUI_DESKTOP_UNAVAILABLE
            };
            let selection = Environment::from(inventory.as_ref());
            card = card.child(
                row(locale)
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new(("context-row", index))
                            .ghost()
                            .label(technical(&inventory.source))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SelectContext(selection.clone()))
                            })),
                    )
                    .child(settings_text(
                        screen,
                        Role::Small,
                        technical(&inventory.endpoint.address()),
                        ColorToken::TextMuted,
                    ))
                    .child(settings_text(
                        screen,
                        Role::Small,
                        state.text(locale),
                        if inventory.reachable {
                            ColorToken::StatusRunning
                        } else {
                            ColorToken::TextMuted
                        },
                    )),
            );
        }
        card
    }
}
