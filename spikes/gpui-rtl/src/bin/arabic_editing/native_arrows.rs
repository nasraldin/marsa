//! Drives the visible editor through AppKit's native window key-event path.

use std::io;

use gpui_kit::Result;
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSTextView, NSWindow};
use objc2_foundation::{NSPoint, NSRange, NSString};

use super::{super::core_text::require, platform};

const LEFT: (u16, &str) = (123, "\u{f702}");
const RIGHT: (u16, &str) = (124, "\u{f703}");

pub(super) fn verify(window: &NSWindow, field: &NSTextView) -> Result<()> {
    require(
        window.isVisible() && window.isKeyWindow(),
        "the GPUI window must be visible and key",
    )?;
    field.setString(&NSString::from_str("عرب"));
    field.setSelectedRange(NSRange::new(3, 0));

    let mut previous_x = platform::caret_rect(field, 3).origin.x;
    for offset in [2, 1, 0] {
        send(window, RIGHT, NSEventModifierFlags::empty())?;
        require(
            field.selectedRange() == NSRange::new(offset, 0),
            "Right moved to the wrong Arabic offset",
        )?;
        let x = platform::caret_rect(field, offset).origin.x;
        require(
            x > previous_x,
            "Right did not move the Arabic caret visually",
        )?;
        println!("Visible native Right: UTF-16 {offset}, screen x {x:.1}");
        previous_x = x;
    }
    send(window, LEFT, NSEventModifierFlags::empty())?;
    require(
        field.selectedRange() == NSRange::new(1, 0),
        "Left moved to the wrong Arabic offset",
    )?;
    send(window, LEFT, NSEventModifierFlags::Shift)?;
    require(
        field.selectedRange() == NSRange::new(1, 1),
        "Shift-Left selected the wrong Arabic letter",
    )?;
    require(
        field.string().to_string() == "عرب",
        "an arrow key modified the Arabic text",
    )?;
    println!("Visible native Left and Shift-Left passed. Selected UTF-16 1:1.");
    Ok(())
}

fn send(
    window: &NSWindow,
    (key_code, characters): (u16, &str),
    modifiers: NSEventModifierFlags,
) -> Result<()> {
    let characters = NSString::from_str(characters);
    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyDown,
        NSPoint::ZERO,
        modifiers,
        0.,
        window.windowNumber(),
        None,
        &characters,
        &characters,
        false,
        key_code,
    ).ok_or_else(|| io::Error::other("AppKit could not construct the arrow key event"))?;
    window.sendEvent(&event);
    Ok(())
}
