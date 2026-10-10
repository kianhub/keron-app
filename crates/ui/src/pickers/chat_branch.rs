//! The session footer's branch dropdown. The chip names the branch the
//! chat's checkout has checked out now, as git reports it on the chat's host
//! (`ListChatBranches`), not the branch the chat was recorded on, which goes
//! stale once the agent switches. It re-reads when the popover opens, when
//! the chat's run ends and when the window comes back.
//!
//! Picking a branch switches that checkout (`SwitchChatBranch`); "New
//! branch…" names one in the search field and makes it at HEAD. Each guard
//! says so in the popover: a working agent, a branch another checkout has
//! checked out, git's own refusal. A Local checkout is shared, so the popover
//! says a switch moves every chat in the folder.
//!
//! Remote chats take the same path: both calls carry `targetDeviceId` and
//! the relay forwards them to the chat's host, which resolves the folder from
//! the chat's row and runs git there.
use super::*;

use zeron_proto::{Chat, ChatIndicator, CheckoutBranch, CheckoutBranches};

const CHAT_BRANCH_MENU_WIDTH: f32 = 320.0;

/// The chip's label: the live branch, a detached HEAD's short commit, and
/// until git answers, the branch the chat was recorded on.
pub(super) fn chat_branch_label(
    live: Option<&CheckoutBranches>,
    recorded: Option<&str>,
) -> SharedString {
    match live {
        Some(live) => live.current.clone().or_else(|| live.head.clone()),
        None => recorded.map(str::to_owned),
    }
    .map(SharedString::from)
    .unwrap_or_else(|| SharedString::from("No ref"))
}

/// The worktree folder a chat runs in, or `None` for the project's own
/// folder (a Local checkout).
pub(crate) fn chat_worktree<'a>(chat: &'a Chat, space: &Space) -> Option<&'a str> {
    let trim = |path: &str| path.trim_end_matches(['/', '\\']).to_owned();
    chat.cwd
        .as_deref()
        .filter(|cwd| space.git_detected && trim(cwd) != trim(&space.path))
}

/// The last component of a folder path: a worktree's name.
pub(crate) fn folder_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

impl Pickers {
    /// The branch chip and popover serve the selected chat, not a draft.
    pub(super) fn in_session(&self, cx: &App) -> bool {
        self.state.read(cx).selected_chat_row().is_some()
    }

    /// Whether the selected chat's agent is working or waiting on an answer
    /// (a send in flight counts).
    pub(super) fn selected_chat_running(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        state
            .selected_chat_row()
            .is_some_and(|chat| chat_running(state, chat))
    }

    /// The branch the selected chat's checkout is switching to. A switch
    /// another chat started doesn't hold this chat's rows.
    fn switching_branch(&self, cx: &App) -> Option<&str> {
        let selected = self.state.read(cx).selected_chat.as_deref()?;
        self.chat_switch
            .as_ref()
            .filter(|(chat, _)| chat == selected)
            .map(|(_, branch)| branch.as_str())
    }

    /// Why the selected chat's checkout can't switch right now. Switching
    /// under a working agent breaks its work, whichever chat it serves.
    fn switch_block(&self, cx: &App) -> Option<&'static str> {
        let state = self.state.read(cx);
        let chat = state.selected_chat_row()?;
        if chat_running(state, chat) {
            return Some("The agent is running. Switch when it stops.");
        }
        let cwd = chat.cwd.as_deref()?;
        state
            .chats
            .iter()
            .any(|other| {
                other.id != chat.id
                    && other.device_id == chat.device_id
                    && other.cwd.as_deref() == Some(cwd)
                    && chat_running(state, other)
            })
            .then_some("Another chat's agent is running in this folder.")
    }

    /// Load the selected chat's branches from its host. Non-forced (the
    /// footer's eager kick, every render) only loads from Idle; forced loads
    /// keep the current rows on screen until fresh ones land.
    pub(super) fn ensure_chat_branches(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some((chat, local)) = ({
            let state = self.state.read(cx);
            state
                .selected_chat_row()
                .filter(|chat| {
                    state
                        .space_for_chat(chat)
                        .is_some_and(|space| space.git_detected)
                })
                .map(|chat| (chat.clone(), state.local_device_id.clone()))
        }) else {
            return;
        };
        let fresh = self.chat_branches_owner.as_deref() == Some(chat.id.as_str());
        if fresh && matches!(self.chat_branches, Loadable::Loading) {
            return;
        }
        if !force && fresh && !matches!(self.chat_branches, Loadable::Idle) {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        if !(fresh && matches!(self.chat_branches, Loadable::Ready(_))) {
            self.chat_branches = Loadable::Loading;
        }
        self.chat_branches_owner = Some(chat.id.clone());
        self.chat_branches_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::LIST_CHAT_BRANCHES,
                    chat_branch_params(&chat, local.as_deref(), None),
                )
                .await
                .map_err(|err| err.to_string())
                .and_then(|value| {
                    serde_json::from_value::<CheckoutBranches>(value).map_err(|e| e.to_string())
                });
            this.update(cx, |pickers, cx| {
                if pickers.chat_branches_owner.as_deref() != Some(chat.id.as_str()) {
                    return;
                }
                pickers.chat_branches = match result {
                    Ok(branches) => Loadable::Ready(branches),
                    Err(err) => Loadable::Error(err),
                };
                if pickers.open_kind() == Some(PickerKind::Branch)
                    && !pickers.naming_branch
                    && pickers.search.read(cx).text().is_empty()
                {
                    pickers.active = pickers.chat_branch_index(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn filtered_chat_branch_rows(&self, cx: &App) -> Vec<CheckoutBranch> {
        let Some(listing) = self.chat_branches.ready() else {
            return Vec::new();
        };
        let names: Vec<&str> = listing.branches.iter().map(|b| b.name.as_str()).collect();
        let query = self.search.read(cx).text().to_string();
        popover::filter_indices(&query, &names)
            .into_iter()
            .take(MAX_REF_ROWS)
            .map(|ix| listing.branches[ix].clone())
            .collect()
    }

    /// The keyboard highlight's home: the current branch's row.
    pub(super) fn chat_branch_index(&self, cx: &App) -> usize {
        let current = self
            .chat_branches
            .ready()
            .and_then(|listing| listing.current.clone());
        self.filtered_chat_branch_rows(cx)
            .iter()
            .position(|row| Some(&row.name) == current.as_ref())
            .unwrap_or(0)
    }

    /// Keyboard rows: the branches, then "New branch…" (none while naming).
    pub(super) fn chat_branch_row_count(&self, cx: &App) -> usize {
        if self.naming_branch || self.chat_branches.ready().is_none() {
            0
        } else {
            self.filtered_chat_branch_rows(cx).len() + 1
        }
    }

    /// Enter in the chat branch popover: create the named branch, or pick
    /// the highlighted row.
    pub(super) fn submit_chat_branch(&mut self, cx: &mut Context<Self>) {
        if self.naming_branch {
            let name = self.search.read(cx).text().trim().to_string();
            if !name.is_empty() && self.switch_block(cx).is_none() {
                self.switch_chat_branch(name, true, cx);
            }
            return;
        }
        let rows = self.filtered_chat_branch_rows(cx);
        match rows.into_iter().nth(self.active) {
            Some(row) => self.pick_chat_branch(row, cx),
            None if self.chat_branches.ready().is_some() => self.start_naming_branch(cx),
            None => {}
        }
    }

    fn pick_chat_branch(&mut self, row: CheckoutBranch, cx: &mut Context<Self>) {
        if self.switch_block(cx).is_some() || row.checked_out_at.is_some() {
            return;
        }
        let current = self
            .chat_branches
            .ready()
            .and_then(|listing| listing.current.as_deref());
        if current == Some(row.name.as_str()) {
            self.close(cx);
            return;
        }
        self.switch_chat_branch(row.name, false, cx);
    }

    /// "New branch…": the search field takes the new branch's name, starting
    /// from whatever was typed to search.
    fn start_naming_branch(&mut self, cx: &mut Context<Self>) {
        if self.switch_block(cx).is_some() {
            return;
        }
        self.naming_branch = true;
        self.chat_switch_error = None;
        self.search.update(cx, |input, cx| {
            input.set_placeholder("New branch name", cx);
        });
        cx.notify();
    }

    /// Escape while naming goes back to the branch list.
    pub(super) fn stop_naming_branch(&mut self, cx: &mut Context<Self>) {
        self.naming_branch = false;
        self.chat_switch_error = None;
        self.search_reset_muted = !self.search.read(cx).text().is_empty();
        self.search.update(cx, |input, cx| {
            input.set_placeholder("Search branches…", cx);
            if !input.text().is_empty() {
                input.set_text("", cx);
            }
        });
        self.active = self.chat_branch_index(cx);
        cx.notify();
    }

    /// `SwitchChatBranch` on the chat's host. Success replaces the listing
    /// and closes; a refusal keeps the popover open with the reason.
    fn switch_chat_branch(&mut self, name: String, create: bool, cx: &mut Context<Self>) {
        if self.switching_branch(cx).is_some() {
            return; // one switch at a time per chat
        }
        let Some((chat, local)) = ({
            let state = self.state.read(cx);
            state
                .selected_chat_row()
                .map(|chat| (chat.clone(), state.local_device_id.clone()))
        }) else {
            return;
        };
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.chat_switch_error = None;
        self.chat_switch = Some((chat.id.clone(), name.clone()));
        // A switch another chat left in flight still settles itself.
        if let Some(earlier) = self.chat_switch_task.take() {
            earlier.detach();
        }
        self.chat_switch_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::SWITCH_CHAT_BRANCH,
                    chat_branch_params(&chat, local.as_deref(), Some((&name, create))),
                )
                .await
                .map_err(|err| err.to_string())
                .and_then(|value| {
                    serde_json::from_value::<CheckoutBranches>(value).map_err(|e| e.to_string())
                });
            this.update(cx, |pickers, cx| {
                pickers.finish_chat_switch(&chat.id, &name, result, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// A switch landed. Its chat's listing takes the result; the popover
    /// closes or shows git's reason only while that chat is still selected,
    /// never in another chat's popover.
    fn finish_chat_switch(
        &mut self,
        chat_id: &str,
        branch: &str,
        result: Result<CheckoutBranches, String>,
        cx: &mut Context<Self>,
    ) {
        if self
            .chat_switch
            .as_ref()
            .is_some_and(|(chat, name)| chat == chat_id && name == branch)
        {
            self.chat_switch = None;
        }
        let selected = self.state.read(cx).selected_chat.as_deref() == Some(chat_id);
        match result {
            Ok(branches) => {
                if self.chat_branches_owner.as_deref() == Some(chat_id) {
                    self.chat_branches = Loadable::Ready(branches);
                }
                if selected {
                    self.naming_branch = false;
                    if self.open_kind() == Some(PickerKind::Branch) {
                        self.close(cx);
                    }
                }
            }
            Err(err) if selected => self.chat_switch_error = Some(err),
            Err(_) => {}
        }
        cx.notify();
    }

    /// The chat branch popover, mounted on the footer's branch chip.
    pub(super) fn chat_branch_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(PickerKind, AnyElement)> {
        (self.mounted_kind() == Some(PickerKind::Branch)).then(|| {
            let content = self.render_chat_branch_popover(cx);
            (
                PickerKind::Branch,
                self.popover_frame(CHAT_BRANCH_MENU_WIDTH, content, cx),
            )
        })
    }

    fn render_chat_branch_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let block = self.switch_block(cx);
        let local_checkout = {
            let state = self.state.read(cx);
            state.selected_chat_row().is_some_and(|chat| {
                state
                    .space_for_chat(chat)
                    .is_some_and(|space| chat_worktree(chat, space).is_none())
            })
        };
        let switching = self.switching_branch(cx).map(str::to_owned);
        let current = self
            .chat_branches
            .ready()
            .and_then(|listing| listing.current.clone());
        let scrollbar = popover::rail(self, "chat-branch-scrollbar", &theme, cx);
        let body: AnyElement = match &self.chat_branches {
            Loadable::Loading | Loadable::Idle => {
                popover::skeleton_rows("chat-branch-skeleton", &theme, 4, cx.entity_id(), cx)
            }
            Loadable::Error(message) => {
                let message = message.clone();
                self.retry_row(
                    "chat-branch-retry",
                    &message,
                    PickerKind::Branch,
                    &theme,
                    cx,
                )
            }
            Loadable::Ready(listing) if self.naming_branch => {
                let from = chat_branch_label(Some(listing), None);
                let hint = match &switching {
                    Some(name) => format!("Creating {name}…"),
                    None => format!("Starts from {from}. Enter creates it."),
                };
                div()
                    .px(px(Theme::SPACE_SM))
                    .py(px(4.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(hint))
                    .into_any_element()
            }
            Loadable::Ready(_) => {
                let rows = self.filtered_chat_branch_rows(cx);
                let new_ix = rows.len();
                let active = self.active;
                let list = popover::menu_scroll_list("chat-branch-list", &self.menu_scroll)
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .max_h(px(self.list_budget(176.0)))
                    .when(rows.is_empty(), |el| {
                        el.child(
                            div()
                                .p(px(Theme::SPACE_SM))
                                .text_size(crate::typography::ui_rems(12.0))
                                .text_color(theme.text_faint)
                                .child(SharedString::from("No branches match.")),
                        )
                    })
                    .children(rows.into_iter().enumerate().map(|(ix, row)| {
                        let is_current = current.as_deref() == Some(row.name.as_str());
                        let held = row.checked_out_at.clone();
                        let is_switching = switching.as_deref() == Some(row.name.as_str());
                        let blocked = block.is_some() || held.is_some() || switching.is_some();
                        let label = SharedString::from(row.name.clone());
                        popover::menu_row_nav(
                            &theme,
                            is_current,
                            ix == active,
                            format!("chat-branch-row-{ix}"),
                        )
                        .id(("chat-branch-row", ix))
                        .when(blocked && !is_current, |el| {
                            el.opacity(0.5).cursor_default()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_chat_branch(row.clone(), cx);
                        }))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .when(is_switching, |el| {
                            el.child(row_tag(&theme, "switching…".into()))
                        })
                        .when_some(held, |el, path| {
                            el.child(
                                div()
                                    .id(("chat-branch-held", ix))
                                    .flex_none()
                                    .min_w_0()
                                    .max_w(px(140.0))
                                    .child(row_tag(
                                        &theme,
                                        format!("in {}", folder_name(&path)).into(),
                                    ))
                                    .tooltip(crate::settings::widgets::text_tooltip(format!(
                                        "Checked out in {path}"
                                    ))),
                            )
                        })
                        .when(is_current, |el| {
                            el.child(
                                crate::icons::icon(crate::icons::CHECK)
                                    .size(px(14.0))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                        })
                    }));
                let new_branch = popover::menu_row_nav(
                    &theme,
                    false,
                    active == new_ix,
                    "chat-branch-new".to_string(),
                )
                .id("chat-branch-new")
                .when(block.is_some(), |el| el.opacity(0.5).cursor_default())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.start_naming_branch(cx);
                    let handle = this.search.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }))
                .child(
                    crate::icons::icon(crate::icons::PLUS)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from("New branch…")),
                );
                div()
                    .flex()
                    .flex_col()
                    .child(
                        popover::menu_scroll_host("chat-branch-list-host")
                            .on_hover(cx.listener(Self::on_menu_list_hover))
                            .child(popover::faded_menu_list(&self.menu_scroll, list))
                            .children(scrollbar),
                    )
                    .child(popover::menu_section().child(new_branch))
                    .into_any_element()
            }
        };
        // Plain lines under a hairline: why nothing can switch, what git
        // refused, and that a shared folder moves every chat in it.
        let notes: Vec<(SharedString, gpui::Hsla)> = [
            block.map(|text| (SharedString::from(text), theme.warning)),
            self.chat_switch_error
                .clone()
                .map(|text| (SharedString::from(text), theme.danger.opacity(0.9))),
            local_checkout.then(|| {
                (
                    SharedString::from(
                        "Switching changes the branch for every chat in this folder.",
                    ),
                    theme.text_faint,
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        div()
            .flex()
            .flex_col()
            .child(self.search_box(&theme))
            .child(body)
            .when(!notes.is_empty(), |el| {
                el.child(popover::menu_section().children(notes.into_iter().map(
                    |(text, color)| {
                        div()
                            .px(px(Theme::SPACE_SM))
                            .py(px(2.0))
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(color)
                            .child(text)
                    },
                )))
            })
            .into_any_element()
    }
}

/// A right-aligned muted tag on a branch row.
fn row_tag(theme: &Theme, text: SharedString) -> gpui::Div {
    div()
        .flex_none()
        .min_w_0()
        .truncate()
        .text_size(crate::typography::ui_rems(10.0))
        .text_color(theme.text_muted)
        .child(text)
}

fn chat_running(state: &AppState, chat: &Chat) -> bool {
    matches!(
        state.display_status_for(chat, chrono::Utc::now()),
        ChatIndicator::Working | ChatIndicator::AwaitingInput
    )
}

/// `chatId` (+ `targetDeviceId` for a chat hosted elsewhere, which the relay
/// forwards), plus the branch to switch to when switching.
fn chat_branch_params(
    chat: &Chat,
    local_device: Option<&str>,
    switch: Option<(&str, bool)>,
) -> serde_json::Value {
    let mut params = serde_json::json!({ "chatId": chat.id });
    if local_device != Some(chat.device_id.as_str()) {
        params["targetDeviceId"] = serde_json::Value::String(chat.device_id.clone());
    }
    if let Some((branch, create)) = switch {
        params["branch"] = serde_json::Value::String(branch.to_owned());
        params["create"] = serde_json::Value::Bool(create);
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_names_the_live_branch_before_the_recorded_one() {
        let live = |current: Option<&str>, head: Option<&str>| CheckoutBranches {
            current: current.map(str::to_owned),
            head: head.map(str::to_owned),
            branches: Vec::new(),
        };
        // The agent switched to `feature`; the chat was recorded on `main`.
        assert_eq!(
            chat_branch_label(Some(&live(Some("feature"), Some("abc1234"))), Some("main")),
            "feature"
        );
        // Detached HEAD: the short commit.
        assert_eq!(
            chat_branch_label(Some(&live(None, Some("abc1234"))), Some("main")),
            "abc1234"
        );
        // Until git answers (or when the host can't be reached).
        assert_eq!(chat_branch_label(None, Some("main")), "main");
        assert_eq!(chat_branch_label(None, None), "No ref");
    }

    #[gpui::test]
    fn a_switch_in_flight_stays_with_the_chat_that_started_it(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::dark()));
        let state = cx.new(|_| {
            let mut state = AppState::new();
            for id in ["a", "b"] {
                state.chats.push(
                    serde_json::from_value(serde_json::json!({
                        "id": id, "deviceId": "device", "archived": false,
                        "createdAt": "2026-09-01T00:00:00Z"
                    }))
                    .unwrap(),
                );
            }
            state.selected_chat = Some("a".into());
            state
        });
        let select = |id: &str, cx: &mut gpui::TestAppContext| {
            state.update(cx, |state, cx| {
                state.selected_chat = Some(id.into());
                cx.notify();
            });
        };
        let pickers = cx.new(|cx| Pickers::new(state.clone(), cx));
        pickers.update(cx, |pickers, cx| {
            pickers.chat_switch = Some(("a".into(), "feature".into()));
            assert_eq!(pickers.switching_branch(cx), Some("feature"));
        });
        // A's switch is still in flight when B is picked in the sidebar.
        select("b", cx);
        pickers.update(cx, |pickers, cx| {
            assert_eq!(pickers.switching_branch(cx), None);
            pickers.finish_chat_switch("a", "feature", Err("dirty tree".into()), cx);
            assert_eq!(pickers.chat_switch_error, None);
            assert_eq!(pickers.chat_switch, None);
        });
        // A's own refusal shows in A.
        select("a", cx);
        pickers.update(cx, |pickers, cx| {
            pickers.chat_switch = Some(("a".into(), "feature".into()));
            pickers.finish_chat_switch("a", "feature", Err("dirty tree".into()), cx);
            assert_eq!(pickers.chat_switch_error.as_deref(), Some("dirty tree"));
            assert_eq!(pickers.switching_branch(cx), None);
        });
    }
}
