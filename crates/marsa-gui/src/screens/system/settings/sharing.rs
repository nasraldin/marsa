//! Explicit folder sharing and owner responses to pending bind requests.

use crate::screens::common::message;
use marsa_client::api::{
    AddShareRequest, BindApprovalDecision, RemoveShareRequest, ResolveBindRequest,
};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{
        Context, Div, ParentElement, PathPromptOptions, PromptButton, PromptLevel, Styled, Window,
        div,
    },
    gpui_component::{
        Disableable,
        button::{Button, ButtonVariants},
    },
};

use crate::screens::common::{ScreenContext, color, panel, row, technical};

use super::super::{Notice, SystemView, machine::title_text};

impl SystemView {
    pub(in crate::screens::system) fn sharing(
        &self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let snapshot = screen.state.daemon();
        let shares = snapshot.and_then(|snapshot| snapshot.shares.as_ref());
        let binds = snapshot.and_then(|snapshot| snapshot.bind_approvals.as_ref());
        let disabled = self.busy() || snapshot.is_none();
        div().flex().flex_col().gap_4()
            .child(panel(screen)
                .child(row(locale).justify_between()
                    .child(div().child(title_text(gui::GUI_DESKTOP_SHARED_FOLDERS.text(screen.locale)))
                        .child(message(screen.locale, gui::GUI_DESKTOP_CONTAINERS_CAN_REQUEST_ACCESS_TO_OTHER_FOLDERS_WHEN_NEEDED)))
                    .child(Button::new("add-folder").primary().disabled(disabled)
                        .label(message(screen.locale, gui::GUI_DESKTOP_ADD_FOLDER))
                        .on_click(cx.listener(move |this, _, window, cx| this.choose_folder(locale, window, cx)))))
                .children(shares.is_some_and(|shares| shares.shares.is_empty()).then(|| message(screen.locale, gui::GUI_DESKTOP_NO_SHARED_FOLDERS)))
                .children(shares.into_iter().flat_map(|shares| &shares.shares).map(|share| {
                    let id = share.id.clone();
                    let path = share.host_path.clone();
                    row(locale).justify_between().gap_3().border_t_1().py_2()
                        .border_color(color(screen.palette, ColorToken::BorderSubtle))
                        .child(div().flex_1().min_w_0().child(technical(&path)))
                        .child(if share.read_only { message(screen.locale, gui::GUI_DESKTOP_READ_ONLY) } else { message(screen.locale, gui::GUI_DESKTOP_READ_AND_WRITE) })
                        .children(share.requires_restart.then(|| message(screen.locale, gui::GUI_DESKTOP_RESTART_REQUIRED)))
                        .child(Button::new(format!("remove-share-{id}")).outline().disabled(disabled)
                            .label(message(screen.locale, gui::GUI_DESKTOP_REMOVE))
                            .on_click(cx.listener(move |this, _, window, cx| this.confirm_unshare(id.clone(), &path, locale, window, cx))))
                })))
            .child(panel(screen)
                .child(title_text(gui::GUI_DESKTOP_PROTECTED_LOCATIONS.text(screen.locale)))
                .child(message(screen.locale, gui::GUI_DESKTOP_CREDENTIAL_FOLDERS_ARE_ALWAYS_EXCLUDED_EVEN_WHEN_A_PARENT_FOLD))
                .child(technical("~/.ssh   ·   ~/.aws   ·   ~/.gnupg   ·   ~/.kube   ·   ~/.docker   ·   ~/.marsa")))
            .children(binds.filter(|binds| !binds.approvals.is_empty()).map(|binds| {
                panel(screen).child(title_text(gui::GUI_DESKTOP_FOLDER_ACCESS_REQUESTS.text(screen.locale)))
                    .children(binds.approvals.iter().map(|bind| {
                        div().flex().flex_col().gap_2().border_t_1().border_color(color(screen.palette, ColorToken::BorderSubtle)).py_2()
                            .children(bind.directories.iter().map(|path| technical(path)))
                            .child(message(screen.locale, gui::GUI_DESKTOP_A_STATIC_SHARE_BECOMES_AVAILABLE_AFTER_YOU_RESTART_THE_MACHINE))
                            .child(row(locale).gap_2().children([
                                (BindApprovalDecision::Once, message(screen.locale, gui::GUI_DESKTOP_ALLOW_ONCE)),
                                (BindApprovalDecision::Persist, message(screen.locale, gui::GUI_DESKTOP_ALWAYS_ALLOW)),
                                (BindApprovalDecision::Deny, message(screen.locale, gui::GUI_DESKTOP_DENY)),
                            ].into_iter().map(|(decision, text)| {
                                let id = bind.id.clone();
                                Button::new(format!("bind-{}-{decision:?}", bind.id)).outline().disabled(disabled).label(text)
                                    .on_click(cx.listener(move |this, _, _, cx| this.resolve_bind(id.clone(), decision, cx)))
                            })))
                    }))
            }))
    }

    fn choose_folder(&mut self, locale: Locale, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let path = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                gui::GUI_DESKTOP_CHOOSE_A_FOLDER_TO_SHARE
                    .text(locale)
                    .into(),
            ),
        });
        self.command = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = path.await else {
                return;
            };
            let Some(host_path) = paths
                .first()
                .and_then(|path| path.to_str())
                .map(str::to_owned)
            else {
                return;
            };
            let confirm = cx.update(|window, cx| {
                window.prompt(
                    PromptLevel::Info,
                    gui::GUI_DESKTOP_SHARE_THIS_FOLDER.text(locale),
                    Some(technical(&host_path).as_ref()),
                    &[
                        PromptButton::Ok(gui::GUI_DESKTOP_READ_ONLY.text(locale).into()),
                        PromptButton::Other(gui::GUI_DESKTOP_READ_AND_WRITE.text(locale).into()),
                        PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
                    ],
                    cx,
                )
            });
            let Ok(confirm) = confirm else {
                return;
            };
            let Ok(choice @ (0 | 1)) = confirm.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.add_share(host_path, choice == 0, cx));
        }));
    }

    fn add_share(&mut self, host_path: String, read_only: bool, cx: &mut Context<Self>) {
        let Some(request_id) = self.mutation_id(cx) else {
            return;
        };
        self.request(
            move |client| async move {
                Ok(client
                    .shares()
                    .add(AddShareRequest {
                        request_id,
                        host_path,
                        read_only,
                        approval_id: None,
                        persist: true,
                    })
                    .await?
                    .into_inner())
            },
            |this, share, _| this.share_changed(share.requires_restart),
            cx,
        );
    }

    fn confirm_unshare(
        &mut self,
        share_id: String,
        path: &str,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            return;
        }
        let path = technical(path);
        let answer = window.prompt(
            PromptLevel::Warning,
            gui::GUI_DESKTOP_REMOVE_FOLDER_ACCESS.text(locale),
            Some(path.as_ref()),
            &[
                PromptButton::Ok(gui::GUI_DESKTOP_REMOVE.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        self.command = Some(cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| {
                    let Some(request_id) = this.mutation_id(cx) else {
                        return;
                    };
                    this.request(
                        move |client| async move {
                            Ok(client
                                .shares()
                                .remove(RemoveShareRequest {
                                    request_id,
                                    share_id,
                                })
                                .await?
                                .into_inner())
                        },
                        |this, result, _| this.share_changed(result.requires_restart),
                        cx,
                    );
                });
            }
        }));
    }

    fn resolve_bind(
        &mut self,
        approval_id: String,
        decision: BindApprovalDecision,
        cx: &mut Context<Self>,
    ) {
        let Some(request_id) = self.mutation_id(cx) else {
            return;
        };
        self.request(
            move |client| async move {
                Ok(client
                    .shares()
                    .resolve_bind(ResolveBindRequest {
                        request_id,
                        approval_id,
                        decision: decision as i32,
                    })
                    .await?
                    .into_inner())
            },
            |this, result, _| this.share_changed(result.requires_restart),
            cx,
        );
    }

    fn share_changed(&mut self, requires_restart: bool) {
        self.notice = Some(if requires_restart {
            Notice::AppliedNextStart
        } else {
            Notice::Saved
        });
    }
}
