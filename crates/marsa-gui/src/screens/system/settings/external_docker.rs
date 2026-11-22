//! Display external containers and images without exposing Marsa's write actions.

use marsa_ui::{
    ColorToken,
    gpui::{
        Context, Div, InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _,
        Styled as _, div, px,
    },
    gpui_component::{Disableable as _, button::Button},
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{SystemView, settings_card, settings_heading, settings_text};
use crate::screens::common::{ScreenContext, row, technical};

const DISPLAY_LIMIT: usize = 200;

impl SystemView {
    pub(super) fn refresh_external_inventory(&mut self, cx: &mut Context<Self>) {
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
                this.external_inventory = result.and_then(Result::ok).flatten();
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
                gui::GUI_DESKTOP_EXTERNAL_DOCKER.text(locale),
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
            || self.external_inventory_checked && self.external_inventory.is_none()
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
        if let Some(inventory) = &self.external_inventory {
            card = card
                .child(settings_text(
                    screen,
                    Role::Mono,
                    technical(&inventory.socket.to_string_lossy()),
                    ColorToken::TextMuted,
                ))
                .child(
                    row(locale)
                        .items_start()
                        .gap_4()
                        .child(inventory_list(
                            screen,
                            "external-docker-containers",
                            gui::GUI_DESKTOP_EXTERNAL_CONTAINERS.text(locale),
                            inventory.containers.len(),
                            inventory.containers.iter().map(|item| {
                                item.names
                                    .first()
                                    .map_or(item.id.as_str(), |name| name.trim_start_matches('/'))
                            }),
                        ))
                        .child(inventory_list(
                            screen,
                            "external-docker-images",
                            gui::GUI_DESKTOP_EXTERNAL_IMAGES.text(locale),
                            inventory.images.len(),
                            inventory.images.iter().map(|item| {
                                item.repo_tags
                                    .first()
                                    .map_or(item.id.as_str(), String::as_str)
                            }),
                        )),
                );
        }
        card
    }
}

fn inventory_list<'a>(
    screen: &ScreenContext<'_>,
    id: &'static str,
    title: &str,
    count: usize,
    names: impl Iterator<Item = &'a str>,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            row(screen.locale)
                .gap_2()
                .child(settings_text(
                    screen,
                    Role::Label,
                    title.to_owned(),
                    ColorToken::TextPrimary,
                ))
                .child(settings_text(
                    screen,
                    Role::Mono,
                    technical(&count.to_string()),
                    ColorToken::TextMuted,
                )),
        )
        .child(
            div().id(id).max_h(px(160.)).overflow_y_scroll().child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(names.take(DISPLAY_LIMIT).map(|name| {
                        let bounded: String = name.chars().take(160).collect();
                        settings_text(
                            screen,
                            Role::Mono,
                            technical(&bounded),
                            ColorToken::TextPrimary,
                        )
                        .truncate()
                        .px(px(4.))
                    })),
            ),
        )
        .children((count > DISPLAY_LIMIT).then(|| {
            settings_text(
                screen,
                Role::Label,
                gui::GUI_DESKTOP_EXTERNAL_DOCKER_LIST_LIMIT.text(screen.locale),
                ColorToken::TextMuted,
            )
        }))
}
