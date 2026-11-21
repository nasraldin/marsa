//! Exercise the same input, focus, and action path for every window variant.

use std::{io, rc::Rc, sync::Arc};

use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, HeadlessAppContext, Result, Role, Window,
    WindowBounds, WindowOptions,
    base::LayoutDirection,
    component::{ActiveTheme as _, Theme, ThemeMode, WindowExt as _},
    platform::{current_headless_renderer, current_platform},
    point, px, size,
    test::{TestQueryExt as _, TestWindowExt as _},
};

use super::fixture::{Commands, Controls, Labels};

pub(super) fn check(condition: bool, failure: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(failure.into()).into())
    }
}

fn focused(window: &Window, label: &'static str) -> Result<()> {
    let item = window.find_by_label(label);
    check(
        item.focused() == Some(true),
        format!("expected keyboard focus on {label}; observed {item:?}"),
    )
}

pub(super) fn probe(
    name: &str,
    labels: Labels,
    direction: LayoutDirection,
    mode: ThemeMode,
) -> Result<()> {
    let mut cx = HeadlessAppContext::with_platform(
        current_platform(true).text_system(),
        Arc::new(()),
        current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        direction.set_global(cx);
        Theme::change(mode, None, cx);
    });

    let commands = Rc::new(Commands::default());
    let initial = cx.update(|cx| cx.focus_handle());
    let (handle, _) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(80.), px(80.)),
                    size: size(px(760.), px(480.)),
                })),
                show: false,
                focus: false,
                ..WindowOptions::default()
            },
            cx,
            |_, cx| {
                cx.new(|_| Controls {
                    labels,
                    commands: commands.clone(),
                    initial: initial.clone(),
                })
            },
        )
    })?;

    check(
        cx.update(|cx| cx.theme().is_dark()) == mode.is_dark(),
        "requested theme was not active",
    )?;
    cx.update_window(handle, |_, window, cx| {
        window.activate_window();
        window.render_frame(cx);
    })?;
    let unfocused_frame = cx.capture_screenshot(handle)?;
    cx.update_window(handle, |_, window, cx| {
        initial.focus(window, cx);
        window.render_frame(cx);
    })?;
    let focused_frame = cx.capture_screenshot(handle)?;
    let changed_pixels = unfocused_frame
        .pixels()
        .zip(focused_frame.pixels())
        .filter(|(before, after)| before != after)
        .count();
    check(
        changed_pixels > 0,
        "focus did not change the rendered frame",
    )?;
    println!("{name}: focusing Start changed {changed_pixels} rendered pixels");

    let result = cx.update_window(handle, |_, window, cx| -> Result<()> {
        window.render_frame(cx);
        let start = window.find_by_label(labels.start);
        let remove = window.find_by_label(labels.remove);
        let start_x = start.bounds().center().x;
        let remove_x = remove.bounds().center().x;
        check(
            if direction.is_rtl() { start_x > remove_x } else { start_x < remove_x },
            "control bounds do not follow layout direction",
        )?;
        check(start.role() == Some(Role::Button), "start lacks Button role")?;
        check(
            window.find_by_label(labels.disabled).disabled() == Some(true),
            "disabled control lacks disabled semantics",
        )?;

        focused(window, labels.start)?;
        window.press("space", cx);
        window.press("enter", cx);
        check(commands.started.get() == 2, "Space/Enter did not activate Start exactly once each")?;
        window.press("tab", cx);
        focused(window, labels.remove)?;
        window.press("shift-tab", cx);
        focused(window, labels.start)?;
        check(commands.disabled.get() == 0, "disabled control executed")?;
        println!("{name}: Tab/Shift-Tab order, disabled skip, Space/Enter, mirrored bounds, Button role and localized names passed");

        window.press("tab", cx);
        window.press("enter", cx);
        check(window.has_active_dialog(cx), "Enter on removal did not open dialog")?;
        check(
            window.find_by_label(labels.input).role() == Some(Role::TextInput),
            "confirmation input lacks TextInput role",
        )?;
        window.press("enter", cx);
        check(
            window.has_active_dialog(cx) && commands.removed.get() == 0,
            "empty modal confirmation executed or closed",
        )?;
        window.click("start", cx);
        check(
            window.has_active_dialog(cx) && commands.started.get() == 2,
            "background activation was not blocked by the modal overlay",
        )?;
        window.press("escape", cx);
        check(!window.has_active_dialog(cx), "Escape did not dismiss dialog")?;
        focused(window, labels.remove)?;
        println!("{name}: dialog opened via keyboard, rejected empty Enter, Escape dismissed and restored trigger focus");

        window.press("enter", cx);
        window.press("tab", cx);
        focused(window, labels.input)?;
        window.input("WRONG", cx);
        window.press("enter", cx);
        check(
            window.has_active_dialog(cx) && commands.removed.get() == 0,
            "incorrect confirmation executed or closed",
        )?;
        window.press("tab", cx);
        focused(window, labels.cancel)?;
        window.press("tab", cx);
        focused(window, labels.confirm)?;
        window.press("space", cx);
        check(
            window.has_active_dialog(cx) && commands.removed.get() == 0,
            "wrong token bypassed the confirmation-button guard",
        )?;
        window.press("tab", cx);
        focused(window, labels.input)?;
        window.press("cmd-a", cx);
        window.input("REMOVE", cx);
        check(
            window.find_by_label(labels.input).value() == Some("REMOVE"),
            "typed token missing from input semantics",
        )?;
        window.press("tab", cx);
        focused(window, labels.cancel)?;
        window.press("tab", cx);
        focused(window, labels.confirm)?;
        window.press("tab", cx);
        focused(window, labels.input)?;
        window.press("shift-tab", cx);
        focused(window, labels.confirm)?;
        window.press("space", cx);
        check(
            !window.has_active_dialog(cx) && commands.removed.get() == 1,
            "typed confirmation failed to activate exactly once",
        )?;
        focused(window, labels.remove)?;
        println!("{name}: rejected incorrect entry, modal tab loop stayed inside, authorized activation and focus restoration passed");

        Ok(())
    })?;
    result?;
    probe_menu(&mut cx, handle, labels, &commands)?;
    println!(
        "{name}: menu keyboard open, Escape, Down/Enter, disabled item skip, dismissal and focus restoration passed"
    );
    Ok(())
}

fn probe_menu(
    cx: &mut HeadlessAppContext,
    handle: AnyWindowHandle,
    labels: Labels,
    commands: &Commands,
) -> Result<()> {
    cx.update_window(handle, |_, window, cx| {
        window.press("tab", cx);
        focused(window, labels.menu)?;
        window.press("space", cx);
        check(
            window.find("popup-menu").role() == Some(Role::Menu)
                && window.find_by_label(labels.menu_run).role() == Some(Role::MenuItem),
            "keyboard activation did not expose Menu and MenuItem semantics",
        )
    })??;
    cx.update_window(handle, |_, window, cx| window.press("escape", cx))?;
    // Popup dismissal emits an event; GPUI delivers it after the update.
    cx.update_window(handle, |_, window, cx| -> Result<()> {
        window.render_frame(cx);
        check(
            window.try_find_by_label(labels.menu_run).is_none(),
            "Escape did not dismiss menu",
        )?;
        focused(window, labels.menu)?;
        window.press("enter", cx);
        window.press("down", cx);
        Ok(())
    })??;
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))?;
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        check(
            commands.inspected.get() == 1 && commands.disabled.get() == 0,
            format!(
                "menu command counts: enabled={}, disabled={}",
                commands.inspected.get(),
                commands.disabled.get()
            ),
        )?;
        check(
            window.try_find_by_label(labels.menu_run).is_none(),
            "menu stayed open after activation",
        )?;
        focused(window, labels.menu)
    })??;
    Ok(())
}
