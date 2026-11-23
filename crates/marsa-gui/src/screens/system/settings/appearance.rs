//! General settings for the desktop's typography and timestamp display.

use marsa_ui::{
    gpui::{Context, Div},
    gui_keys as gui,
    preferences::{TerminalFont, TimeFormat, UiFont, UiFontSize},
};

use super::{SystemView, setting_choices, setting_row};
use crate::screens::common::ScreenContext;

impl SystemView {
    pub(super) fn appearance_settings(
        &self,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> [Div; 4] {
        let locale = screen.locale;
        let display = self
            .settings
            .as_ref()
            .map(|(settings, _)| settings.ui.display);
        let font = gui::GUI_SETTINGS_UI_FONT.text(locale);
        let size = gui::GUI_SETTINGS_UI_FONT_SIZE.text(locale);
        let terminal = gui::GUI_SETTINGS_TERMINAL_FONT.text(locale);
        let time = gui::GUI_SETTINGS_TIME_FORMAT.text(locale);

        [
            setting_row(
                screen,
                font,
                gui::GUI_SETTINGS_UI_FONT_DESCRIPTION.text(locale),
                setting_choices(
                    ("ui-font", font),
                    [
                        (UiFont::IbmPlex, gui::GUI_SETTINGS_UI_FONT_PLEX.text(locale)),
                        (
                            UiFont::System,
                            gui::GUI_SETTINGS_UI_FONT_SYSTEM.text(locale),
                        ),
                    ],
                    display.map(|display| display.ui_font),
                    self.busy(),
                    |settings, font| settings.ui.display.ui_font = font,
                    cx,
                ),
            ),
            setting_row(
                screen,
                size,
                gui::GUI_SETTINGS_UI_FONT_SIZE_DESCRIPTION.text(locale),
                setting_choices(
                    ("ui-font-size", size),
                    [
                        (
                            UiFontSize::Small,
                            gui::GUI_SETTINGS_UI_FONT_SIZE_SMALL.text(locale),
                        ),
                        (
                            UiFontSize::Default,
                            gui::GUI_SETTINGS_UI_FONT_SIZE_DEFAULT.text(locale),
                        ),
                        (
                            UiFontSize::Large,
                            gui::GUI_SETTINGS_UI_FONT_SIZE_LARGE.text(locale),
                        ),
                    ],
                    display.map(|display| display.ui_font_size),
                    self.busy(),
                    |settings, size| settings.ui.display.ui_font_size = size,
                    cx,
                ),
            ),
            setting_row(
                screen,
                terminal,
                gui::GUI_SETTINGS_TERMINAL_FONT_DESCRIPTION.text(locale),
                setting_choices(
                    ("terminal-font", terminal),
                    [
                        (
                            TerminalFont::IbmPlexMono,
                            gui::GUI_SETTINGS_TERMINAL_FONT_PLEX.text(locale),
                        ),
                        (
                            TerminalFont::Menlo,
                            gui::GUI_SETTINGS_TERMINAL_FONT_MENLO.text(locale),
                        ),
                    ],
                    display.map(|display| display.terminal_font),
                    self.busy(),
                    |settings, font| settings.ui.display.terminal_font = font,
                    cx,
                ),
            ),
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
                    display.map(|display| display.time_format),
                    self.busy(),
                    |settings, format| settings.ui.display.time_format = format,
                    cx,
                ),
            ),
        ]
    }
}
