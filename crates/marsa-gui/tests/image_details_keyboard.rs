//! Exercise the real Images details trigger in a hidden macOS window.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
#[path = "../src/screens/resources/image_details_trigger.rs"]
mod image_details_trigger;

#[cfg(target_os = "macos")]
fn main() {
    native::run();
}

#[cfg(target_os = "macos")]
mod native {
    use std::{cell::Cell, rc::Rc};

    use marsa_ui::{
        Locale, direction,
        gpui::{
            AppContext as _, Application, Context, FocusHandle, IntoElement, KeyUpEvent, Keystroke,
            ParentElement, PlatformInput, Render, Styled, Window, WindowOptions, WindowingRequest,
            div,
        },
        gpui_component::{Root, WindowExt, button::Button},
        gui_keys as gui,
    };

    use super::image_details_trigger::ImageDetailsTrigger;

    struct Fixture {
        locale: Locale,
        before: FocusHandle,
        after: FocusHandle,
        opened: Rc<Cell<u8>>,
    }

    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let opened = Rc::clone(&self.opened);
            let label = gui::GUI_DESKTOP_IMAGE_DETAILS.text(self.locale);
            div()
                .flex()
                .child(
                    Button::new("before-image-details")
                        .track_focus(&self.before)
                        .label("Before"),
                )
                .child(ImageDetailsTrigger::new(
                    "sha256:fixture",
                    label,
                    label.into(),
                    true,
                    move |window, cx| {
                        opened.set(opened.get() + 1);
                        window.open_dialog(cx, |dialog, _, _| dialog.title("Fixture image"));
                    },
                ))
                .child(
                    Button::new("after-image-details")
                        .track_focus(&self.after)
                        .label("After"),
                )
        }
    }

    pub(super) fn run() {
        let app = Application::with_platform(Rc::new(gpui_macos::MacPlatform::new(false)))
            .with_windowing(WindowingRequest::Headless);
        app.run(|cx| {
            marsa_ui::gpui_component::init(cx);
            direction::init(Locale::EnglishUs, cx);
            for locale in [Locale::EnglishUs, Locale::Arabic] {
                direction::set_locale(locale, cx);
                let before = cx.focus_handle();
                let after = cx.focus_handle();
                let opened = Rc::new(Cell::new(0));
                let options = WindowOptions {
                    focus: false,
                    show: false,
                    ..WindowOptions::default()
                };
                let window = cx
                    .open_window(options, |window, cx| {
                        let view = cx.new(|_| Fixture {
                            locale,
                            before: before.clone(),
                            after: after.clone(),
                            opened: Rc::clone(&opened),
                        });
                        cx.new(|cx| Root::new(view, window, cx))
                    })
                    .expect("a hidden Images keyboard test window");
                cx.update_window(window.into(), |_, window, cx| {
                    before.focus(window, cx);
                    window.draw(cx).clear(cx);
                    let press = |key, window: &mut Window, cx: &mut marsa_ui::gpui::App| {
                        let keystroke = Keystroke::parse(key).expect("valid test key");
                        window.dispatch_keystroke(keystroke.clone(), cx);
                        window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
                        window.draw(cx).clear(cx);
                    };
                    press("tab", window, cx);
                    let trigger = window.focused(cx).expect("Tab focuses the details trigger");
                    assert_ne!(trigger, before);
                    assert_ne!(trigger, after);
                    for (index, key) in ["enter", "space"].into_iter().enumerate() {
                        press(key, window, cx);
                        assert_eq!(usize::from(opened.get()), index + 1, "{locale:?} {key}");
                        assert!(window.has_active_dialog(cx), "{locale:?} {key}");
                        press("escape", window, cx);
                        assert!(!window.has_active_dialog(cx), "{locale:?} Escape");
                        assert_eq!(
                            window.focused(cx),
                            Some(trigger.clone()),
                            "{locale:?} Escape"
                        );
                    }
                    press("tab", window, cx);
                    assert_eq!(window.focused(cx), Some(after));
                })
                .expect("Images keyboard test dispatch");
            }
            std::process::exit(0);
        });
    }
}
