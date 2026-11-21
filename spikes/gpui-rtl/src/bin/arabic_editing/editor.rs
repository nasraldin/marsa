//! Runs real GPUI input and its text-service adapter in an offscreen Metal window.

use std::{io, sync::Arc};

use gpui_kit::{
    App, AppContext as _, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler as _,
    Focusable as _, HeadlessAppContext, IntoElement, Result, Styled as _, Window, WindowBounds,
    WindowOptions,
    base::LayoutDirection,
    component::input::{Input, InputState},
    platform::{current_headless_renderer, current_platform},
    point, px, size,
    test::{TestInput, TestWindowExt as _},
};

use super::core_text::{EDITOR_FONT, EDITOR_SIZE, Geometry, require};

pub fn probe() -> Result<()> {
    let mut cx = HeadlessAppContext::with_platform(
        current_platform(true).text_system(),
        Arc::new(()),
        current_headless_renderer,
    );
    cx.update(gpui_kit::init);
    let (window, view) = cx.update(|cx| {
        LayoutDirection::Rtl.set_global(cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(560.), px(120.)),
                })),
                show: false,
                focus: false,
                ..WindowOptions::default()
            },
            cx,
            |window, cx| cx.new(|cx| EditingProbe(cx.new(|cx| InputState::new(window, cx)))),
        )
    })?;
    cx.update_window(window, |_, window, cx| {
        let input = view.read(cx).0.clone();
        input.read(cx).focus_handle(cx).focus(window, cx);
        window.render_frame(cx);
        keyboard_and_click(&input, window, cx)?;
        composition(&input, window, cx)
    })??;
    Ok(())
}

struct EditingProbe(Entity<InputState>);

impl gpui_kit::Render for EditingProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Input::new(&self.0)
            .font_family(EDITOR_FONT)
            .text_size(px(EDITOR_SIZE))
    }
}

fn keyboard_and_click(input: &Entity<InputState>, window: &mut Window, cx: &mut App) -> Result<()> {
    window.input("عرب", cx);
    require(
        input.read(cx).value().as_ref() == "عرب",
        "Arabic keystrokes were lost",
    )?;
    let mut position = Vec::new();
    let mut offsets = Vec::new();
    for step in 0..4 {
        position.push(caret_x(input, cx)?);
        offsets.push(input.read(cx).cursor());
        if step != 3 {
            window.press("right", cx);
        }
    }
    require(
        offsets == [6, 4, 2, 0],
        "GPUI skipped a logical Arabic boundary",
    )?;
    println!("GPUI Arabic Right moves (bytes): {offsets:?}; painted x: {position:.3?}");

    let physical_start = position.into_iter().fold(f32::NEG_INFINITY, f32::max) - 0.1;
    let field = input.read(cx).input_bounds();
    let hit = point(px(physical_start), field.center().y);
    window.drag(hit, hit, cx);
    let upstream_offset = input.read(cx).cursor();

    let native = Geometry::new("عرب")?;
    let native_right = native.carets[0].primary;
    let target = native.hit(native_right - 0.1);
    let corrected_x = apply_selection(input, target, window, cx)?;
    let alternate_x = apply_selection(input, 1, window, cx)?;
    println!(
        "Click at Arabic inline start: GPUI byte {upstream_offset}, CoreText UTF-16 {target}.          Public GPUI setter corrected the index; x for UTF-16 [0,1] remains [{corrected_x:.3}, {alternate_x:.3}]."
    );

    let text = "مرسى api-2 /work/app بيت";
    input.update(cx, |state, cx| state.set_value(text, window, cx));
    let mixed = Geometry::new(text)?;
    let mut coordinates = Vec::new();
    for caret in &mixed.carets {
        coordinates.push((caret.byte, apply_selection(input, caret.utf16, window, cx)?));
    }
    println!(
        "GPUI mixed painted x with public logical-position override (byte, x): {coordinates:.3?}"
    );
    Ok(())
}

fn apply_selection(
    input: &Entity<InputState>,
    caret: usize,
    window: &mut Window,
    cx: &mut App,
) -> Result<f32> {
    input.update(cx, |state, cx| {
        state.set_selected_text_range(caret..caret, window, cx)
    });
    window.render_frame(cx);
    require(
        input
            .read(cx)
            .value()
            .get(..input.read(cx).cursor())
            .is_some_and(|prefix| prefix.encode_utf16().count() == caret),
        "GPUI's public selection setter changed the requested UTF-16 index",
    )?;
    caret_x(input, cx)
}

fn caret_x(input: &Entity<InputState>, cx: &App) -> Result<f32> {
    input
        .read(cx)
        .cursor_layout()
        .map(|(bounds, _)| f32::from(bounds.origin.x))
        .ok_or_else(|| io::Error::other("GPUI input did not paint its caret").into())
}

fn composition(input: &Entity<InputState>, window: &mut Window, cx: &mut App) -> Result<()> {
    input.update(cx, |state, cx| state.set_value("a🙂b", window, cx));
    window.render_frame(cx);
    let bounds = input.read(cx).input_bounds();
    let mut ime = TestInput::new(ElementInputHandler::new(bounds, input.clone()), window);
    ime.compose_text(Some(1..3), "نَ", Some(2..2), cx);
    require(
        ime.marked_text_range(cx) == Some(1..3),
        "wrong UTF-16 marked range",
    )?;
    require(
        input.read(cx).value().as_ref() == "aنَb",
        "marked replacement split the emoji",
    )?;
    ime.compose_text(None, "نَصّ", Some(4..4), cx);
    require(
        ime.marked_text_range(cx) == Some(1..5),
        "updated mark had wrong UTF-16 range",
    )?;
    ime.commit_text("مرسى", cx);
    require(
        ime.marked_text_range(cx).is_none(),
        "commit retained a marked range",
    )?;
    require(
        input.read(cx).value().as_ref() == "aمرسىb",
        "IME commit replaced the wrong text",
    )?;
    println!("GPUI text-service composition and commit passed: a🙂b → aنَb → aنَصّb → aمرسىb.");

    let selected = input.update(cx, |state, cx| {
        state.set_selected_text_range(1..5, window, cx);
        state.selected_text().to_string()
    });
    require(selected == "مرسى", "GPUI selected the wrong logical text")?;
    window.press("backspace", cx);
    require(
        input.read(cx).value().as_ref() == "ab",
        "Backspace did not delete the Arabic selection",
    )?;
    window.press("cmd-a", cx);
    window.input("نَصّ👩🏽‍💻", cx);
    window.press("backspace", cx);
    require(
        input.read(cx).value().as_ref() == "نَصّ",
        "Backspace split a ZWJ grapheme",
    )?;
    window.press("backspace", cx);
    require(
        input.read(cx).value().as_ref() == "نَ",
        "Backspace split an Arabic mark",
    )?;
    println!(
        "GPUI logical selection and grapheme deletion passed for Arabic marks and an emoji ZWJ sequence."
    );
    Ok(())
}
