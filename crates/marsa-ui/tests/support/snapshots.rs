//! Render a production view with CoreText and Metal at a fixed viewport and scale.

use std::{env, fs, path::PathBuf, rc::Rc};

use gpui_macos::MacPlatform;
use image::{Rgba, RgbaImage};
use marsa_ui::{
    Locale, Palette,
    gpui::{
        AnyWindowHandle, App, AppContext as _, Application, Bounds, Entity, KeyUpEvent, Keystroke,
        MouseMoveEvent, Pixels, PlatformInput, Render, Size, Window, WindowBounds, WindowOptions,
        WindowingRequest, point, px,
    },
};

use super::assets::MarsaAssets;

/// The canonical order used by gallery and screen references.
#[derive(Clone, Copy)]
pub(super) struct Variant {
    pub(super) name: &'static str,
    pub(super) locale: Locale,
    pub(super) palette: Palette,
}

impl Variant {
    pub(super) const ALL: [Self; 4] = [
        Self {
            name: "en-light",
            locale: Locale::EnglishUs,
            palette: Palette::Light,
        },
        Self {
            name: "en-dark",
            locale: Locale::EnglishUs,
            palette: Palette::Dark,
        },
        Self {
            name: "ar-light",
            locale: Locale::Arabic,
            palette: Palette::Light,
        },
        Self {
            name: "ar-dark",
            locale: Locale::Arabic,
            palette: Palette::Dark,
        },
    ];
}

pub(super) fn application() -> Application {
    Application::with_platform(Rc::new(MacPlatform::new(false)))
        .with_windowing(WindowingRequest::Headless)
        .with_assets(MarsaAssets)
}

pub(super) fn open<V: Render + 'static>(
    cx: &mut App,
    viewport: Size<Pixels>,
    build_root: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> AnyWindowHandle {
    let bounds = Bounds::centered(None, viewport, cx);
    let handle = cx
        .open_window(
            WindowOptions {
                show: false,
                focus: false,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..WindowOptions::default()
            },
            build_root,
        )
        .expect("open offscreen window")
        .into();
    cx.update_window(handle, |_, window, cx| {
        assert_eq!(window.viewport_size(), viewport, "fixed snapshot viewport");
        // GPUI seeds new windows with the system pointer, even when hidden.
        // Start outside the viewport until the fixture supplies its own input.
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(-1.), px(-1.)),
                pressed_button: None,
                modifiers: Default::default(),
            }),
            cx,
        );
        draw(window, cx);
    })
    .expect("lay out offscreen window");
    handle
}

fn draw(window: &mut Window, cx: &mut App) {
    window.set_scale_factor(2.);
    window.draw(cx).clear(cx);
}

pub(super) fn key(window: &mut Window, cx: &mut App, name: &'static str) {
    let keystroke = Keystroke::parse(name).expect("fixture keystroke");
    window.dispatch_keystroke(keystroke.clone(), cx);
    window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
}

pub(super) fn capture(window: &mut Window, cx: &mut App) -> RgbaImage {
    draw(window, cx);
    window.render_snapshot().expect("capture Metal texture")
}

pub(super) struct SnapshotSuite {
    references: PathBuf,
    output: PathBuf,
    update: bool,
    failures: usize,
}

impl SnapshotSuite {
    pub(super) fn new() -> Self {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let update = env::var("MARSA_UPDATE_SNAPSHOTS").as_deref() == Ok("1");
        assert!(
            !update || env::var_os("CI").is_none(),
            "CI must verify committed snapshots"
        );
        Self {
            references: manifest.join("tests/snapshots"),
            output: env::var_os("MARSA_SNAPSHOT_DIFF_DIR").map_or_else(
                || {
                    manifest
                        .ancestors()
                        .nth(2)
                        .expect("workspace root")
                        .join("target/ui-snapshot-diffs")
                },
                PathBuf::from,
            ),
            update,
            failures: 0,
        }
    }

    pub(super) fn compare(&mut self, fixture: &str, variant: Variant, actual: &RgbaImage) {
        let reference = self
            .references
            .join(fixture)
            .join(format!("{}.png", variant.name));
        if self.update {
            fs::create_dir_all(reference.parent().expect("reference directory"))
                .expect("create reference directory");
            actual.save(&reference).expect("write reference PNG");
            return;
        }
        let output = self.output.join(fixture).join(variant.name);
        let expected = match image::open(&reference) {
            Ok(expected) => expected.into_rgba8(),
            Err(error) => {
                self.failures += 1;
                eprintln!(
                    "{fixture}/{}: could not read {}: {error}",
                    variant.name,
                    reference.display()
                );
                fs::create_dir_all(&output).expect("create failure directory");
                actual
                    .save(output.join("actual.png"))
                    .expect("save candidate PNG");
                return;
            }
        };
        let changed = material_changes(&expected, actual);
        if actual.dimensions() == expected.dimensions() && changed <= ALLOWED_CHANGED_PIXELS {
            return;
        }
        self.failures += 1;
        eprintln!(
            "{fixture}/{}: expected {:?}, rendered {:?}, {changed} changed pixels; see {}",
            variant.name,
            expected.dimensions(),
            actual.dimensions(),
            output.display()
        );
        fs::create_dir_all(&output).expect("create failure directory");
        expected
            .save(output.join("expected.png"))
            .expect("save reference PNG");
        actual
            .save(output.join("actual.png"))
            .expect("save actual PNG");
        difference(&expected, actual)
            .save(output.join("diff.png"))
            .expect("save visual diff PNG");
    }

    pub(super) fn finish(self, fixtures: usize) -> bool {
        if self.failures != 0 {
            eprintln!(
                "{} snapshot mismatches; artifacts in {}",
                self.failures,
                self.output.display()
            );
            return false;
        }
        let action = if self.update { "Updated" } else { "Verified" };
        println!(
            "{action} {} native snapshots for {fixtures} fixtures.",
            fixtures * Variant::ALL.len()
        );
        true
    }
}

// The fixed-scale renderer uses pinned fonts. Ignore small antialiasing changes
// and isolated pixels so the same reference works across Apple GPU generations.
const CHANNEL_DELTA: u8 = 8;
const ALLOWED_CHANGED_PIXELS: usize = 32;

fn changed(expected: &Rgba<u8>, actual: &Rgba<u8>) -> bool {
    expected
        .0
        .into_iter()
        .zip(actual.0)
        .any(|(a, b)| a.abs_diff(b) > CHANNEL_DELTA)
}

fn material_changes(expected: &RgbaImage, actual: &RgbaImage) -> usize {
    expected
        .pixels()
        .zip(actual.pixels())
        .filter(|(expected, actual)| changed(expected, actual))
        .count()
}

fn difference(expected: &RgbaImage, actual: &RgbaImage) -> RgbaImage {
    RgbaImage::from_fn(
        expected.width().max(actual.width()),
        expected.height().max(actual.height()),
        |x, y| match (
            expected.get_pixel_checked(x, y),
            actual.get_pixel_checked(x, y),
        ) {
            (Some(expected), Some(actual)) if !changed(expected, actual) => {
                let [r, g, b, _] = actual.0;
                Rgba([r / 3, g / 3, b / 3, 255])
            }
            _ => Rgba([255, 32, 148, 255]),
        },
    )
}
