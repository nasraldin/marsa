//! Verifies that closing the last native test window releases its view and exits.

#[cfg(target_os = "macos")]
fn main() -> gpui_kit::Result<()> {
    use std::{
        cell::Cell,
        io,
        rc::Rc,
        time::{Duration, Instant},
    };

    use gpui_kit::{AppContext as _, Context, IntoElement, Window, WindowOptions, div, prelude::*};

    struct TestView(Rc<Cell<bool>>);

    impl Render for TestView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().child(if gpui_kit::base::LayoutDirection::global(cx).is_rtl() {
                "الحاويات"
            } else {
                "Containers"
            })
        }
    }

    impl Drop for TestView {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    let dropped = Rc::new(Cell::new(false));
    let start = Instant::now();
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(5));
        eprintln!("Native window close did not finish within five seconds.");
        std::process::exit(1);
    });
    gpui_kit::application().run({
        let dropped = dropped.clone();
        let dropped_on_close = dropped.clone();
        move |cx| {
            gpui_kit::init(cx);
            cx.on_window_closed(move |cx, _| {
                if cx.windows().is_empty() {
                    let dropped = dropped_on_close.clone();
                    let executor = cx.background_executor().clone();
                    cx.spawn(async move |cx| {
                        executor.timer(Duration::from_millis(50)).await;
                        if !dropped.get() {
                            eprintln!("Last-window close did not drop the view.");
                            std::process::exit(1);
                        }
                        println!(
                            "Native GPUI close callback fired, view dropped, and process is exiting after {} ms.",
                            start.elapsed().as_millis()
                        );
                        cx.update(|cx| cx.quit());
                    })
                    .detach();
                }
            })
            .detach();
            let Ok((handle, _)) = gpui_kit::open_window(
                WindowOptions {
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|_| TestView(dropped)),
            ) else {
                eprintln!("Failed to open the native GPUI test window.");
                std::process::exit(1);
            };
            let dispatcher = cx.background_executor().clone();
            cx.spawn(async move |cx| {
                dispatcher.timer(Duration::from_millis(250)).await;
                if cx
                    .update_window(handle, |_, window, _| window.remove_window())
                    .is_err()
                {
                    eprintln!("Could not close the native GPUI test window.");
                    std::process::exit(1);
                }
            })
            .detach();
        }
    });
    Err(io::Error::other("GPUI returned before the native quit path").into())
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::FAILURE
}
