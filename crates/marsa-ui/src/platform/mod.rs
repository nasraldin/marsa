//! Native window lifecycle and app-owned menu actions.

use std::{cell::Cell, process::ExitCode, rc::Rc};

use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, FocusHandle, Global, InteractiveElement,
    IntoElement, KeyBinding, Menu, MenuItem, ParentElement, QuitMode, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, point, px, size,
};
use gpui_component::{Root, Theme};
use marsa_l10n::{Locale, keys};

actions!(marsa, [CloseWindow, QuitMarsa]);

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

type OpenWindow = Rc<dyn Fn(&mut App) -> gpui::Result<()>>;

struct Launcher(OpenWindow);

impl Global for Launcher {}

/// Open or activate the workspace, including after its last window was closed.
pub fn show_workspace(cx: &mut App) -> gpui::Result<()> {
    if let Some(handle) = cx.windows().into_iter().next() {
        cx.update_window(handle, |_, window, cx| {
            window.activate_window();
            cx.activate(true);
        })
    } else if let Some(launcher) = cx
        .try_global::<Launcher>()
        .map(|launcher| launcher.0.clone())
    {
        launcher(cx)
    } else {
        Ok(())
    }
}

/// Runs the desktop and keeps its notification observer alive until the app quits.
pub fn run<T: Render>(
    create: impl Fn(&mut Window, &mut App, Locale) -> Entity<T> + 'static,
) -> ExitCode {
    let locale = system_locale();
    let result = Rc::new(Cell::new(ExitCode::SUCCESS));
    let launch_result = Rc::clone(&result);
    let application = Application::with_platform(Rc::new(gpui_macos::MacPlatform::new(false)))
        .with_assets(super::assets::MarsaAssets)
        .with_quit_mode(QuitMode::Explicit);
    application.on_reopen(|cx| {
        if let Err(error) = show_workspace(cx) {
            eprintln!("{error}");
        }
    });
    application.run(move |cx| {
        if let Err(error) = crate::assets::register_fonts(cx) {
            eprintln!("{error}");
        }
        gpui_component::init(cx);
        crate::direction::init(locale, cx);
        Theme::sync_system_appearance(None, cx);
        cx.bind_keys([
            KeyBinding::new("cmd-w", CloseWindow, None),
            KeyBinding::new("cmd-q", QuitMarsa, None),
        ]);
        cx.on_action(|_: &QuitMarsa, cx| cx.quit());
        cx.set_menus([Menu::new(keys::GUI_WINDOW_TITLE.text(locale)).items([
            MenuItem::action(keys::GUI_WINDOW_CLOSE.text(locale), CloseWindow),
            MenuItem::action(keys::GUI_WINDOW_QUIT.text(locale), QuitMarsa),
        ])]);
        cx.set_global(Launcher(Rc::new(move |cx| {
            launch_window(&create, cx, locale)
        })));
        if let Err(error) = show_workspace(cx) {
            eprintln!("{}", keys::GUI_WINDOW_OPEN_FAILED.text(locale));
            eprintln!("{error}");
            launch_result.set(ExitCode::FAILURE);
            cx.quit();
        }
    });
    result.get()
}

fn launch_window<T: Render>(
    create: &impl Fn(&mut Window, &mut App, Locale) -> Entity<T>,
    cx: &mut App,
    locale: Locale,
) -> gpui::Result<()> {
    let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(800.0), px(560.0))),
        titlebar: Some(TitlebarOptions {
            title: Some(keys::GUI_WINDOW_TITLE.text(locale).into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(24.), px(32.))),
        }),
        ..WindowOptions::default()
    };
    cx.open_window(options, |window, cx| {
        let view = create(window, cx, locale);
        let content = cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            MarsaWindow { focus, view }
        });
        cx.new(|cx| Root::new(content, window, cx))
    })?;
    cx.activate(true);
    Ok(())
}

fn system_locale() -> Locale {
    match sys_locale::get_locale().as_deref() {
        Some(tag) if tag.split('-').next() == Some("ar") => Locale::Arabic,
        _ => Locale::EnglishUs,
    }
}
