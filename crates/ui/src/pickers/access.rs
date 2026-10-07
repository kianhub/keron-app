//! The session's permission mode, in the session footer. A session asks
//! before its agent acts (the default). Full access (no approval prompts, no
//! sandbox) is chosen explicitly from this menu, applies to this session
//! only, and shows as a warning-tinted badge for as long as it is on. The
//! host enforces the choice from the session's config row.
use super::*;

const ACCESS_MENU_WIDTH: f32 = 264.0;

impl Pickers {
    /// Whether the selected session chose full access.
    pub(super) fn session_full_access(&self, cx: &App) -> bool {
        self.state
            .read(cx)
            .selected_chat_row()
            .and_then(|chat| chat.config.as_ref())
            .is_some_and(|config| config.sandbox == SandboxLevel::DangerFullAccess)
    }

    /// The footer trigger: a quiet "Ask first", or the "Full access" badge.
    pub(super) fn access_chip(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        if self.session_full_access(cx) {
            self.footer_chip_tinted(
                PickerKind::Access,
                "picker-access",
                crate::icons::DANGER_TRIANGLE,
                SharedString::from("Full access"),
                Some(theme.warning),
                theme,
                cx,
            )
        } else {
            self.footer_chip(
                PickerKind::Access,
                "picker-access",
                crate::icons::KEY_MINIMALISTIC,
                SharedString::from("Ask first"),
                theme,
                cx,
            )
        }
    }

    pub(super) fn render_access_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let full = self.session_full_access(cx);
        let options: [(bool, &'static str, &'static str); 2] = [
            (false, "Ask first", crate::icons::KEY_MINIMALISTIC),
            (true, "Full access", crate::icons::DANGER_TRIANGLE),
        ];
        let active = self.active;
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(ix, (choice, label, icon_path))| {
                        popover::menu_row_nav(
                            &theme,
                            full == choice,
                            ix == active,
                            format!("access-row-{ix}"),
                        )
                        .id(("access-row", ix))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_access(choice, cx);
                        }))
                        .child(
                            crate::icons::icon(icon_path)
                                .size(px(14.0))
                                .text_color(if choice {
                                    theme.warning
                                } else {
                                    theme.text_muted
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(label)),
                        )
                    }),
            )
            .child(
                div()
                    .px(px(8.0))
                    .pt(px(4.0))
                    .pb(px(2.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(
                        "Full access turns off approval prompts and the \
                         sandbox, for this session only.",
                    )),
            )
            .into_any_element()
    }

    /// Apply the session's permission mode. Turning full access off returns
    /// the session to asking in its workspace sandbox.
    pub(super) fn pick_access(&mut self, full: bool, cx: &mut Context<Self>) {
        if full != self.session_full_access(cx) {
            self.update_chat_config(cx, move |config| {
                config.sandbox = if full {
                    SandboxLevel::DangerFullAccess
                } else {
                    SandboxLevel::WorkspaceWrite
                };
            });
        }
        self.close(cx);
    }

    /// The access menu, mounted on its footer chip.
    pub(super) fn access_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(PickerKind, AnyElement)> {
        (self.mounted_kind() == Some(PickerKind::Access)).then(|| {
            let content = self.render_access_popover(cx);
            (
                PickerKind::Access,
                self.popover_frame(ACCESS_MENU_WIDTH, content, cx),
            )
        })
    }
}
