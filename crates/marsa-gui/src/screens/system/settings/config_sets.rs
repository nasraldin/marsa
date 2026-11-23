//! Manage the owner's reusable container configurations through revisioned settings.

use marsa_ui::gpui_component::scroll::ScrollableElement as _;

use std::{cell::Cell, rc::Rc};

use marsa_client::docker::container_config::{ContainerConfigSet, MAX_CONFIG_SETS};
use marsa_ui::{
    ColorToken, Locale, Palette,
    direction::LogicalStyle as _,
    gpui::{
        App, Context, Div, InteractiveElement as _, ParentElement as _, PromptButton, PromptLevel,
        Styled as _, Window, div, px,
    },
    gpui_component::{
        Disableable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use crate::screens::{
    common::{ScreenContext, color, resource_input, row},
    config_options::{OptionsEditor, error_line, error_text, field},
};

use super::{SystemView, settings_card, settings_heading, settings_text};

impl SystemView {
    pub(super) fn config_sets_page(
        &self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let palette = screen.palette;
        let sets = self.config_sets();
        let available = self.settings.is_some() && !self.busy();
        let empty = sets.is_empty();
        settings_card(screen)
            .child(
                row(locale)
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .child(settings_heading(
                        screen,
                        gui::GUI_CONFIG_SETS.text(locale),
                        gui::GUI_CONFIG_SETS_HELP.text(locale),
                    ))
                    .child(
                        Button::new("add-container-config-set")
                            .primary()
                            .label(gui::GUI_CONFIG_ADD_SET.text(locale))
                            .disabled(!available || sets.len() >= MAX_CONFIG_SETS)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_config_set(None, locale, palette, window, cx);
                            })),
                    ),
            )
            .children(empty.then(|| {
                settings_text(
                    screen,
                    Role::Body,
                    gui::GUI_CONFIG_NO_SETS.text(locale),
                    ColorToken::TextMuted,
                )
            }))
            .children(sets.iter().enumerate().map(|(index, set)| {
                let edit = set.clone();
                let remove = set.clone();
                row(locale)
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .border_t_1()
                    .border_color(color(palette, ColorToken::BorderSubtle))
                    .py_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(settings_text(
                                screen,
                                Role::Label,
                                set.name.clone(),
                                ColorToken::TextPrimary,
                            ))
                            .children((!set.description.is_empty()).then(|| {
                                settings_text(
                                    screen,
                                    Role::Small,
                                    set.description.clone(),
                                    ColorToken::TextMuted,
                                )
                            })),
                    )
                    .child(
                        row(locale)
                            .gap_2()
                            .child(
                                Button::new(("edit-container-config-set", index))
                                    .outline()
                                    .label(gui::GUI_CONFIG_EDIT_SET.text(locale))
                                    .disabled(!available)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_config_set(
                                            Some(edit.clone()),
                                            locale,
                                            palette,
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(("remove-container-config-set", index))
                                    .outline()
                                    .label(gui::GUI_DESKTOP_REMOVE.text(locale))
                                    .disabled(!available)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_config_set(remove.clone(), locale, window, cx);
                                    })),
                            ),
                    )
            }))
    }

    fn open_config_set(
        &mut self,
        old: Option<ContainerConfigSet>,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() || self.settings.is_none() || window.has_active_dialog(cx) {
            return;
        }
        let title = match old {
            Some(_) => gui::GUI_CONFIG_EDIT_SET,
            None => gui::GUI_CONFIG_ADD_SET,
        };
        let name = field(
            gui::GUI_CONFIG_NAME.text(locale),
            old.as_ref().map_or("", |set| &set.name),
            window,
            cx,
        );
        let description = field(
            gui::GUI_CONFIG_DESCRIPTION.text(locale),
            old.as_ref().map_or("", |set| &set.description),
            window,
            cx,
        );
        let defaults = Default::default();
        let draft = OptionsEditor::editable(
            old.as_ref().map_or(&defaults, |set| &set.options),
            locale,
            window,
            cx,
        );
        let owner = cx.weak_entity();
        let failed = Rc::new(Cell::new(None::<&'static str>));
        let submit = Rc::new({
            let name = name.clone();
            let description = description.clone();
            let draft = draft.clone();
            let failed = failed.clone();
            move |window: &mut Window, cx: &mut App| {
                let form = draft
                    .borrow()
                    .read(cx)
                    .map(|options| ContainerConfigSet {
                        name: name.read(cx).value().trim().into(),
                        description: description.read(cx).value().to_string(),
                        options,
                    })
                    .and_then(|form| form.validate().map(|()| form))
                    .map_err(|error| error_text(error, locale));
                let result = form.and_then(|form| {
                    owner
                        .update(cx, |this, cx| {
                            this.save_config_set(old.as_ref(), form, locale, cx)
                        })
                        .unwrap_or(Err(gui::GUI_CONFIG_SET_CHANGED.text(locale)))
                });
                failed.set(result.err());
                window.refresh();
                result.is_ok()
            }
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            let submit_on_click = submit.clone();
            let submit_on_ok = submit.clone();
            dialog
                .centered()
                .width(px(600.))
                .title(title.text(locale))
                .child(
                    div()
                        .id("config-set-editor")
                        .max_h(px(460.))
                        .overflow_y_scrollbar()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .px_1()
                        .text_start(cx)
                        .child(resource_input(&name))
                        .child(resource_input(&description))
                        .children(failed.get().map(|text| error_line(text, palette, cx)))
                        .child(OptionsEditor::render(&draft, locale, palette, cx)),
                )
                .on_ok(move |_, window, cx| submit_on_ok(window, cx))
                .footer(
                    row(locale)
                        .justify_row_end()
                        .gap_2()
                        .child(
                            Button::new("cancel-config-set")
                                .outline()
                                .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("save-config-set")
                                .primary()
                                .label(gui::GUI_CONFIG_SAVE_SET.text(locale))
                                .on_click(move |_, window, cx| {
                                    if submit_on_click(window, cx) {
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
    }

    fn save_config_set(
        &mut self,
        old: Option<&ContainerConfigSet>,
        set: ContainerConfigSet,
        locale: Locale,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        if self.busy() || self.settings.is_none() {
            return Err(gui::GUI_CONFIG_SET_CHANGED.text(locale));
        }
        let sets = self.config_sets();
        let index = old.and_then(|old| sets.iter().position(|set| set == old));
        if old.is_some() && index.is_none() {
            return Err(gui::GUI_CONFIG_SET_CHANGED.text(locale));
        }
        if index.is_none() && sets.len() >= MAX_CONFIG_SETS {
            return Err(gui::GUI_CONFIG_TOO_MANY.text(locale));
        }
        if sets.iter().enumerate().any(|(position, entry)| {
            index != Some(position) && entry.name.to_lowercase() == set.name.to_lowercase()
        }) {
            return Err(gui::GUI_CONFIG_DUPLICATE_SET.text(locale));
        }
        self.save_settings(
            move |settings| {
                let sets = &mut settings.ui.container_config_sets;
                match index {
                    Some(index) => sets[index] = set,
                    None => sets.push(set),
                }
            },
            cx,
        );
        Ok(())
    }

    fn remove_config_set(
        &mut self,
        set: ContainerConfigSet,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            return;
        }
        let detail = format!(
            "{}\n\n{}",
            gui::GUI_CONFIG_REMOVE_SET_HELP.text(locale),
            set.name
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            gui::GUI_CONFIG_REMOVE_SET_TITLE.text(locale),
            Some(&detail),
            &[
                PromptButton::Ok(gui::GUI_DESKTOP_REMOVE.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        cx.spawn(async move |view, cx| {
            if answer.await == Ok(0) {
                let _ = view.update(cx, |this, cx| {
                    this.save_settings(
                        move |settings| {
                            settings
                                .ui
                                .container_config_sets
                                .retain(|entry| entry != &set);
                        },
                        cx,
                    );
                });
            }
        })
        .detach();
    }
}
