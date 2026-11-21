//! Presents the same Rust-backed AppKit editor for native desktop interaction.

use std::{io, time::Duration};

use gpui_kit::component::button::Button;
use gpui_kit::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, Result, Styled as _,
    Window, WindowOptions, div, px, rgb,
};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSAccessibility as _, NSAutoresizingMaskOptions, NSFont, NSTextInputClient as _, NSView,
    NSWritingDirection,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::{grapheme_view::GraphemeTextView, platform};

#[path = "native_arrows.rs"]
mod native_arrows;

pub fn run() -> Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let container = arguments.iter().any(|arg| arg == "--native-container");
    let check_arrows = arguments.iter().any(|arg| arg == "--check-arrows");
    let source = arguments
        .iter()
        .find_map(|arg| arg.strip_prefix("--source="))
        .map(str::to_owned);
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        if let Err(error) = open(cx, container, check_arrows, source) {
            eprintln!("Visible AppKit probe: {error:#}");
            std::process::exit(1);
        }
    });
    Ok(())
}

fn open(cx: &mut App, container: bool, check_arrows: bool, source: Option<String>) -> Result<()> {
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
    let field = handle.update(cx, |_, window, _| attach(window, container))??;
    cx.activate(true);
    let executor = cx.background_executor().clone();
    cx.spawn(async move |_| {
        let mut previous = String::new();
        let mut source = source;
        let mut check_arrows = check_arrows;
        loop {
            executor.timer(Duration::from_millis(70)).await;
            let Some(window) = field.window() else {
                break;
            };
            if window.isKeyWindow() {
                if check_arrows {
                    check_arrows = false;
                    if let Err(error) = native_arrows::verify(&window, &field) {
                        eprintln!("Native arrow probe: {error:#}");
                        std::process::exit(1);
                    }
                }
                if let Some(source) = source.take()
                    && let Some(context) = field.inputContext()
                {
                    context.setSelectedKeyboardInputSource(Some(&NSString::from_str(&source)));
                }
            }
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

fn attach(window: &mut Window, container: bool) -> Result<Retained<GraphemeTextView>> {
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
    if container {
        // Preserve AccessKit's content view and attach the native control as
        // its sibling so macOS can discover both accessibility hierarchies.
        let content = native_window
            .contentView()
            .ok_or_else(|| io::Error::other("GPUI window has no content view"))?;
        let wrapper = NSView::initWithFrame(main.alloc(), content.frame());
        native_window.setContentView(Some(&wrapper));
        content.setFrame(wrapper.bounds());
        content.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        wrapper.addSubview(&content);
        wrapper.addSubview(&field);
    } else {
        host.addSubview(&field);
    }
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
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.))
            .bg(rgb(0xe8eef4))
            .text_color(rgb(0x172331))
            .child("Marsa · Native text editor / محرر النص")
            .child(
                Button::new("probe-button")
                    .label("GPUI control / زر الاختبار")
                    .accessibility_label("GPUI control / زر الاختبار")
                    .tab_stop(true)
                    .on_click(|_, _, _| println!("GPUI accessibility probe button activated.")),
            )
    }
}
