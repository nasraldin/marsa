//! GPUI's native macOS entry point and last-window lifecycle.
use std::{cell::Cell, process::ExitCode, rc::Rc};

use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyBinding, Menu, MenuItem, ParentElement, QuitMode, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, px, size,
};
use gpui_component::{Root, Theme};
use marsa_l10n::{Locale, keys};

actions!(marsa, [CloseWindow]);

struct MarsaWindow<T> {
    focus: FocusHandle,
    view: Entity<T>,
}

impl<T: Render> Render for MarsaWindow<T> {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.view.clone())
            .track_focus(&self.focus)
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
    }
}

/// Runs Marsa on the main thread and exits after the final window closes.
pub fn run<T: Render>(
    create: impl FnOnce(&mut Window, &mut App, Locale) -> Entity<T> + 'static,
) -> ExitCode {
    let locale = system_locale();
    let result = Rc::new(Cell::new(ExitCode::SUCCESS));
    let launch_result = Rc::clone(&result);
    Application::with_platform(Rc::new(gpui_macos::MacPlatform::new(false)))
        .with_assets(super::assets::MarsaAssets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx| {
            if let Err(error) = crate::assets::register_fonts(cx) {
                eprintln!("{error}");
            }
            gpui_component::init(cx);
            crate::direction::init(locale, cx);
            Theme::sync_system_appearance(None, cx);
            cx.bind_keys([KeyBinding::new("cmd-w", CloseWindow, None)]);
            cx.set_menus([Menu::new(keys::GUI_WINDOW_TITLE.text(locale)).items([
                MenuItem::action(keys::GUI_WINDOW_CLOSE.text(locale), CloseWindow),
            ])]);
            let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(800.0), px(560.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some(keys::GUI_WINDOW_TITLE.text(locale).into()),
                    ..TitlebarOptions::default()
                }),
                ..WindowOptions::default()
            };
            if let Err(error) = cx.open_window(options, |window, cx| {
                let view = create(window, cx, locale);
                let content = cx.new(|cx| {
                    let focus = cx.focus_handle();
                    focus.focus(window, cx);
                    MarsaWindow { focus, view }
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
