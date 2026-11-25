//! Verify interface typography against GPUI's native macOS text system.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    native::run();
}

#[cfg(target_os = "macos")]
mod native {
    use std::{borrow::Cow, rc::Rc};

    use marsa_ui::{
        gpui::{App, Application, Font, FontId, px},
        preferences::{TerminalFont, UiFont, UiFontSize},
        tokens::typography::{Family, Role, rem_size, ui_size},
    };

    pub(super) fn run() {
        Application::with_platform(Rc::new(gpui_macos::MacPlatform::new(true))).run(|cx| {
            verify_terminal_face(cx);
            verify_interface_faces_and_sizes(cx);
            verify_interface_weights(cx);
            std::process::exit(0);
        });
    }

    fn verify_terminal_face(cx: &App) {
        let text = cx.text_system();
        text.add_fonts(vec![Cow::Borrowed(include_bytes!(
            "../../../assets/fonts/catalog/JetBrainsMono.ttf"
        ))])
        .expect("register the bundled JetBrains Mono font");
        let faces = [TerminalFont::System, TerminalFont::JetBrainsMono].map(|preference| {
            let font = Family::available_terminal(preference, cx).font();
            assert_eq!(font, Family::terminal(preference).font());
            let id = direct_font(cx, &font);
            let advance = |character| {
                text.advance(id, px(12.), character)
                    .expect("terminal glyph advance")
                    .width
            };
            assert_eq!(advance('i'), advance('M'), "{preference:?} is monospace");
            id
        });
        assert_ne!(faces[0], faces[1]);
    }

    fn verify_interface_faces_and_sizes(cx: &App) {
        let text = cx.text_system();
        let fonts: [&'static [u8]; 5] = [
            include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf"),
            include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf"),
            include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
            include_bytes!("../../../assets/fonts/IBMPlexSansArabic-Regular.ttf"),
            include_bytes!("../../../assets/fonts/catalog/Inter.ttf"),
        ];
        text.add_fonts(fonts.into_iter().map(Cow::Borrowed).collect())
            .expect("register bundled typography samples");

        let body = |arabic, preference| Role::Body.with_ui_font(arabic, preference);
        let plex = direct_font(cx, &body(false, UiFont::IbmPlex).font());
        let arabic = direct_font(cx, &body(true, UiFont::IbmPlex).font());
        let inter = direct_font(cx, &body(false, UiFont::Inter).font());
        assert_ne!(plex, arabic, "Arabic role selects the bundled Arabic face");
        assert_ne!(
            plex, inter,
            "alternate interface preference changes the face"
        );

        let advances = [
            (UiFontSize::ExtraSmall, 11.),
            (UiFontSize::Small, 12.),
            (UiFontSize::Default, 14.),
            (UiFontSize::Medium, 15.),
            (UiFontSize::Large, 16.),
            (UiFontSize::ExtraLarge, 18.),
        ]
        .map(|(preference, expected)| {
            let size = ui_size(body(false, UiFont::Inter).size).to_pixels(px(rem_size(preference)));
            assert!((f32::from(size) - expected).abs() < 0.01, "{preference:?}");
            text.advance(inter, size, 'M')
                .expect("interface glyph advance at the selected size")
                .width
        });
        assert!(advances.windows(2).all(|pair| pair[0] < pair[1]));
        println!("Inter body advances at the six text sizes: {advances:?}");
    }

    fn verify_interface_weights(cx: &App) {
        let text = cx.text_system();
        let raleway = include_bytes!("../../../assets/fonts/catalog/Raleway.ttf");
        text.add_fonts(vec![Cow::Borrowed(raleway.as_slice())])
            .expect("register the bundled Raleway variable font");
        let select = |role: Role, preference| {
            text.resolve_font(&role.with_ui_font(false, preference).font())
        };
        let normal = select(Role::Body, UiFont::Raleway);
        let medium = select(Role::Label, UiFont::Raleway);
        let semibold = select(Role::Title, UiFont::Raleway);
        assert_ne!(normal, medium, "variable font body and label weights");
        assert_ne!(medium, semibold, "variable font label and title weights");
        for (font, weight, expected) in [
            (normal, 400, 12.277),
            (medium, 500, 12.236),
            (semibold, 600, 12.176),
        ] {
            let advance = text.advance(font, px(14.), 'M').expect("Raleway advance");
            assert!(
                (f32::from(advance.width) - expected).abs() < 0.02,
                "Raleway uses the checked-in {weight}-weight instance: {advance:?}"
            );
        }

        let normal = select(Role::Body, UiFont::IbmPlex);
        let medium = select(Role::Label, UiFont::IbmPlex);
        let semibold = select(Role::Title, UiFont::IbmPlex);
        assert_ne!(normal, medium, "static font body and label weights");
        assert_ne!(medium, semibold, "static font label and title weights");
    }

    fn direct_font(cx: &App, font: &Font) -> FontId {
        let text = cx.text_system();
        let id = text.resolve_font(font);
        assert_eq!(
            text.get_font_for_id(id).as_ref(),
            Some(font),
            "{} must resolve directly",
            font.family,
        );
        id
    }
}
