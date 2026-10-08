//! Mac banners for loose ends. The memory server marks each loose-ends row
//! with the heat level a notification is due for right now (`notify`: 3 hot,
//! 4 burning; it applies quiet hours and spaces out burning repeats). Home
//! posts that as a banner, then tells the server with
//! `POST /memory/loose-ends/notified`. All Home adds is memory: one app run
//! never posts the same (row, level) twice, even while the server hasn't
//! caught up, and a burning row repeats only once the server says it's due
//! again after recording the last banner.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::Instant;

use gpui::{App, AppContext as _, Context, Task};
use gpui_tokio::Tokio;
use keron_door::{DoorClient, DoorError};
use keron_home::loose_ends::{self, Action};
use keron_home::{Body, FetchError, ListItem, Payload, SourceSpec};

use super::Home;

/// Shows one banner: (title, body).
pub(crate) type Banner = Rc<dyn Fn(&str, &str)>;
/// Tells the server the banners for these ids at this level went out.
pub(crate) type Record = Rc<dyn Fn(u8, Vec<String>, &App) -> Task<Result<(), FetchError>>>;

/// The memory source whose rows notify.
const LOOSE_ENDS: &str = "loose-ends";
/// A banner's title is cut to this many characters.
const TITLE_CHARS: usize = 60;
const HOT: u8 = 3;
const BURNING: u8 = 4;

/// Where the server's copy of one posted banner stands.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ack {
    Sending,
    /// Sent again on the next refresh.
    Failed,
    /// The server's answer came back at this instant.
    Recorded(Instant),
    /// The server refused it for good (the item closed); sent again only if
    /// the row comes back due.
    Refused,
}

pub(crate) struct Notices {
    banner: Banner,
    record: Record,
    /// Every banner this app run posted, by (row id, level).
    posted: HashMap<(String, u8), Ack>,
    sends: HashMap<u64, Task<()>>,
    next_send: u64,
}

impl Notices {
    pub(crate) fn new(banner: Banner, record: Record) -> Self {
        Self {
            banner,
            record,
            posted: HashMap::new(),
            sends: HashMap::new(),
            next_send: 0,
        }
    }

    /// The owner's: real banners, which bring the app forward on Home when
    /// clicked, recorded through the door.
    pub(crate) fn owner(door: DoorClient) -> Self {
        Self::new(
            Rc::new(|title, body| {
                crate::notify::post(title, body, Some(crate::notify::HOME_TARGET))
            }),
            Rc::new(move |level, ids, cx| {
                let door = door.clone();
                let send = Tokio::spawn(cx, async move {
                    loose_ends::act(&door, &Action::Notified { level }, &ids)
                        .await
                        .map(|_| ())
                });
                cx.background_spawn(async move {
                    send.await.unwrap_or_else(|_| {
                        Err(FetchError::Unsupported("the request stopped".to_string()))
                    })
                })
            }),
        )
    }
}

impl Home {
    /// A payload for `widget` came back from a fetch that began at
    /// `started`. For the loose-ends widget: post each banner the server
    /// says is due and this run hasn't posted, then record them, and send
    /// again what failed to record before.
    pub(super) fn post_due_notices(
        &mut self,
        widget: &str,
        started: Instant,
        cx: &mut Context<Self>,
    ) {
        let loose_ends = self.catalog.manifests.iter().any(|manifest| {
            manifest.id == widget
                && matches!(&manifest.source, SourceSpec::Memory(name) if name == LOOSE_ENDS)
        });
        let (Some(notices), true) = (self.notices.as_mut(), loose_ends) else {
            return;
        };
        let Some(state) = self.widgets.get(widget) else {
            return;
        };
        let Some(Payload {
            body: Body::List(rows),
            ..
        }) = &state.payload
        else {
            return;
        };
        let allowed = rows.iter().any(|row| row.notify.is_some()) && banners_allowed(cx);
        let mut sends: BTreeMap<u8, Vec<String>> = BTreeMap::new();
        for row in rows {
            let Some(id) = row.id.as_deref() else {
                continue;
            };
            // Rows the owner just acted on wait for the server's next word.
            if state.overrides.contains_key(id) {
                continue;
            }
            if let Some(level) = row.notify.filter(|level| (HOT..=BURNING).contains(level)) {
                let key = (id.to_string(), level);
                let banner = match notices.posted.get(&key) {
                    None => true,
                    // Due again after the server recorded the last one.
                    Some(Ack::Recorded(at)) => level == BURNING && started > *at,
                    Some(Ack::Failed | Ack::Refused) => {
                        notices.posted.insert(key, Ack::Sending);
                        sends.entry(level).or_default().push(id.to_string());
                        continue;
                    }
                    Some(Ack::Sending) => false,
                };
                if banner && allowed {
                    let (title, body) = banner_text(row, level);
                    (notices.banner)(&title, &body);
                    notices.posted.insert(key, Ack::Sending);
                    sends.entry(level).or_default().push(id.to_string());
                }
            }
        }
        // What failed to record before, for rows still open.
        for ((id, level), ack) in notices.posted.iter_mut() {
            if *ack == Ack::Failed && rows.iter().any(|row| row.id.as_deref() == Some(id)) {
                *ack = Ack::Sending;
                sends.entry(*level).or_default().push(id.clone());
            }
        }
        for (level, ids) in sends {
            let send = (notices.record)(level, ids.clone(), cx);
            let key = notices.next_send;
            notices.next_send += 1;
            let task = cx.spawn(async move |this, cx| {
                let result = send.await;
                this.update(cx, |this, _| this.finish_notices(key, level, &ids, result))
                    .ok();
            });
            notices.sends.insert(key, task);
        }
    }

    fn finish_notices(
        &mut self,
        send: u64,
        level: u8,
        ids: &[String],
        result: Result<(), FetchError>,
    ) {
        let Some(notices) = self.notices.as_mut() else {
            return;
        };
        notices.sends.remove(&send);
        let ack = match &result {
            Ok(()) => Ack::Recorded(Instant::now()),
            Err(FetchError::Door(DoorError::Http {
                status: 400 | 409, ..
            })) => Ack::Refused,
            Err(_) => Ack::Failed,
        };
        if let Err(error) = &result {
            tracing::debug!(%error, "home: couldn't record a loose-end notification");
        }
        for id in ids {
            if let Some(slot) = notices.posted.get_mut(&(id.clone(), level))
                && *slot == Ack::Sending
            {
                *slot = ack;
            }
        }
    }
}

/// Zeron's rule for every banner: notifications are on and, with "only when
/// Keron is in the background" set, no Keron window is key.
fn banners_allowed(cx: &App) -> bool {
    let settings = crate::settings::current(cx);
    settings.notifications_enabled
        && !(settings.notifications_background_only && cx.active_window().is_some())
}

/// "Hot: <title>" or "Burning: <title>", over the row's second line (or its
/// kind and age).
fn banner_text(row: &ListItem, level: u8) -> (String, String) {
    let word = if level >= BURNING { "Burning" } else { "Hot" };
    let title = format!("{word}: {}", cut(row.title.trim(), TITLE_CHARS));
    let body = row
        .sub
        .clone()
        .filter(|sub| !sub.trim().is_empty())
        .unwrap_or_else(|| {
            let kind = row.kind.as_deref().map(|kind| kind.replace('-', " "));
            let parts: Vec<&str> = [kind.as_deref(), row.age.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            if parts.is_empty() {
                "A loose end".to_string()
            } else {
                parts.join(" · ")
            }
        });
    (title, body)
}

/// At most `max` characters, the last an ellipsis when cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}
