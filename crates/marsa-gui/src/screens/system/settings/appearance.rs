//! Theme, language, typography and clock-format preferences.

use marsa_ui::{
    ColorToken, Locale,
    gpui::{Context, Div, ParentElement as _, Styled as _, div, px, svg},
    gpui_component::{
        Disableable as _,
        button::Button,
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
    preferences::{Language, TerminalFont, TimeFormat, UiFont, UiFontSize},
    tokens::typography::{Family, Role},
};

use super::{SystemView, setting_choices, setting_row, settings_text};
use crate::screens::{
    common::{ScreenContext, color, row},
    theme_picker::theme_picker,
};

const INTERFACE_FONTS: [UiFont; 24] = [
    UiFont::System,
    UiFont::IbmPlex,
    UiFont::Geist,
    UiFont::Inter,
    UiFont::PlusJakartaSans,
    UiFont::DmSans,
    UiFont::Outfit,
    UiFont::SpaceGrotesk,
    UiFont::SofiaSans,
    UiFont::Nunito,
    UiFont::Poppins,
    UiFont::Montserrat,
    UiFont::Raleway,
    UiFont::Manrope,
    UiFont::Roboto,
    UiFont::OpenSans,
    UiFont::Lato,
    UiFont::SourceSans3,
    UiFont::WorkSans,
    UiFont::FiraSans,
    UiFont::JetBrainsMono,
    UiFont::FiraCode,
    UiFont::Quicksand,
    UiFont::Comfortaa,
];

const TERMINAL_FONTS: [TerminalFont; 16] = [
    TerminalFont::System,
    TerminalFont::JetBrainsMono,
    TerminalFont::FiraCode,
    TerminalFont::SourceCodePro,
    TerminalFont::CascadiaCode,
    TerminalFont::IbmPlexMono,
    TerminalFont::RobotoMono,
    TerminalFont::UbuntuMono,
    TerminalFont::SpaceMono,
    TerminalFont::Inconsolata,
    TerminalFont::AnonymousPro,
    TerminalFont::DmMono,
    TerminalFont::RedHatMono,
    TerminalFont::Menlo,
    TerminalFont::Consolas,
    TerminalFont::SfMono,
];

impl SystemView {
    pub(super) fn theme_setting(&self, screen: &ScreenContext<'_>, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .py_2()
            .border_t_1()
            .border_color(color(screen.palette, ColorToken::BorderSubtle))
            .child(settings_text(
                screen,
                Role::Small,
                gui::GUI_SETTINGS_THEME_HELP.text(screen.locale),
                ColorToken::TextMuted,
            ))
            .child(theme_picker(
                "appearance-picker",
                screen.locale,
                screen.palette,
                self.preferences().map(|(_, theme)| theme),
                self.busy(),
                cx,
                |view, theme, cx| view.save_settings(|settings| settings.ui.theme = theme, cx),
            ))
    }

    pub(super) fn language_setting(&self, screen: &ScreenContext<'_>, cx: &Context<Self>) -> Div {
        let locale = screen.locale;
        let language = gui::GUI_DESKTOP_SETTINGS_LANGUAGE.text(locale);
        let selected = self.preferences().map(|(language, _)| language);
        let options = vec![
            MenuOption::plain(Language::System, gui::GUI_DESKTOP_SYSTEM.text(locale)),
            MenuOption::plain(
                Language::English,
                gui::GUI_LANGUAGE_ENGLISH_US_AUTONYM.text(locale),
            ),
            MenuOption::plain(
                Language::Arabic,
                gui::GUI_LANGUAGE_ARABIC_AUTONYM.text(locale),
            ),
        ];
        setting_row(
            screen,
            language,
            gui::GUI_SETTINGS_LANGUAGE_HELP.text(locale),
            preference_menu(
                ("language-picker", language),
                selected,
                options,
                self.busy() || selected.is_none(),
                |view, language, cx| {
                    view.save_settings(|settings| settings.ui.language = language, cx)
                },
                cx,
            ),
        )
    }

    pub(super) fn typography_settings(
        &self,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> [Div; 3] {
        let locale = screen.locale;
        let display = self
            .settings
            .as_ref()
            .map(|(settings, _)| settings.ui.display);
        let disabled = self.busy() || display.is_none();
        let font = gui::GUI_SETTINGS_UI_FONT.text(locale);
        let size = gui::GUI_SETTINGS_UI_FONT_SIZE.text(locale);
        let terminal = gui::GUI_SETTINGS_TERMINAL_FONT.text(locale);
        let interface_options = INTERFACE_FONTS
            .into_iter()
            .map(|preference| {
                let face = Family::interface(preference, locale == Locale::Arabic);
                let name = if preference == UiFont::System {
                    gui::GUI_SETTINGS_UI_FONT_SYSTEM.text(locale)
                } else {
                    face.name()
                };
                MenuOption::with_face(preference, name, face)
            })
            .collect();
        let sizes = [
            (
                UiFontSize::ExtraSmall,
                gui::GUI_SETTINGS_UI_FONT_SIZE_EXTRA_SMALL,
            ),
            (UiFontSize::Small, gui::GUI_SETTINGS_UI_FONT_SIZE_SMALL),
            (UiFontSize::Default, gui::GUI_SETTINGS_UI_FONT_SIZE_DEFAULT),
            (UiFontSize::Medium, gui::GUI_SETTINGS_UI_FONT_SIZE_MEDIUM),
            (UiFontSize::Large, gui::GUI_SETTINGS_UI_FONT_SIZE_LARGE),
            (
                UiFontSize::ExtraLarge,
                gui::GUI_SETTINGS_UI_FONT_SIZE_EXTRA_LARGE,
            ),
        ]
        .into_iter()
        .map(|(preference, label)| MenuOption::plain(preference, label.text(locale)))
        .collect();
        let terminal_options = TERMINAL_FONTS
            .into_iter()
            .filter_map(|preference| {
                let face = Family::terminal(preference);
                let name = if preference == TerminalFont::System {
                    gui::GUI_SETTINGS_TERMINAL_FONT_SYSTEM.text(locale)
                } else {
                    face.name()
                };
                face.available(cx)
                    .then_some(MenuOption::with_face(preference, name, face))
            })
            .collect();
        let terminal_preference = display.map(|display| {
            let preference = display.terminal_font;
            if Family::terminal(preference).available(cx) {
                preference
            } else {
                TerminalFont::IbmPlexMono
            }
        });

        [
            typography_row(
                screen,
                "icons/type.svg",
                font,
                preference_menu(
                    ("ui-font", font),
                    display.map(|display| display.ui_font),
                    interface_options,
                    disabled,
                    |view, font, cx| {
                        view.save_settings(|settings| settings.ui.display.ui_font = font, cx)
                    },
                    cx,
                ),
            ),
            typography_row(
                screen,
                "icons/case-sensitive.svg",
                size,
                preference_menu(
                    ("ui-font-size", size),
                    display.map(|display| display.ui_font_size),
                    sizes,
                    disabled,
                    |view, size, cx| {
                        view.save_settings(|settings| settings.ui.display.ui_font_size = size, cx)
                    },
                    cx,
                ),
            ),
            typography_row(
                screen,
                "icons/terminal.svg",
                terminal,
                preference_menu(
                    ("terminal-font", terminal),
                    terminal_preference,
                    terminal_options,
                    disabled,
                    |view, font, cx| {
                        view.save_settings(|settings| settings.ui.display.terminal_font = font, cx)
                    },
                    cx,
                ),
            ),
        ]
    }

    pub(super) fn time_format_setting(
        &self,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let time = gui::GUI_SETTINGS_TIME_FORMAT.text(locale);
        setting_row(
            screen,
            time,
            gui::GUI_SETTINGS_TIME_FORMAT_DESCRIPTION.text(locale),
            setting_choices(
                ("time-format", time),
                [
                    (
                        TimeFormat::TwelveHour,
                        gui::GUI_SETTINGS_TIME_FORMAT_TWELVE.text(locale),
                    ),
                    (
                        TimeFormat::TwentyFourHour,
                        gui::GUI_SETTINGS_TIME_FORMAT_TWENTY_FOUR.text(locale),
                    ),
                ],
                self.settings
                    .as_ref()
                    .map(|(settings, _)| settings.ui.display.time_format),
                self.busy(),
                |settings, format| settings.ui.display.time_format = format,
                cx,
            ),
        )
    }
}

fn typography_row(
    screen: &ScreenContext<'_>,
    icon: &'static str,
    title: &'static str,
    choice: Div,
) -> Div {
    row(screen.locale)
        .justify_between()
        .items_center()
        .flex_wrap()
        .gap_2()
        .min_h(px(56.))
        .py_2()
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .child(
            row(screen.locale)
                .gap_3()
                .min_w(px(138.))
                .flex_1()
                .items_center()
                .child(
                    svg()
                        .path(icon)
                        .size(px(16.))
                        .flex_shrink_0()
                        .text_color(color(screen.palette, ColorToken::TextMuted)),
                )
                .child(settings_text(
                    screen,
                    Role::Label,
                    title,
                    ColorToken::TextPrimary,
                )),
        )
        .child(choice)
}

#[derive(Clone, Copy)]
struct MenuOption<T> {
    value: T,
    label: &'static str,
    family: Option<Family>,
}

impl<T> MenuOption<T> {
    fn with_face(value: T, label: &'static str, family: Family) -> Self {
        Self {
            value,
            label,
            family: Some(family),
        }
    }

    fn plain(value: T, label: &'static str) -> Self {
        Self {
            value,
            label,
            family: None,
        }
    }
}

fn preference_menu<T: Copy + PartialEq + 'static>(
    (id, label): (&'static str, &'static str),
    selected: Option<T>,
    options: Vec<MenuOption<T>>,
    disabled: bool,
    save: fn(&mut SystemView, T, &mut Context<SystemView>),
    cx: &Context<SystemView>,
) -> Div {
    let view = cx.weak_entity();
    let selected_option = options.iter().find(|option| Some(option.value) == selected);
    let current_label = selected_option.map_or("—", |option| option.label);
    let mut button = Button::new(id)
        .outline()
        .dropdown_caret(true)
        .w_full()
        .label(current_label)
        .accessibility_label(format!("{label} · {current_label}"))
        .disabled(disabled);
    if let Some(family) = selected_option.and_then(|option| option.family) {
        button = button.font(family.font());
    }
    div()
        .w(px(232.))
        .max_w_full()
        .child(button.dropdown_menu(move |menu, window, _| {
            let menu = menu
                .min_w(px(232.))
                .max_h(window.viewport_size().height * 0.5)
                .scrollable(true);
            options.iter().fold(menu, |menu, option| {
                let view = view.clone();
                let value = option.value;
                menu.item(
                    PopupMenuItem::new(option.label)
                        .checked(selected == Some(value))
                        .on_click(move |_, _, cx| {
                            if selected != Some(value) {
                                let _ = view.update(cx, |this, cx| save(this, value, cx));
                            }
                        }),
                )
            })
        }))
}
