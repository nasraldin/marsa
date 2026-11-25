//! Exercise logical layout and CoreText on the same hidden macOS window.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    native::run();
}

#[cfg(target_os = "macos")]
mod native {
    use std::{cell::Cell, rc::Rc};

    use marsa_ui::gpui_component::IconNamed as _;
    use marsa_ui::{
        ColorToken, Locale, Palette,
        direction::{self, LogicalStyle as _},
        gpui::{
            App, AppContext as _, Application, Bounds, Context, Entity, Hsla, IntoElement,
            MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
            PlatformInput, Render, SharedString, Styled, TextRun, Window, WindowBounds,
            WindowOptions, WindowingRequest, canvas, div, font, point, px, size,
        },
        gpui_component::{
            self, IconName,
            input::{Editor, EditorState},
            resolve_icon_path,
        },
        gui_keys,
        preferences::DisplaySettings,
    };

    const TECHNICAL: [&str; 2] = ["/tmp/work:123", "--name=api-43"];

    struct DirectionRow {
        palette: Palette,
        sentence: SharedString,
        centers: Rc<Cell<[Pixels; 2]>>,
    }

    impl Render for DirectionRow {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            direction::row(cx)
                .justify_row_end()
                .padding_start(px(11.), cx)
                .padding_end(px(17.), cx)
                .w(px(400.))
                .h(px(44.))
                .font(font("Arial"))
                .text_size(px(14.))
                .bg(Hsla::from(self.palette.color(ColorToken::BgCanvas)))
                .text_color(self.palette.color(ColorToken::TextPrimary))
                .children([0, 1].map(|index| {
                    let centers = Rc::clone(&self.centers);
                    div()
                        .relative()
                        .w(px(160.))
                        .text_start(cx)
                        .child(if index == 0 {
                            self.sentence.clone()
                        } else {
                            "App".into()
                        })
                        .child(
                            canvas(
                                move |bounds, _, _| {
                                    let mut positions = centers.get();
                                    positions[index] = bounds.center().x;
                                    centers.set(positions);
                                },
                                |_, (), _, _| (),
                            )
                            .absolute()
                            .inset_0(),
                        )
                }))
        }
    }

    pub(super) fn run() {
        let app = Application::with_platform(Rc::new(gpui_macos::MacPlatform::new(false)))
            .with_windowing(WindowingRequest::Headless);
        app.run(|cx| {
            gpui_component::init(cx);
            direction::init(Locale::EnglishUs, cx);
            let centers = Rc::new(Cell::new([px(0.); 2]));
            let window = cx
                .open_window(
                    WindowOptions {
                        focus: false,
                        show: false,
                        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(px(420.), px(160.)),
                            cx,
                        ))),
                        ..WindowOptions::default()
                    },
                    |_, cx| {
                        cx.new(|_| DirectionRow {
                            palette: Palette::Light,
                            sentence: "".into(),
                            centers: Rc::clone(&centers),
                        })
                    },
                )
                .expect("hidden direction test window");

            for (locale, palette, expected, sidebar) in [
                (
                    Locale::EnglishUs,
                    Palette::Light,
                    [px(143.), px(303.)],
                    IconName::PanelLeftOpen,
                ),
                (
                    Locale::EnglishUs,
                    Palette::Dark,
                    [px(143.), px(303.)],
                    IconName::PanelLeftOpen,
                ),
                (
                    Locale::Arabic,
                    Palette::Light,
                    [px(257.), px(97.)],
                    IconName::PanelRightOpen,
                ),
                (
                    Locale::Arabic,
                    Palette::Dark,
                    [px(257.), px(97.)],
                    IconName::PanelRightOpen,
                ),
            ] {
                direction::set_locale(locale, cx);
                assert_locale_labels(locale);
                let [path, option] = TECHNICAL.map(direction::isolate_technical_text);
                let sentence = SharedString::from(match locale {
                    Locale::EnglishUs => format!("Path {path} then {option}"),
                    Locale::Arabic => format!("المسار {path} ثم {option}"),
                });
                window
                    .update(cx, |view, _, cx| {
                        view.palette = palette;
                        view.sentence = sentence.clone();
                        cx.notify();
                    })
                    .expect("direction test view updates");
                cx.update_window(window.into(), |_, window, cx| {
                    window.draw(cx).clear(cx);
                    assert_eq!(centers.get(), expected, "{locale:?} {palette:?}");
                    assert_technical_order(&sentence, locale, window);
                    assert_eq!(
                        resolve_icon_path(direction::sidebar_toggle(true).path(), cx),
                        sidebar.path(),
                    );
                    let back = if locale == Locale::Arabic {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronLeft
                    };
                    assert_eq!(
                        resolve_icon_path(IconName::ChevronLeft.path(), cx),
                        back.path(),
                    );
                    for icon in [IconName::Check, IconName::Play, IconName::Square] {
                        let path = icon.path();
                        assert_eq!(resolve_icon_path(path.clone(), cx), path);
                    }
                    println!("{locale:?} {palette:?} row centers {expected:?}");
                })
                .expect("direction test window updates");
                assert_code_editor_scrollbar(locale, palette, cx);
            }
            direction::set_locale(Locale::EnglishUs, cx);
            assert_locale_labels(Locale::EnglishUs);
            std::process::exit(0);
        });
    }

    struct CodePanel(Entity<EditorState>);

    impl Render for CodePanel {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            Editor::new(&self.0).w(px(320.)).h(px(150.))
        }
    }

    fn assert_code_editor_scrollbar(locale: Locale, palette: Palette, cx: &mut App) {
        palette.apply_to_components(locale, DisplaySettings::default(), cx);
        let options = WindowOptions {
            focus: false,
            show: false,
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(360.), px(200.)),
                cx,
            ))),
            ..WindowOptions::default()
        };
        let handle = cx
            .open_window(options, |window, cx| {
                let yaml = "    image: alpine\n".repeat(50);
                let editor = cx.new(|cx| EditorState::new(window, cx).default_value(yaml));
                cx.new(|_| CodePanel(editor))
            })
            .expect("open hidden code editor");
        cx.update_window(handle.into(), |view, window, cx| {
            let editor = view.downcast::<CodePanel>().unwrap().read(cx).0.clone();
            window.draw(cx).clear(cx);
            assert_eq!(editor.read(cx).scroll_offset().y, px(0.));
            // Press the scrollbar towards the end of the technical text.
            let position = point(px(313.), px(128.));
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            window.draw(cx).clear(cx);
            assert!(
                editor.read(cx).scroll_offset().y < px(0.),
                "{locale:?} {palette:?}: scrollbar follows the technical text direction",
            );
            window.remove_window();
        })
        .unwrap();
    }

    fn assert_locale_labels(locale: Locale) {
        let tag = gpui_component::locale();
        let expected = match locale {
            Locale::EnglishUs => ("en", "Cancel", "Please select", "Copy"),
            Locale::Arabic => ("ar", "إلغاء", "اختر خيارًا", "نسخ"),
        };
        assert_eq!(&*tag, expected.0);
        assert_eq!(
            gpui_component::_rust_i18n_translate(&tag, "Dialog.cancel"),
            expected.1
        );
        assert_eq!(gui_keys::GUI_DESKTOP_CANCEL.text(locale), expected.1);
        assert_eq!(
            gpui_component::_rust_i18n_translate(&tag, "Combobox.placeholder"),
            expected.2
        );
        assert_eq!(
            gpui_component::_rust_i18n_translate(&tag, "Copy"),
            expected.3
        );
    }

    fn assert_technical_order(sentence: &SharedString, locale: Locale, window: &Window) {
        let shaped = window.text_system().shape_line(
            sentence.clone(),
            px(14.),
            &[TextRun {
                len: sentence.len(),
                font: font("Arial"),
                ..TextRun::default()
            }],
            None,
        );
        let [path, option] = TECHNICAL.map(|token| {
            let start = sentence.find(token).expect("technical fixture value");
            let mut glyphs: Vec<_> = shaped
                .runs
                .iter()
                .flat_map(|run| &run.glyphs)
                .filter(|glyph| (start..start + token.len()).contains(&glyph.index))
                .collect();
            glyphs.sort_by_key(|glyph| glyph.index);
            assert_eq!(glyphs.len(), token.len(), "{token}");
            assert!(
                glyphs
                    .windows(2)
                    .all(|pair| pair[0].position.x < pair[1].position.x),
                "{token} must read left to right in {sentence}",
            );
            glyphs[0].position.x
        });
        if locale == Locale::Arabic {
            assert!(
                path > option,
                "Arabic labels order the technical runs right to left"
            );
        } else {
            assert!(
                path < option,
                "English labels order the technical runs left to right"
            );
        }
    }
}
