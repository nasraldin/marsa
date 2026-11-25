//! Language, appearance, sharing and Docker setup layouts from Figma Flows.

use std::path::Path;

use marsa_ui::{
    ColorToken, Locale, Palette, TextKey,
    gpui::{
        Context, Div, ExternalPaths, FontWeight, InteractiveElement as _, ParentElement,
        StatefulInteractiveElement as _, Styled, div, img, px, svg,
    },
    gpui_component::{
        Disableable, IconName,
        button::{Button, ButtonVariants},
        scroll::ScrollableElement as _,
    },
    gui_keys as gui,
    tokens::typography::{Role, ui_size},
};

use crate::screens::{
    common::{ScreenContext, align, color, panel, row, technical},
    theme_picker::{choice_button, selection_check, theme_picker},
};

use super::{Desktop, Setup, Step};

impl Desktop {
    pub(in crate::desktop) fn setup_view(&self, cx: &mut Context<Self>) -> Div {
        let Some(setup) = &self.onboarding else {
            return div();
        };
        let locale = setup.locale;
        let palette = self.palette;
        let screen = ScreenContext {
            locale,
            palette,
            display: self.display,
            state: &self.state,
            services: self.services.as_ref(),
        };
        let (title, help) = setup.step.copy();
        let content = match setup.step {
            Step::Language => language_options(setup, palette, cx),
            Step::Appearance => theme_picker(
                "setup-theme",
                locale,
                palette,
                Some(setup.appearance),
                setup.pending,
                cx,
                Desktop::setup_appearance,
            ),
            Step::Sharing => sharing(setup, &screen, cx),
            Step::Docker => docker(&screen),
        };
        row(locale)
            .size_full()
            .p_2()
            .gap_4()
            .child(step_rail(setup, palette))
            .child(
                row(locale)
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(700.))
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .px_3()
                            .pt(px(64.))
                            .pb_4()
                            .child(
                                div()
                                    .id("setup-content")
                                    .flex_1()
                                    .min_h_0()
                                    .px_3()
                                    .overflow_y_scrollbar()
                                    .flex()
                                    .flex_col()
                                    .gap_4()
                                    .child(
                                        align(div().flex().flex_col().gap_2(), locale)
                                            .child(heading(locale).child(title.text(locale)))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(color(
                                                        palette,
                                                        ColorToken::TextMuted,
                                                    ))
                                                    .child(help.text(locale)),
                                            ),
                                    )
                                    .child(content)
                                    .children(setup.failed.then(|| {
                                        align(
                                            div().text_sm().text_color(color(
                                                palette,
                                                ColorToken::StatusError,
                                            )),
                                            locale,
                                        )
                                        .child(gui::GUI_SETUP_RETRY.text(locale))
                                    })),
                            )
                            .child(setup_footer(setup, palette, cx)),
                    ),
            )
    }
}

impl Step {
    fn copy(self) -> (TextKey, TextKey) {
        match self {
            Self::Language => (gui::GUI_SETUP_CHOOSE_LANGUAGE, gui::GUI_SETUP_LANGUAGE_HELP),
            Self::Appearance => (
                gui::GUI_SETUP_CHOOSE_APPEARANCE,
                gui::GUI_SETUP_APPEARANCE_HELP,
            ),
            Self::Sharing => (gui::GUI_SETUP_CHOOSE_FOLDERS, gui::GUI_SETUP_SHARING_HELP),
            Self::Docker => (gui::GUI_SETUP_DOCKER_TITLE, gui::GUI_SETUP_DOCKER_HELP),
        }
    }

    fn key(self) -> TextKey {
        match self {
            Self::Language => gui::GUI_DESKTOP_SETTINGS_LANGUAGE,
            Self::Appearance => gui::GUI_DESKTOP_SETTINGS_APPEARANCE,
            Self::Sharing => gui::GUI_DESKTOP_SHARED_FOLDERS,
            Self::Docker => gui::GUI_SETUP_DOCKER_TITLE,
        }
    }
}

fn step_rail(setup: &Setup, palette: Palette) -> Div {
    let locale = setup.locale;
    div()
        .h_full()
        .w(px(248.))
        .flex_shrink_0()
        .p(px(12.))
        .rounded(px(16.))
        .border_1()
        .border_color(color(palette, ColorToken::GlassEdge))
        .bg(color(palette, ColorToken::GlassRail))
        .flex()
        .flex_col()
        .gap_3()
        .pt(px(54.))
        .child(
            row(locale)
                .gap_2()
                .px_2()
                .child(img("marsa/harbor.png").size(px(32.)))
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(gui::GUI_DESKTOP_MARSA.text(locale)),
                ),
        )
        .child(
            align(
                div().px_2().text_lg().font_weight(FontWeight::SEMIBOLD),
                locale,
            )
            .child(gui::GUI_SETUP_TITLE.text(locale)),
        )
        .child(div().mt_4().flex().flex_col().gap_2().children(
            Step::ALL.into_iter().enumerate().map(|(index, step)| {
                let active = step == setup.step;
                let done = index < setup.step as usize;
                let index_badge = div()
                    .size(px(28.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .bg(color(
                        palette,
                        if active {
                            ColorToken::AccentDefault
                        } else {
                            ColorToken::BgSubtle
                        },
                    ))
                    .text_color(color(
                        palette,
                        if active {
                            ColorToken::TextOnAccent
                        } else {
                            ColorToken::TextMuted
                        },
                    ))
                    .children(done.then(|| {
                        svg()
                            .path("icons/circle-check.svg")
                            .size(px(16.))
                            .text_color(color(palette, ColorToken::StatusRunning))
                    }))
                    .children((!done).then(|| (index + 1).to_string()));
                let element = row(locale)
                    .gap_3()
                    .h(px(44.))
                    .px_2()
                    .rounded(px(6.))
                    .text_sm()
                    .child(index_badge)
                    .child(step.key().text(locale));
                if active {
                    element
                        .bg(color(palette, ColorToken::AccentSubtle))
                        .text_color(color(palette, ColorToken::AccentDefault))
                } else {
                    element.text_color(color(palette, ColorToken::TextMuted))
                }
            }),
        ))
}

fn language_options(setup: &Setup, palette: Palette, cx: &Context<Desktop>) -> Div {
    div().flex().flex_col().gap(px(18.)).children(
        [
            (
                Locale::EnglishUs,
                gui::GUI_LANGUAGE_ENGLISH_US_AUTONYM,
                gui::GUI_SETUP_ENGLISH_DIRECTION,
            ),
            (
                Locale::Arabic,
                gui::GUI_LANGUAGE_ARABIC_AUTONYM,
                gui::GUI_SETUP_ARABIC_DIRECTION,
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (choice, label, direction))| {
            let selected = setup.locale == choice;
            let label = label.text(choice);
            choice_button(("setup-language", index), selected, palette)
                .w_full()
                .h(px(76.))
                .accessibility_label(label)
                .child(
                    row(choice)
                        .w_full()
                        .gap_3()
                        .child(
                            align(div().flex_1().flex().flex_col().gap_1(), choice)
                                .child(div().font_weight(FontWeight::MEDIUM).child(label))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(color(palette, ColorToken::TextMuted))
                                        .child(direction.text(choice)),
                                ),
                        )
                        .children(selected.then(|| selection_check(palette))),
                )
                .on_click(cx.listener(move |desktop, _, _, cx| desktop.setup_locale(choice, cx)))
        }),
    )
}

fn sharing(setup: &Setup, screen: &ScreenContext<'_>, cx: &Context<Desktop>) -> Div {
    let locale = screen.locale;
    let palette = screen.palette;
    let drop_zone = div()
        .id("setup-share-drop")
        .w_full()
        .rounded(px(12.))
        .border_1()
        .border_dashed()
        .border_color(color(palette, ColorToken::BorderControl))
        .bg(color(palette, ColorToken::BgElevated))
        .px_6()
        .py_8()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .cursor_pointer()
        .hover(|style| style.bg(color(palette, ColorToken::BgHover)))
        .can_drop(|drag, _, _| drag.downcast_ref::<ExternalPaths>().is_some())
        .on_drop(cx.listener(|desktop, paths: &ExternalPaths, _, cx| {
            desktop.add_setup_folders(paths.paths(), cx);
        }))
        .on_click(cx.listener(|desktop, _, _, cx| desktop.choose_setup_folder(cx)))
        .child(
            svg()
                .path("icons/folder-open.svg")
                .size(px(36.))
                .text_color(color(palette, ColorToken::AccentDefault)),
        )
        .child(
            align(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color(palette, ColorToken::TextPrimary)),
                locale,
            )
            .child(gui::GUI_SETUP_DROP_FOLDERS.text(locale)),
        )
        .child(
            align(
                div()
                    .text_sm()
                    .text_color(color(palette, ColorToken::TextMuted)),
                locale,
            )
            .child(gui::GUI_SETUP_DROP_FOLDERS_HINT.text(locale)),
        )
        .child(
            Button::new("setup-choose-folder")
                .outline()
                .mt_1()
                .label(gui::GUI_SETUP_BROWSE_FOLDER.text(locale))
                .disabled(setup.pending)
                .on_click(cx.listener(|desktop, _, _, cx| desktop.choose_setup_folder(cx))),
        );

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(drop_zone)
        .children(setup.folders.is_empty().then(|| {
            align(
                div()
                    .text_sm()
                    .text_color(color(palette, ColorToken::TextMuted)),
                locale,
            )
            .child(gui::GUI_SETUP_NO_FOLDERS.text(locale))
        }))
        .children(setup.folders.iter().enumerate().map(|(index, folder)| {
            let path = folder.clone();
            let name = Path::new(folder)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(folder);
            row(locale)
                .id(("setup-folder", index))
                .gap_3()
                .w_full()
                .px_3()
                .py_3()
                .rounded(px(10.))
                .bg(color(palette, ColorToken::BgSubtle))
                .border_1()
                .border_color(color(palette, ColorToken::BorderSubtle))
                .child(
                    div()
                        .size(px(40.))
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(color(palette, ColorToken::BgElevated))
                        .border_1()
                        .border_color(color(palette, ColorToken::BorderSubtle))
                        .child(
                            svg()
                                .path("icons/folder.svg")
                                .size(px(20.))
                                .text_color(color(palette, ColorToken::AccentDefault)),
                        ),
                )
                .child(
                    align(div().flex_1().min_w_0().flex().flex_col().gap_1(), locale)
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(name.to_owned()),
                        )
                        .child(
                            row(locale)
                                .gap_2()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .font_family("IBM Plex Mono")
                                        .text_color(color(palette, ColorToken::TextMuted))
                                        .child(technical(folder)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(color(palette, ColorToken::StatusRunning))
                                        .child(gui::GUI_SETUP_FOLDER_READY.text(locale)),
                                ),
                        ),
                )
                .child(
                    Button::new(("setup-remove-folder", index))
                        .ghost()
                        .p_0()
                        .size(px(28.))
                        .rounded_full()
                        .child(IconName::Close)
                        .accessibility_label(gui::GUI_DESKTOP_REMOVE.text(locale))
                        .disabled(setup.pending)
                        .on_click(cx.listener(move |desktop, _, _, cx| {
                            if let Some(setup) = &mut desktop.onboarding {
                                setup.folders.remove(&path);
                                cx.notify();
                            }
                        })),
                )
        }))
}

fn docker(screen: &ScreenContext<'_>) -> Div {
    let locale = screen.locale;
    let palette = screen.palette;
    panel(screen)
        .min_h(px(216.))
        .child(
            align(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(ui_size(16.)),
                locale,
            )
            .child(gui::GUI_DESKTOP_DOCKER_CONTEXT.text(locale)),
        )
        .child(
            align(
                div()
                    .text_xs()
                    .text_color(color(palette, ColorToken::TextMuted)),
                locale,
            )
            .child(gui::GUI_SETUP_REGISTER_CONTEXT.text(locale)),
        )
        .child(
            row(locale)
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(color(palette, ColorToken::BorderSubtle))
                        .bg(color(palette, ColorToken::BgSubtle))
                        .p_3()
                        .font_family("IBM Plex Mono")
                        .child(technical("docker context use marsa")),
                )
                .child(
                    Button::new("setup-copy-context-command")
                        .outline()
                        .size(px(42.))
                        .p_0()
                        .accessibility_label(gui::GUI_DESKTOP_COPY_COMMAND.text(locale))
                        .tooltip(gui::GUI_DESKTOP_COPY_COMMAND.text(locale))
                        .child(IconName::Copy)
                        .on_click(|_, _, cx| {
                            cx.write_to_clipboard(marsa_ui::gpui::ClipboardItem::new_string(
                                "docker context use marsa".to_owned(),
                            ));
                        }),
                ),
        )
        .child(
            align(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(color(palette, ColorToken::BorderSubtle)),
                locale,
            )
            .child(
                div()
                    .text_xs()
                    .text_color(color(palette, ColorToken::TextMuted))
                    .child(gui::GUI_SETUP_CLI_PATH_NOTE.text(locale)),
            ),
        )
}

fn setup_footer(setup: &Setup, palette: Palette, cx: &Context<Desktop>) -> Div {
    let locale = setup.locale;
    let primary = match setup.step {
        Step::Sharing if setup.folders.is_empty() => gui::GUI_SETUP_CONTINUE_WITHOUT_SHARING,
        Step::Docker => gui::GUI_SETUP_FINISH,
        _ => gui::GUI_DESKTOP_CONTINUE,
    };
    row(locale)
        .justify_between()
        .gap_3()
        .mt_3()
        .pt_3()
        .flex_wrap()
        .border_t_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
        .child(div().children((setup.step != Step::Language).then(|| {
            Button::new("setup-back")
                .outline()
                .label(gui::GUI_SETUP_BACK.text(locale))
                .disabled(setup.pending)
                .on_click(cx.listener(|desktop, _, _, cx| desktop.setup_step(false, cx)))
        })))
        .child(
            Button::new("setup-next")
                .primary()
                .label(primary.text(locale))
                .disabled(setup.pending)
                .on_click(cx.listener(|desktop, _, _, cx| desktop.setup_step(true, cx))),
        )
}

fn heading(locale: Locale) -> Div {
    let role = match locale {
        Locale::Arabic => Role::Hero.arabic(),
        Locale::EnglishUs => Role::Hero.latin(),
    };
    role.apply(div())
}
