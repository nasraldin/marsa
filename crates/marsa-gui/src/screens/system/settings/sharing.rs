//! Explicit folder sharing and owner responses to pending bind requests.

use marsa_client::api::{
    AddShareRequest, BindApproval, BindApprovalDecision, RemoveShareRequest, ResolveBindRequest,
};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{
        Context, Div, ParentElement, PathPromptOptions, PromptButton, PromptLevel, Styled, Window,
        div, px,
    },
    gpui_component::{
        Disableable, WindowExt,
        button::{Button, ButtonVariants},
        menu::{DropdownMenu, PopupMenuItem},
    },
    tokens::{
        scale::{Radius, Space},
        typography::Role,
    },
};

use crate::screens::common::{ScreenContext, color, row, technical};

use super::{
    super::{Notice, SystemView},
    settings_card, settings_heading, settings_icon, settings_text,
};

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
        let title = gui::GUI_DESKTOP_SHARED_FOLDERS.text(locale);
        let folder_list = settings_card(screen)
            .min_h(px(280.))
            .gap(px(Space::S16.pixels()))
            .child(
                row(locale)
                    .justify_between()
                    .items_start()
                    .child(settings_heading(
                        screen,
                        title,
                        gui::GUI_DESKTOP_CONTAINERS_CAN_REQUEST_ACCESS_TO_OTHER_FOLDERS_WHEN_NEEDED
                            .text(locale),
                    ))
                    .child(
                        Button::new("add-folder")
                            .primary()
                            .h(px(Space::S36.pixels()))
                            .rounded(px(Radius::R8.pixels()))
                            .disabled(disabled)
                            .label(gui::GUI_DESKTOP_ADD_FOLDER.text(locale))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose_folder(locale, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(
                        shares
                            .is_some_and(|shares| shares.shares.is_empty())
                            .then(|| {
                                settings_text(
                                    screen,
                                    Role::Small,
                                    gui::GUI_DESKTOP_NO_SHARED_FOLDERS.text(locale),
                                    ColorToken::TextMuted,
                                )
                            }),
                    )
                    .children(
                        shares
                            .into_iter()
                            .flat_map(|shares| &shares.shares)
                            .map(|share| {
                                let id = share.id.clone();
                                let path = share.host_path.clone();
                                let viewer = cx.weak_entity();
                                let access = if share.read_only {
                                    gui::GUI_DESKTOP_READ_ONLY
                                } else {
                                    gui::GUI_DESKTOP_READ_AND_WRITE
                                };
                                row(locale)
                                    .gap(px(Space::S16.pixels()))
                                    .min_h(px(Space::S48.pixels() + Space::S8.pixels()))
                                    .border_t_1()
                                    .border_color(color(screen.palette, ColorToken::BorderSubtle))
                                    .child(settings_icon(
                                        screen,
                                        "icons/folder-open.svg",
                                        20.,
                                        ColorToken::TextMuted,
                                    ))
                                    .child(
                                        settings_text(
                                            screen,
                                            Role::Mono,
                                            technical(&share.host_path),
                                            ColorToken::TextPrimary,
                                        )
                                        .flex_1()
                                        .min_w_0()
                                        .truncate(),
                                    )
                                    .child(
                                        div()
                                            .w(px(172.))
                                            .flex_shrink_0()
                                            .child(settings_text(
                                                screen,
                                                Role::Label,
                                                access.text(locale),
                                                ColorToken::TextMuted,
                                            ))
                                            .children(share.requires_restart.then(|| {
                                                settings_text(
                                                    screen,
                                                    Role::Small,
                                                    gui::GUI_DESKTOP_RESTART_REQUIRED.text(locale),
                                                    ColorToken::StatusWarning,
                                                )
                                            })),
                                    )
                                    .child(
                                        Button::new(format!("share-actions-{id}"))
                                            .ghost()
                                            .disabled(disabled)
                                            .accessibility_label(
                                                gui::GUI_DESKTOP_SHARED_FOLDER_ACTIONS.text(locale),
                                            )
                                            .child(settings_icon(
                                                screen,
                                                "icons/ellipsis.svg",
                                                Space::S16.pixels(),
                                                ColorToken::TextMuted,
                                            ))
                                            .dropdown_menu(move |menu, _, _| {
                                                let id = id.clone();
                                                let path = path.clone();
                                                let viewer = viewer.clone();
                                                menu.item(
                                                    PopupMenuItem::new(
                                                        gui::GUI_DESKTOP_REMOVE.text(locale),
                                                    )
                                                    .on_click(move |_, window, cx| {
                                                        let _ = viewer.update(cx, |this, cx| {
                                                            this.confirm_unshare(
                                                                id.clone(),
                                                                &path,
                                                                locale,
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    }),
                                                )
                                            }),
                                    )
                            }),
                    ),
            );
        let protected = row(locale)
            .items_start()
            .gap(px(Space::S8.pixels()))
            .min_h(px(Space::S72.pixels() + Space::S4.pixels()))
            .rounded(px(Radius::R8.pixels()))
            .p(px(Space::S12.pixels()))
            .bg(color(screen.palette, ColorToken::StatusStartingBg))
            .child(settings_icon(screen, "icons/info.svg", 20., ColorToken::TextMuted))
            .child(div().flex().flex_col().gap(px(Space::S4.pixels()))
                .child(settings_text(screen, Role::Label, gui::GUI_DESKTOP_PROTECTED_LOCATIONS.text(locale), ColorToken::StatusStarting))
                .child(settings_text(screen, Role::Small,
                    gui::GUI_DESKTOP_CREDENTIAL_FOLDERS_ARE_ALWAYS_EXCLUDED_EVEN_WHEN_A_PARENT_FOLD.text(locale),
                    ColorToken::TextPrimary)));
        let examples = row(Locale::EnglishUs)
            .min_h(px(Space::S48.pixels() + Space::S8.pixels()))
            .px(px(Space::S16.pixels()))
            .rounded(px(Radius::R8.pixels()))
            .border_1()
            .border_color(color(screen.palette, ColorToken::BorderSubtle))
            .bg(color(screen.palette, ColorToken::BgCode))
            .child(settings_text(screen, Role::Mono,
                technical("~/.ssh   ·   ~/.aws   ·   ~/.gnupg   ·   ~/.kube   ·   ~/.docker   ·   ~/.marsa"),
                ColorToken::TextPrimary));

        div()
            .flex()
            .flex_col()
            .gap(px(Space::S16.pixels()))
            .child(folder_list)
            .child(protected)
            .child(examples)
            .children(
                binds
                    .filter(|binds| !binds.approvals.is_empty())
                    .map(|binds| {
                        settings_card(screen)
                            .child(settings_text(
                                screen,
                                Role::Lead,
                                gui::GUI_DESKTOP_FOLDER_ACCESS_REQUESTS.text(locale),
                                ColorToken::TextPrimary,
                            ))
                            .children(binds.approvals.iter().map(|bind| {
                                let pending = bind.clone();
                                let palette = screen.palette;
                                row(locale)
                                    .gap(px(Space::S16.pixels()))
                                    .justify_between()
                                    .border_t_1()
                                    .border_color(color(palette, ColorToken::BorderSubtle))
                                    .py(px(Space::S8.pixels()))
                                    .child(div().flex().flex_col().flex_1().min_w_0().children(
                                        bind.directories.iter().map(|path| {
                                            settings_text(
                                                screen,
                                                Role::Mono,
                                                technical(path),
                                                ColorToken::TextPrimary,
                                            )
                                            .truncate()
                                        }),
                                    ))
                                    .child(
                                        Button::new(format!("review-bind-{}", bind.id))
                                            .outline()
                                            .disabled(disabled)
                                            .label(gui::GUI_DESKTOP_BIND_REVIEW.text(locale))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.review_bind(
                                                    pending.clone(),
                                                    locale,
                                                    palette,
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    )
                            }))
                    }),
            )
    }

    fn review_bind(
        &mut self,
        approval: BindApproval,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(service) = self.services.clone().filter(|_| !self.busy()) else {
            return;
        };
        let view = cx.weak_entity();
        let state = service.store().select(Clone::clone).current().clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let screen = ScreenContext { locale, palette, state: &state, services: Some(&service) };
            let title = if locale == Locale::Arabic { Role::Title } else { Role::Section };
            let busy = view.upgrade().is_none_or(|view| view.read(cx).busy());
            let paths = div().flex().flex_col()
                .gap(px(Space::S8.pixels()))
                .p(px(Space::S16.pixels()))
                .rounded(px(Radius::R8.pixels()))
                .border_1()
                .border_color(color(palette, ColorToken::BorderSubtle))
                .bg(color(palette, ColorToken::BgCode))
                .children(approval.directories.iter().map(|path| {
                    row(locale)
                        .gap(px(Space::S16.pixels()))
                        .child(settings_text(&screen, Role::Small,
                            gui::GUI_DESKTOP_BIND_HOST_FOLDER.text(locale), ColorToken::TextMuted)
                            .w(px(144.)).flex_shrink_0())
                        .child(settings_text(&screen, Role::Mono, technical(path), ColorToken::TextPrimary)
                            .flex_1().min_w_0())
                }));
            let explanation = row(locale)
                .items_start()
                .gap(px(Space::S12.pixels()))
                .p(px(Space::S16.pixels()))
                .rounded(px(Radius::R8.pixels()))
                .bg(color(palette, ColorToken::StatusStartingBg))
                .child(settings_icon(&screen, "icons/info.svg", 20., ColorToken::TextMuted))
                .child(div().flex().flex_col().gap(px(Space::S4.pixels()))
                    .child(settings_text(&screen, Role::Label,
                        gui::GUI_DESKTOP_BIND_SESSION_OR_FUTURE.text(locale), ColorToken::StatusStarting))
                    .child(settings_text(&screen, Role::Small,
                        gui::GUI_DESKTOP_BIND_OPTION_EXPLANATION.text(locale), ColorToken::TextPrimary))
                    .child(settings_text(&screen, Role::Small,
                        gui::GUI_DESKTOP_A_STATIC_SHARE_BECOMES_AVAILABLE_AFTER_YOU_RESTART_THE_MACHINE.text(locale),
                        ColorToken::TextMuted)));
            let action = |decision, label: &'static str| {
                let view = view.clone();
                let id = approval.id.clone();
                let button = Button::new(format!("review-bind-{decision:?}"))
                    .label(label)
                    .h(px(Space::S36.pixels()))
                    .rounded(px(Radius::R8.pixels()))
                    .disabled(busy)
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let _ = view.update(cx, |view, cx| view.resolve_bind(id.clone(), decision, cx));
                    });
                if decision == BindApprovalDecision::Once { button.primary() } else { button.outline() }
            };
            let actions = row(locale)
                .justify_end()
                .gap(px(Space::S8.pixels()))
                .pt(px(Space::S16.pixels()))
                .border_t_1()
                .border_color(color(palette, ColorToken::BorderSubtle))
                .child(action(BindApprovalDecision::Deny, gui::GUI_DESKTOP_DENY.text(locale)))
                .children((locale == Locale::EnglishUs).then(|| div().flex_1()))
                .child(action(BindApprovalDecision::Persist, gui::GUI_DESKTOP_BIND_SHARE_ALWAYS.text(locale)))
                .child(action(BindApprovalDecision::Once, gui::GUI_DESKTOP_BIND_SHARE_ONCE.text(locale)));
            dialog
                .width(px(680.))
                .overlay_closable(false)
                .title(settings_text(&screen, title, gui::GUI_DESKTOP_SHARE_THIS_FOLDER.text(locale), ColorToken::TextPrimary))
                .bg(color(palette, ColorToken::GlassRail))
                .border_color(color(palette, ColorToken::GlassEdge))
                .rounded(px(Radius::R16.pixels()))
                .child(div().flex().flex_col().gap(px(Space::S16.pixels()))
                    .child(settings_text(&screen, Role::Body,
                        gui::GUI_DESKTOP_BIND_DIALOG_DESCRIPTION.text(locale), ColorToken::TextMuted))
                    .child(paths)
                    .child(explanation)
                    .child(actions))
        });
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
