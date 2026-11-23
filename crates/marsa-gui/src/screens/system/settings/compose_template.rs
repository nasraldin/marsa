//! Edit the YAML used by the new stack dialog.

use marsa_client::compose::DEFAULT_TEMPLATE;
use marsa_ui::{
    ColorToken, Locale,
    direction::{align_to_start, isolate_technical_text},
    gpui::{
        AppContext, Context, Div, ParentElement, PromptButton, PromptLevel, Styled, Window, div, px,
    },
    gpui_component::{
        Disableable, WindowExt,
        button::{Button, ButtonVariants},
        input::{Editor, EditorState, InputEvent},
    },
    gui_keys as gui,
    preferences::valid_compose_template,
    tokens::{scale::Space, typography::Role},
};

use crate::screens::common::{ScreenContext, color, row};

use super::{Notice, SystemView, settings_card, settings_heading, settings_text};

impl SystemView {
    pub(super) fn compose_template_settings(
        &self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let custom = self
            .settings
            .as_ref()
            .is_some_and(|(settings, _)| settings.ui.compose_template.is_some());
        let template = self.compose_template();
        let busy = self.busy() || template.is_none();
        settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_COMPOSE_TEMPLATE_TITLE.text(locale),
                gui::GUI_COMPOSE_TEMPLATE_HELP.text(locale),
            ))
            .children(template.map(|value| {
                div()
                    .flex()
                    .flex_col()
                    .p(px(Space::S12.pixels()))
                    .rounded_md()
                    .bg(color(screen.palette, ColorToken::BgCode))
                    .children(value.lines().take(8).map(|line| {
                        align_to_start(Role::Mono.latin().apply(div()), Locale::EnglishUs)
                            .text_color(color(screen.palette, ColorToken::TextPrimary))
                            .child(isolate_technical_text(line))
                            .truncate()
                    }))
                    .children(
                        (value.lines().count() > 8)
                            .then(|| settings_text(screen, Role::Mono, "…", ColorToken::TextMuted)),
                    )
            }))
            .child(
                row(locale)
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("edit-compose-template")
                            .primary()
                            .label(gui::GUI_COMPOSE_TEMPLATE_EDIT.text(locale))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_compose_template(locale, window, cx);
                            })),
                    )
                    .child(
                        Button::new("restore-compose-template")
                            .outline()
                            .label(gui::GUI_COMPOSE_TEMPLATE_RESTORE.text(locale))
                            .disabled(busy || !custom)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.restore_compose_template(locale, window, cx);
                            })),
                    ),
            )
    }

    fn edit_compose_template(&self, locale: Locale, window: &mut Window, cx: &mut Context<Self>) {
        let Some(original) = self
            .compose_template()
            .filter(|_| !self.busy())
            .map(str::to_owned)
        else {
            return;
        };
        let editor = cx.new(|cx| EditorState::new(window, cx).default_value(original.clone()));
        cx.subscribe(&editor, |_, _, _: &InputEvent, cx| cx.refresh_windows())
            .detach();
        let view = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let template = editor.read(cx).value();
            let valid = valid_compose_template(&template);
            let save = view.clone();
            let original_at_open = original.clone();
            dialog
                .width(px(620.))
                .overlay_closable(false)
                .title(gui::GUI_COMPOSE_TEMPLATE_TITLE.text(locale))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            Editor::new(&editor)
                                .h(px(280.))
                                .aria_label(gui::GUI_COMPOSE_YAML.text(locale)),
                        )
                        .children((!valid).then(|| gui::GUI_COMPOSE_TEMPLATE_INVALID.text(locale)))
                        .child(
                            row(locale)
                                .gap_2()
                                .justify_end()
                                .child(
                                    Button::new("cancel-compose-template-edit")
                                        .outline()
                                        .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("save-compose-template")
                                        .primary()
                                        .label(gui::GUI_COMPOSE_TEMPLATE_SAVE.text(locale))
                                        .disabled(!valid || template.as_ref() == original)
                                        .on_click(move |_, window, cx| {
                                            let started = save.update(cx, |this, cx| {
                                                if this.busy() {
                                                    return false;
                                                }
                                                if this.compose_template()
                                                    != Some(original_at_open.as_str())
                                                {
                                                    this.notice = Some(Notice::Failed);
                                                    cx.notify();
                                                    return true;
                                                }
                                                let new = (template.as_ref() != DEFAULT_TEMPLATE)
                                                    .then(|| template.to_string());
                                                this.save_settings(
                                                    |settings| settings.ui.compose_template = new,
                                                    cx,
                                                );
                                                true
                                            });
                                            if matches!(started, Ok(true)) {
                                                window.close_dialog(cx);
                                            }
                                        }),
                                ),
                        ),
                )
        });
    }

    fn restore_compose_template(
        &self,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((settings, _)) = self.settings.as_ref().filter(|_| !self.busy()) else {
            return;
        };
        let Some(original) = settings.ui.compose_template.clone() else {
            return;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            gui::GUI_COMPOSE_TEMPLATE_RESTORE.text(locale),
            Some(gui::GUI_COMPOSE_TEMPLATE_RESTORE_HELP.text(locale)),
            &[
                PromptButton::Ok(gui::GUI_COMPOSE_TEMPLATE_RESTORE.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| {
                    if this.compose_template() != Some(original.as_str()) {
                        this.notice = Some(Notice::Failed);
                        cx.notify();
                        return;
                    }
                    this.save_settings(|settings| settings.ui.compose_template = None, cx);
                });
            }
        })
        .detach();
    }
}
