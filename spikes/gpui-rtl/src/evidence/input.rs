//! Exercises Arabic keyboard and pointer behavior in the rendered text field.

use std::io;

use gpui_kit::{
    App, Entity, Focusable as _, MouseButton, Result, Window, point, px,
    test::{TestEventExt as _, TestWindowExt as _},
};

use crate::view::Spike;

use super::require;

pub(super) fn probe(view: &Entity<Spike>, window: &mut Window, cx: &mut App) -> Result<()> {
    let input = view.read(cx).input.clone();
    input.read(cx).focus_handle(cx).focus(window, cx);
    window.press("cmd-a", cx);
    window.input("عرب", cx);
    require(
        input.read(cx).value().as_ref() == "عرب",
        "Arabic character input was lost",
    )?;

    let mut offsets = [0; 4];
    let mut x = [0.; 4];
    for index in 0..4 {
        let state = input.read(cx);
        offsets[index] = state.cursor();
        let bounds = state
            .cursor_layout()
            .ok_or_else(|| io::Error::other("focused Arabic input did not expose caret bounds"))?
            .0;
        x[index] = f32::from(bounds.origin.x);
        if index != 3 {
            window.press("right", cx);
        }
    }
    require(
        offsets == [6, 4, 2, 0],
        "Arabic arrow movement skipped logical byte boundaries",
    )?;
    let caret_passed = x.windows(2).all(|pair| pair[0] + 0.5 < pair[1]);
    println!("Arabic Right x positions from logical end, expected strictly increasing: {x:?}");
    println!("Arabic logical UTF-8 offsets from the same steps: {offsets:?}");
    println!(
        "Visual Arabic caret movement: {}",
        if caret_passed { "PASS" } else { "FAIL" }
    );

    window.press("shift-left", cx);
    let selection = input.read(cx).selected_text().to_string();
    require(
        selection == "ع",
        "Arabic keyboard selection selected the wrong logical grapheme",
    )?;
    window.press("backspace", cx);
    require(
        input.read(cx).value().as_ref() == "رب",
        "Backspace did not remove the Arabic selection",
    )?;
    window.press("delete", cx);
    require(
        input.read(cx).value().as_ref() == "ب",
        "Forward Delete did not remove the next Arabic grapheme",
    )?;
    println!("Logical Arabic Shift-Left selected {selection}; Backspace then Delete left ب.");

    window.press("cmd-a", cx);
    window.input("نَصّ", cx);
    window.press("backspace", cx);
    require(
        input.read(cx).value().as_ref() == "نَ",
        "Arabic combining marks did not delete with their base",
    )?;
    println!("Backspace treated صّ as one grapheme.");

    window.press("cmd-a", cx);
    window.input("عرب", cx);
    let inline_edge = x.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let inline_start = point(
        px(inline_edge - 1.),
        input.read(cx).input_bounds().center().y,
    );
    window.pointer_down(inline_start, MouseButton::Left, cx);
    window.pointer_up(inline_start, MouseButton::Left, cx);
    let offset = input.read(cx).cursor();
    println!(
        "Near the Arabic text's physical starting edge, a pointer click gave offset {offset}, expected 0."
    );
    Ok(())
}
