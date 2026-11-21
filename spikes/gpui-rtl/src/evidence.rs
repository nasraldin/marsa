//! Captures Metal-rendered frames and tests the native text and input paths.

mod input;
mod text;

use std::{fs, io, path::Path, sync::Arc};

use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, HeadlessAppContext, Result, Window, WindowBounds,
    WindowKind, WindowOptions,
    component::{Theme, ThemeMode},
    platform::{current_headless_renderer, current_platform},
    point, px, size,
    test::TestWindowExt as _,
};

use crate::view::{Language, Spike};

const WIDTH: f32 = 920.;
const HEIGHT: f32 = 710.;

fn options(show: bool) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(80.), px(80.)),
            size: size(px(WIDTH), px(HEIGHT)),
        })),
        kind: WindowKind::Normal,
        focus: show,
        show,
        ..WindowOptions::default()
    }
}

pub fn interactive() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if let Err(error) = gpui_kit::open_window(options(true), cx, |window, cx| {
            cx.new(|cx| Spike::new(window, cx))
        }) {
            eprintln!("Could not open RTL spike window: {error}");
            cx.quit();
        } else {
            cx.activate(true);
        }
    });
}

pub fn capture(destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    let mut cx = HeadlessAppContext::with_platform(
        current_platform(true).text_system(),
        Arc::new(()),
        current_headless_renderer,
    );
    cx.update(gpui_kit::init);

    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(options(false), cx, |window, cx| {
            cx.new(|cx| Spike::new(window, cx))
        })
    })?;

    let en = frame(&mut cx, handle, destination, "en-light", ThemeMode::Light)?;
    let en_dark = frame(&mut cx, handle, destination, "en-dark", ThemeMode::Dark)?;
    cx.update_window(handle, |_, window, cx| window.click("language-toggle", cx))?;
    let ar = frame(&mut cx, handle, destination, "ar-light", ThemeMode::Light)?;
    let ar_dark = frame(&mut cx, handle, destination, "ar-dark", ThemeMode::Dark)?;

    require(
        en.in_order() && en_dark.in_order(),
        "English layout order is incorrect",
    )?;
    require(
        ar.in_order() && ar_dark.in_order(),
        "Arabic layout order is incorrect",
    )?;
    require(
        cx.update(|cx| view.read(cx).language) == Language::Arabic,
        "Language button did not switch the existing window",
    )?;
    println!("English centers: {en:?}");
    println!("Arabic centers: {ar:?}");

    for (variant, mode) in [("ar-light", ThemeMode::Light), ("ar-dark", ThemeMode::Dark)] {
        println!("Arabic shaping and editing in {variant}");
        cx.update_window(handle, |_, window, cx| {
            Theme::change(mode, None, cx);
            window.render_frame(cx);
            text::run(window)?;
            input::probe(&view, window, cx)
        })??;
    }

    cx.update_window(handle, |_, window, cx| window.click("language-toggle", cx))?;
    let switched_back = cx.update(|cx| view.read(cx).language) == Language::English;
    require(
        switched_back,
        "Language button did not switch back to English",
    )?;
    println!("Both directions were rendered by one window and language state.");
    Ok(())
}

#[derive(Debug)]
struct Layout {
    language: Language,
    sidebar: f32,
    content: f32,
    name: f32,
    image: f32,
    path: f32,
}

impl Layout {
    fn read(window: &Window, language: Language) -> Self {
        let x = |id: &'static str| f32::from(window.find(id).bounds().center().x);
        Self {
            language,
            sidebar: x("sidebar"),
            content: x("content"),
            name: x("name"),
            image: x("image"),
            path: x("path"),
        }
    }

    fn in_order(&self) -> bool {
        match self.language {
            Language::English => {
                self.sidebar < self.content && self.name < self.image && self.image < self.path
            }
            Language::Arabic => {
                self.sidebar > self.content && self.name > self.image && self.image > self.path
            }
        }
    }
}

fn frame(
    cx: &mut HeadlessAppContext,
    handle: AnyWindowHandle,
    destination: &Path,
    label: &str,
    mode: ThemeMode,
) -> Result<Layout> {
    let layout = cx.update_window(handle, |_, window, cx| {
        Theme::change(mode, None, cx);
        window.render_frame(cx);
        let direction = gpui_kit::base::LayoutDirection::global(cx);
        let language = if direction.is_rtl() {
            Language::Arabic
        } else {
            Language::English
        };
        Layout::read(window, language)
    })?;
    cx.capture_screenshot(handle)?
        .save(destination.join(format!("{label}.png")))?;
    Ok(layout)
}

fn require(condition: bool, reason: &str) -> Result<()> {
    if !condition {
        return Err(io::Error::other(reason).into());
    }
    Ok(())
}
