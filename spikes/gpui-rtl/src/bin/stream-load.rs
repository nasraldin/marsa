//! Exercise the native GPUI frame loop under sustained virtualized list and log load.

#[cfg(target_os = "macos")]
#[path = "stream_load/metrics.rs"]
mod metrics;
#[cfg(target_os = "macos")]
#[path = "stream_load/view.rs"]
mod view;
#[cfg(target_os = "macos")]
#[path = "stream_load/workload.rs"]
mod workload;

#[cfg(target_os = "macos")]
fn main() -> gpui_kit::Result<()> {
    use std::io;

    use gpui_kit::{
        AppContext as _, Bounds, WindowBounds, WindowKind, WindowOptions, point, px, size,
    };

    let mut args = std::env::args().skip(1);
    let seconds = match args.next().as_deref() {
        None => 20,
        Some(value) => value.parse::<usize>()?,
    };
    if !(8..=3600).contains(&seconds) || args.next().is_some() {
        return Err(io::Error::other("usage: stream-load [duration in seconds: 8–3600]").into());
    }

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                fail("load window closed before recording evidence");
            }
        })
        .detach();

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(60.), px(60.)),
                size: size(px(1140.), px(620.)),
            })),
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            ..WindowOptions::default()
        };
        let receiver = match workload::source(seconds) {
            Ok(receiver) => receiver,
            Err(error) => fail(&error.to_string()),
        };
        if let Err(error) = gpui_kit::open_window(options, cx, move |_, cx| {
            cx.new(|_| view::LoadView::new(seconds, receiver))
        }) {
            fail(&error.to_string());
        } else {
            cx.activate(true);
        }
    });

    Ok(())
}

#[cfg(target_os = "macos")]
fn fail(error: &str) -> ! {
    eprintln!("stream-load: {error}");
    std::process::exit(1);
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("This load probe requires macOS Metal.");
    std::process::ExitCode::FAILURE
}
