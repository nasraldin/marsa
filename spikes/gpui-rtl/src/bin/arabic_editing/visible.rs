//! Presents the same Rust-backed AppKit editor for native desktop interaction.

use std::{io, time::Duration};

use gpui_kit::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, Result, Styled as _,
    Window, WindowOptions, div, px, rgb,
};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSAccessibility as _, NSFont, NSTextInputClient as _, NSWritingDirection};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::{grapheme_view::GraphemeTextView, platform};

pub fn run() -> Result<()> {
    gpui_kit::application().run(|cx| {
        if let Err(error) = open(cx) {
            eprintln!("Visible AppKit probe: {error:#}");
            std::process::exit(1);
        }
    });
    Ok(())
}

fn open(cx: &mut App) -> Result<()> {
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(gpui_kit::WindowBounds::Windowed(
                gpui_kit::Bounds::centered(None, gpui_kit::size(px(580.), px(240.)), cx),
            )),
            ..WindowOptions::default()
        },
        |_, cx| cx.new(|_| Root),
    )?;
    let field = handle.update(cx, |_, window, _| attach(window))??;
    cx.activate(true);
    let executor = cx.background_executor().clone();
    cx.spawn(async move |_| {
        let mut previous = String::new();
        loop {
            executor.timer(Duration::from_millis(70)).await;
            let Some(window) = field.window() else {
                break;
            };
            let selection = field.selectedRange();
            let caret = platform::caret_rect(&field, selection.location);
            let input_source = field
                .inputContext()
                .and_then(|context| context.selectedKeyboardInputSource());
            let sample = format!(
                "key={} text={:?} selection={}:{} caret=({:.1},{:.1}) marked={} source={:?}",
                window.isKeyWindow(),
                field.string().to_string(),
                selection.location,
                selection.length,
                caret.origin.x,
                caret.origin.y,
                field.hasMarkedText(),
                input_source.map(|source| source.to_string())
            );
            if sample != previous {
                println!("{sample}");
                previous = sample;
            }
        }
    })
    .detach();
    Ok(())
}

fn attach(window: &mut Window) -> Result<Retained<GraphemeTextView>> {
    window.set_window_title("Marsa AppKit Input Qualification");
    let main =
        MainThreadMarker::new().ok_or_else(|| io::Error::other("AppKit needs the main thread"))?;
    let host = platform::native_view(window)?;
    let native_window = host
        .window()
        .ok_or_else(|| io::Error::other("GPUI NSView has no AppKit window"))?;
    let field = GraphemeTextView::new(
        main,
        NSRect::new(NSPoint::new(30., 70.), NSSize::new(520., 62.)),
    );
    field.setRichText(false);
    field.setEditable(true);
    field.setAccessibilityLabel(Some(&NSString::from_str("محرر النص")));
    field.setFont(Some(&NSFont::systemFontOfSize(32.)));
    field.setBaseWritingDirection(NSWritingDirection::RightToLeft);
    host.addSubview(&field);
    if !native_window.makeFirstResponder(Some(&field)) {
        return Err(io::Error::other("AppKit refused keyboard focus").into());
    }
    Ok(field)
}

struct Root;

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(30.))
            .bg(rgb(0xe8eef4))
            .text_color(rgb(0x172331))
            .child("Marsa · Native text editor / محرر النص")
    }
}
