//! GPUI's native macOS entry point and last-window lifecycle.
use std::{cell::Cell, process::ExitCode, rc::Rc};

use gpui::{
    AppContext, Application, Bounds, Context, FocusHandle, InteractiveElement, IntoElement,
    KeyBinding, Menu, MenuItem, QuitMode, Render, TitlebarOptions, Window, WindowBounds,
    WindowOptions, actions, div, px, size,
};
use gpui_component::{Root, Theme};
use marsa_l10n::{Locale, keys};

actions!(marsa, [CloseWindow]);

struct MarsaWindow {
    focus: FocusHandle,
}

impl Render for MarsaWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
    }
}

/// Runs Marsa on the main thread and exits after the final window closes.
pub fn run() -> ExitCode {
    let locale = system_locale();
    let result = Rc::new(Cell::new(ExitCode::SUCCESS));
    let launch_result = Rc::clone(&result);
    Application::with_platform(Rc::new(gpui_macos::MacPlatform::new(false)))
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx| {
            gpui_component::init(cx);
            Theme::sync_system_appearance(None, cx);
            cx.bind_keys([KeyBinding::new("cmd-w", CloseWindow, None)]);
            cx.set_menus([Menu::new(keys::GUI_WINDOW_TITLE.text(locale)).items([
                MenuItem::action(keys::GUI_WINDOW_CLOSE.text(locale), CloseWindow),
            ])]);
            let bounds = Bounds::centered(None, size(px(960.0), px(660.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(keys::GUI_WINDOW_TITLE.text(locale).into()),
                    ..TitlebarOptions::default()
                }),
                ..WindowOptions::default()
            };
            if let Err(error) = cx.open_window(options, |window, cx| {
                let content = cx.new(|cx| {
                    let focus = cx.focus_handle();
                    focus.focus(window, cx);
                    MarsaWindow { focus }
                });
                cx.new(|cx| Root::new(content, window, cx))
            }) {
                eprintln!("{}", keys::GUI_WINDOW_OPEN_FAILED.text(locale));
                eprintln!("{error}");
                launch_result.set(ExitCode::FAILURE);
                cx.quit();
            } else {
                cx.activate(true);
            }
        });
    result.get()
}

fn system_locale() -> Locale {
    match sys_locale::get_locale().as_deref() {
        Some(tag) if tag.split('-').next() == Some("ar") => Locale::Arabic,
        _ => Locale::EnglishUs,
    }
}
