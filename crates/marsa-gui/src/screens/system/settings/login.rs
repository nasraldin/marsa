//! Start-at-login setting and macOS approval state.

use crate::screens::common::message;
use marsa_client::login::{LoginAction, LoginStatus};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    Locale,
    gpui::{Context, Div, ParentElement, PromptButton, PromptLevel, Window, div},
    gpui_component::{Disableable, checkbox::Checkbox},
};

use super::{
    super::{Notice, SystemView},
    setting,
};
use crate::screens::common::ScreenContext;

impl SystemView {
    pub(in crate::screens::system) fn load_login(&mut self, cx: &Context<Self>) {
        let Some(services) = &self.services else {
            return;
        };
        let mut pending = services.configure_login(LoginAction::Status);
        self.route_load = Some(cx.spawn(async move |this, cx| {
            let result = pending.result().await;
            let _ = this.update(cx, |this, cx| {
                this.login = result.and_then(Result::ok);
                cx.notify();
            });
        }));
    }

    pub(super) fn login_setting(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let (en, ar) = match self.login {
            Some(LoginStatus::RequiresApproval) => (
                "Allow Marsa in System Settings under General → Login Items & Extensions.",
                "اسمح لمرسى من إعدادات النظام، ضمن عام ← عناصر تسجيل الدخول والامتدادات.",
            ),
            None => (
                "Login item status is unavailable. Open the signed Marsa app to change it.",
                "حالة عنصر تسجيل الدخول غير متاحة. افتح تطبيق مرسى الموقّع لتغييرها.",
            ),
            _ => (
                "Start the Marsa service when you sign in.",
                "شغّل خدمة مرسى عند تسجيل الدخول.",
            ),
        };
        setting(
            screen,
            "Start at login",
            "بدء التشغيل عند تسجيل الدخول",
            en,
            ar,
            div().child(
                Checkbox::new("start-at-login")
                    .checked(
                        self.login
                            .is_some_and(|status| status != LoginStatus::Disabled),
                    )
                    .label(message(screen.locale, gui::GUI_DESKTOP_START_SERVICE))
                    .disabled(self.login.is_none() || self.busy())
                    .on_click(cx.listener(move |this, enabled: &bool, window, cx| {
                        this.change_login(*enabled, locale, window, cx);
                    })),
            ),
        )
    }

    fn change_login(
        &mut self,
        enabled: bool,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            return;
        }
        if enabled {
            self.apply_login(LoginAction::Enable, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            gui::GUI_DESKTOP_DISABLE_START_AT_LOGIN.text(locale),
            Some(
                gui::GUI_DESKTOP_MACOS_MAY_STOP_THE_RUNNING_SERVICE_SAVE_WORK_IN_RUNNING_CONTAI
                    .text(locale),
            ),
            &[
                PromptButton::Ok(gui::GUI_DESKTOP_DISABLE.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        self.command = Some(cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| this.apply_login(LoginAction::Disable, cx));
            }
        }));
    }

    fn apply_login(&mut self, action: LoginAction, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        if let Some(services) = &self.services {
            self.action(
                services.configure_login(action),
                |this, status, _| {
                    this.login = Some(status);
                    this.notice = Some(Notice::Saved);
                },
                cx,
            );
        }
    }
}
