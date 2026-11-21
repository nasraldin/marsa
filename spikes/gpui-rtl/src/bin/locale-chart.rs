//! Checks component locale fallback and chart direction on the macOS Metal renderer.

#[cfg(target_os = "macos")]
mod macos {
    use std::{fs, io, path::Path, sync::Arc};

    use gpui_kit::{
        AppContext as _, Bounds, Context, Entity, HeadlessAppContext, IntoElement, Window,
        WindowBounds, WindowOptions,
        base::LayoutDirection,
        component::{
            ActiveTheme as _, Theme, ThemeMode,
            chart::LineChart,
            select::{Select, SelectState},
        },
        div,
        platform::{current_headless_renderer, current_platform},
        point,
        prelude::*,
        px, size,
    };

    use gpui_kit::test::TestWindowExt as _;

    const LABELS: [&str; 3] = ["00:00", "00:01", "00:02"];
    const EN: &str = "Choose an image";
    const AR: &str = "اختر صورة";

    struct Components {
        stock: Entity<SelectState<Vec<&'static str>>>,
        localized: Entity<SelectState<Vec<&'static str>>>,
    }

    impl Render for Components {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let label = if LayoutDirection::global(cx).is_rtl() {
                AR
            } else {
                EN
            };
            div()
                .size_full()
                .p_6()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .h_10()
                        .child(Select::new(&self.stock).id("stock-select")),
                )
                .child(
                    div().h_10().child(
                        Select::new(&self.localized)
                            .id("translated-select")
                            .placeholder(label),
                    ),
                )
                .child(
                    div().w_full().h_48().child(
                        LineChart::new([(LABELS[0], 10.0), (LABELS[1], 18.0), (LABELS[2], 14.0)])
                            .x(|sample| sample.0)
                            .y(|sample| sample.1)
                            .appear(false),
                    ),
                )
        }
    }

    pub(super) fn run(destination: &Path) -> gpui_kit::Result<()> {
        fs::create_dir_all(destination)?;
        let mut cx = HeadlessAppContext::with_platform(
            current_platform(true).text_system(),
            Arc::new(()),
            current_headless_renderer,
        );
        cx.update(gpui_kit::init);
        let (handle, _) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(80.), px(80.)),
                        size: size(px(640.), px(360.)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let stock = cx.new(|cx| SelectState::new(vec!["alpine"], None, window, cx));
                    let localized = cx.new(|cx| SelectState::new(vec!["alpine"], None, window, cx));
                    cx.new(|_| Components { stock, localized })
                },
            )
        })?;
        for (name, locale, mode) in [
            ("en-light", "en", ThemeMode::Light),
            ("en-dark", "en", ThemeMode::Dark),
            ("ar-light", "ar", ThemeMode::Light),
            ("ar-dark", "ar", ThemeMode::Dark),
        ] {
            cx.update_window(handle, |_, window, cx| {
                gpui_kit::component::set_locale(locale);
                LayoutDirection::from_locale(locale).set_global(cx);
                Theme::change(mode, None, cx);
                window.refresh();
                window.render_frame(cx);
                let custom = if locale == "ar" { AR } else { EN };
                let translated_select = window.find("translated-select");
                require(
                    translated_select.value() == Some(custom),
                    "select value was not localized",
                )?;
                let stock_select = window.find("stock-select");
                let fallback = stock_select.value() == Some("Please select");
                if locale == "en" {
                    require(fallback, "English default value was not exposed")?;
                }
                println!(
                    "{name}: localized select value exposed, stock English placeholder={fallback}"
                );
                Ok::<(), io::Error>(())
            })??;
            cx.capture_screenshot(handle)?
                .save(destination.join(format!("{name}.png")))?;
        }
        Ok(())
    }

    fn require(test: bool, message: &str) -> io::Result<()> {
        if test {
            Ok(())
        } else {
            Err(io::Error::other(message))
        }
    }
}

#[cfg(target_os = "macos")]
fn main() -> gpui_kit::Result<()> {
    let destination = std::env::args_os()
        .nth(1)
        .ok_or_else(|| std::io::Error::other("usage: locale-chart <capture-directory>"))?;
    macos::run(std::path::Path::new(&destination))
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::FAILURE
}
