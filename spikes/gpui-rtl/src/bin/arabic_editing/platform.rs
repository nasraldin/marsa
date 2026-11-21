//! Exercises a public AppKit text view attachment without showing a window.

use std::{io, ptr, sync::mpsc, thread, time::Duration};

use gpui_kit::{
    ActivationPolicy, App, AppContext as _, Application, Bounds, Context, IntoElement, Render,
    Result, Window, WindowBounds, WindowOptions, div, point, px, size,
};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSFont, NSStandardKeyBindingResponding as _, NSTextInputClient as _, NSTextView, NSView,
    NSWritingDirection,
};
use objc2_foundation::{NSNotFound, NSPoint, NSRange, NSRect, NSSize, NSString};
use raw_window_handle::RawWindowHandle;

use super::core_text::{EDITOR_SIZE, require};
use super::grapheme_view::GraphemeTextView;

pub fn probe() -> Result<()> {
    let (_watchdog_cancel, watchdog) = mpsc::channel::<()>();
    thread::Builder::new()
        .name("appkit-probe-watchdog".into())
        .spawn(move || {
            if matches!(
                watchdog.recv_timeout(Duration::from_secs(10)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                eprintln!("Native AppKit attachment probe exceeded 10 seconds.");
                std::process::exit(1);
            }
        })?;
    let platform = gpui_kit::platform::current_platform(false);
    platform.set_activation_policy(ActivationPolicy::Accessory);
    Application::with_platform(platform).run(|cx| {
        if let Err(error) = run(cx) {
            eprintln!("Native AppKit attachment probe: {error:#}");
            std::process::exit(1);
        }
        cx.quit();
    });
    Ok(())
}

fn run(cx: &mut App) -> Result<()> {
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(360.), px(100.)),
            })),
            show: false,
            focus: false,
            ..WindowOptions::default()
        },
        |_, cx| cx.new(|_| HiddenRoot),
    )?;
    window.update(cx, |_, window, _| attach_and_probe(window))??;
    Ok(())
}

struct HiddenRoot;

impl Render for HiddenRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn attach_and_probe(window: &Window) -> Result<()> {
    let main =
        MainThreadMarker::new().ok_or_else(|| io::Error::other("AppKit needs the main thread"))?;
    let handle = raw_window_handle::HasWindowHandle::window_handle(window)?;
    let RawWindowHandle::AppKit(raw) = handle.as_raw() else {
        return Err(io::Error::other("GPUI did not return an AppKit NSView").into());
    };
    // SAFETY: GPUI returns a borrowed, non-null AppKit NSView for the life of
    // this platform window. This function runs on the AppKit main thread.
    let host = unsafe { raw.ns_view.cast::<NSView>().as_ref() };
    let native_window = host
        .window()
        .ok_or_else(|| io::Error::other("GPUI NSView has no AppKit window"))?;
    require(
        !native_window.isVisible(),
        "the probe requires a hidden window",
    )?;

    let frame = NSRect::new(NSPoint::new(4., 4.), NSSize::new(320., 50.));
    let stock = NSTextView::initWithFrame(main.alloc(), frame);
    with_attached_field(host, &stock, |field| {
        let (emoji_deletion, arabic_deletion) = delete_clusters(field);
        println!("Stock AppKit post-Backspace values: {emoji_deletion:?}, {arabic_deletion:?}.");
        Ok(())
    })?;

    let candidate = GraphemeTextView::new(main, frame);
    with_attached_field(host, &candidate, |field| {
        verify_arabic(field)?;
        verify_mixed(field)?;
        verify_marked_text(field)?;
        let (emoji_deletion, arabic_deletion) = delete_clusters(field);
        require(
            emoji_deletion == "نَصّ",
            "adapted AppKit Backspace split an emoji grapheme",
        )?;
        require(
            arabic_deletion == "نَ",
            "adapted AppKit Backspace split an Arabic grapheme",
        )?;
        println!(
            "Adapted AppKit Backspace removed an emoji ZWJ sequence and an Arabic combining cluster."
        );
        Ok(())
    })
}

fn with_attached_field(
    host: &NSView,
    field: &NSTextView,
    run: impl FnOnce(&NSTextView) -> Result<()>,
) -> Result<()> {
    let native_window = host
        .window()
        .ok_or_else(|| io::Error::other("GPUI NSView has no AppKit window"))?;
    field.setEditable(true);
    field.setRichText(false);
    field.setFont(
        NSFont::fontWithName_size(
            &NSString::from_str("IBMPlexSansArabic"),
            f64::from(EDITOR_SIZE),
        )
        .as_deref(),
    );
    host.addSubview(field);
    let result = (|| {
        require(
            native_window.makeFirstResponder(Some(field)),
            "the attached text view could not become first responder",
        )?;
        require(
            native_window.firstResponder().is_some_and(|responder| {
                Retained::as_ptr(&responder).cast::<()>() == ptr::from_ref(field).cast::<()>()
            }),
            "AppKit selected a different first responder",
        )?;
        require(
            !native_window.isVisible(),
            "first responder change showed the window",
        )?;
        run(field)
    })();
    let restored = native_window.makeFirstResponder(Some(host));
    field.removeFromSuperview();
    result.and_then(|()| require(restored, "AppKit could not restore the GPUI responder"))
}

fn verify_arabic(field: &NSTextView) -> Result<()> {
    field.setString(&NSString::from_str("عرب"));
    field.setBaseWritingDirection(NSWritingDirection::RightToLeft);
    field.setSelectedRange(NSRange::new(3, 0));

    let mut caret_x = Vec::new();
    let mut offsets = Vec::new();
    for step in 0..4 {
        let offset = field.selectedRange().location;
        offsets.push(offset);
        caret_x.push(caret_rect(field, offset).origin.x);
        if step != 3 {
            // SAFETY: This NSTextView implements the public action; nil is the
            // programmatic sender accepted by AppKit's key binding protocol.
            unsafe { field.moveRight(None) };
        }
    }
    require(
        offsets == [3, 2, 1, 0],
        "AppKit physical Right skipped Arabic offsets",
    )?;
    require(
        caret_x.windows(2).all(|pair| pair[0] < pair[1]),
        "AppKit Right did not move the caret visually",
    )?;

    let first = caret_rect(field, 0);
    let hit = field.characterIndexForPoint(NSPoint::new(first.origin.x - 0.5, first.mid().y));
    require(hit == 0, "AppKit hit test missed the physical Arabic start")?;
    // SAFETY: NSTextView implements the public action and accepts a nil sender.
    unsafe { field.moveLeftAndModifySelection(None) };
    require(
        field.selectedRange() == NSRange::new(0, 1),
        "AppKit Shift-Left selected the wrong Arabic range",
    )?;
    // SAFETY: NSTextView implements the public action and accepts a nil sender.
    unsafe { field.deleteBackward(None) };
    require(
        field.string().to_string() == "رب",
        "AppKit deleted the wrong Arabic grapheme",
    )?;
    println!(
        "Attached AppKit Arabic Right offsets: {offsets:?}; screen x: {caret_x:.3?}; native hit {hit}; shift-selection and deletion passed."
    );
    Ok(())
}

fn verify_mixed(field: &NSTextView) -> Result<()> {
    let text = "مرسى api-2 /work/app بيت";
    let inside_latin = "مرسى a".encode_utf16().count();
    field.setString(&NSString::from_str(text));
    field.setBaseWritingDirection(NSWritingDirection::RightToLeft);
    field.setSelectedRange(NSRange::new(inside_latin, 0));
    let before = caret_rect(field, inside_latin).origin.x;
    // SAFETY: NSTextView implements the public action and accepts a nil sender.
    unsafe { field.moveRight(None) };
    let next = field.selectedRange().location;
    let after = caret_rect(field, next).origin.x;
    require(
        next == inside_latin + 1 && after > before,
        "native Right did not follow a Latin run in Arabic text",
    )?;
    println!(
        "Attached AppKit mixed Arabic/Latin Right: UTF-16 {inside_latin} → {next}; x {before:.3} → {after:.3}."
    );
    Ok(())
}

fn caret_rect(field: &NSTextView, offset: usize) -> NSRect {
    // SAFETY: The empty range lies on a valid UTF-16 boundary in the field;
    // AppKit accepts a null optional actual-range output pointer.
    unsafe {
        field.firstRectForCharacterRange_actualRange(NSRange::new(offset, 0), ptr::null_mut())
    }
}

fn verify_marked_text(field: &NSTextView) -> Result<()> {
    field.setString(&NSString::from_str("a🙂b"));
    // SAFETY: NSString is accepted by NSTextInputClient, the replacement covers
    // the complete emoji, and the new caret is within the replacement preedit.
    unsafe {
        field.setMarkedText_selectedRange_replacementRange(
            &NSString::from_str("نَ"),
            NSRange::new(2, 0),
            NSRange::new(1, 2),
        );
    }
    require(
        field.markedRange() == NSRange::new(1, 2),
        "AppKit preedit range was incorrect",
    )?;
    // SAFETY: NSString is accepted and NSNotFound requests insertion over the
    // current marked range using NSTextInputClient's documented sentinel.
    unsafe {
        field.insertText_replacementRange(
            &NSString::from_str("مرسى"),
            NSRange::new(NSNotFound as usize, 0),
        );
    }
    require(
        !field.hasMarkedText(),
        "AppKit commit retained its marked range",
    )?;
    require(
        field.string().to_string() == "aمرسىb",
        "AppKit committed into the wrong UTF-16 range",
    )?;
    println!(
        "Attached AppKit NSTextInputClient marked replacement and commit passed in a hidden GPUI window."
    );
    Ok(())
}

fn delete_clusters(field: &NSTextView) -> (String, String) {
    let text = "نَصّ👩🏽‍💻";
    field.setString(&NSString::from_str(text));
    field.setSelectedRange(NSRange::new(text.encode_utf16().count(), 0));
    // SAFETY: NSTextView implements the public action and accepts a nil sender.
    unsafe { field.deleteBackward(None) };
    let after_emoji = field.string().to_string();
    // SAFETY: NSTextView implements the public action and accepts a nil sender.
    unsafe { field.deleteBackward(None) };
    (after_emoji, field.string().to_string())
}
