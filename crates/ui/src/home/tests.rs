use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{AppContext as _, Entity, Task, TestAppContext, px, size};
use keron_home::catalog::Catalog;
use keron_home::{Body, HomePaths, Kind, Layout, ListItem, Manifest, Payload, SourceSpec};

use super::*;

fn manifest(id: &str, title: &str) -> Manifest {
    Manifest {
        id: id.to_string(),
        title: title.to_string(),
        kind: Kind::List,
        source: SourceSpec::Zeron("sessions".to_string()),
        every: Duration::from_secs(60),
        icon: None,
        wide: false,
        limit: None,
        empty: None,
        path: None,
    }
}

fn catalog(manifests: Vec<Manifest>) -> Catalog {
    Catalog {
        manifests,
        ..Catalog::default()
    }
}

fn rows(titles: &[&str]) -> Payload {
    Payload {
        updated: None,
        errors: Vec::new(),
        body: Body::List(
            titles
                .iter()
                .map(|title| ListItem {
                    title: title.to_string(),
                    ..ListItem::default()
                })
                .collect(),
        ),
    }
}

/// Home over a temporary `~` with no door, after its first load.
fn home(cx: &mut TestAppContext) -> (Entity<Home>, HomePaths, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let paths = HomePaths::under_home(dir.path());
    let home = cx.new(|cx| Home::with_parts(paths.clone(), None, cx));
    cx.run_until_parked();
    (home, paths, dir)
}

#[gpui::test]
fn customize_changes_the_layout_and_saves_it(cx: &mut TestAppContext) {
    let (home, paths, _dir) = home(cx);
    home.update(cx, |home, cx| {
        home.apply_catalog(
            catalog(vec![
                manifest("alpha", "Alpha"),
                manifest("beta", "Beta"),
                manifest("gamma", "Gamma"),
            ]),
            cx,
        );
        home.set_shown("beta", false, cx);
        home.move_by("gamma", -1, cx);
        home.set_width("alpha", 2, cx);
    });
    let order = home.read_with(cx, |home, _| {
        home.arranged()
            .into_iter()
            .map(|slot| (slot.manifest.id, slot.shown, slot.width))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        order,
        vec![
            ("alpha".to_string(), true, 2),
            ("gamma".to_string(), true, 1),
            ("beta".to_string(), false, 1),
        ]
    );

    // Written once the changes settle, not on every click.
    assert!(!paths.layout_file.exists());
    cx.executor()
        .advance_clock(SAVE_DEBOUNCE + Duration::from_millis(50));
    cx.run_until_parked();
    let layout = home.read_with(cx, |home, _| home.layout.clone());
    assert_eq!(Layout::load(&paths.layout_file).unwrap(), layout);

    // A change still settling is written when Home goes away.
    home.update(cx, |home, cx| home.set_shown("beta", true, cx));
    cx.update(|_| drop(home));
    cx.run_until_parked();
    let saved = Layout::load(&paths.layout_file).unwrap();
    assert!(
        saved
            .widgets
            .iter()
            .any(|entry| entry.id == "beta" && entry.shown)
    );
}

#[gpui::test]
fn a_reload_keeps_what_unchanged_widgets_show(cx: &mut TestAppContext) {
    let (home, _paths, _dir) = home(cx);
    home.update(cx, |home, cx| {
        home.apply_catalog(
            catalog(vec![
                manifest("alpha", "Alpha"),
                manifest("beta", "Beta"),
                manifest("gone", "Gone"),
            ]),
            cx,
        );
        for id in ["alpha", "beta", "gone"] {
            home.widgets.entry(id.to_string()).or_default().payload = Some(rows(&[id]));
        }

        home.apply_catalog(
            catalog(vec![
                manifest("alpha", "Alpha"),
                manifest("beta", "Beta, renamed"),
                manifest("new", "New"),
            ]),
            cx,
        );

        let payload = |id: &str| home.widgets.get(id).and_then(|state| state.payload.clone());
        assert_eq!(payload("alpha"), Some(rows(&["alpha"])));
        assert_eq!(payload("beta"), None, "a changed manifest starts over");
        assert_eq!(payload("new"), None);
        assert!(!home.widgets.contains_key("gone"));
    });
}

#[gpui::test]
fn a_failed_action_says_why_until_a_later_refresh_comes_back(cx: &mut TestAppContext) {
    let (home, _paths, _dir) = home(cx);
    home.update(cx, |home, cx| {
        home.widgets.entry("loose-ends".to_string()).or_default();
        let before = Instant::now();
        home.finish_action(
            "loose-ends",
            "r1",
            Err(FetchError::Unsupported("the door said no".to_string())),
            cx,
        );
        let failed = |home: &Home| home.widgets["loose-ends"].action_error.is_some();

        // A refresh already under way when the action failed leaves it.
        home.apply_fetch("loose-ends", before, Ok(rows(&["a"])), cx);
        assert!(failed(home));
        // One that began afterwards clears it.
        let later = Instant::now() + Duration::from_millis(1);
        home.apply_fetch("loose-ends", later, Ok(rows(&["a"])), cx);
        assert!(!failed(home));
    });
}

#[gpui::test]
fn links_open_in_the_browser_in_the_app_or_not_at_all(cx: &mut TestAppContext) {
    let (home, _paths, _dir) = home(cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&home, move |_, event: &HomeEvent, _| {
            events.borrow_mut().push(event.clone())
        })
        .detach();
    });

    home.update(cx, |home, cx| {
        home.open_link("https://github.com/o/r/pull/7", cx)
    });
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/o/r/pull/7")
    );
    assert!(events.borrow().is_empty());

    let chat = keron_home::kinds::chat_link("chat-1");
    home.update(cx, |home, cx| home.open_link(&chat, cx));
    assert_eq!(
        *events.borrow(),
        vec![HomeEvent::OpenChat("chat-1".to_string())]
    );

    for link in [
        "file:///etc/hosts",
        "javascript:alert(1)",
        "mailto:someone@example.com",
        "",
    ] {
        home.update(cx, |home, cx| home.open_link(link, cx));
    }
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/o/r/pull/7")
    );
    assert_eq!(events.borrow().len(), 1);
}

#[test]
fn list_cards_leave_out_done_rows_then_cap_the_rest() {
    let items: Vec<ListItem> = (0..10)
        .map(|ix| ListItem {
            id: Some(format!("r{ix}")),
            title: format!("Row {ix}"),
            ..ListItem::default()
        })
        .collect();
    let overrides = HashMap::from([
        (
            "r1".to_string(),
            RowOverride {
                change: RowChange::Hidden,
                settled: false,
            },
        ),
        (
            "r2".to_string(),
            RowOverride {
                change: RowChange::Snoozed,
                settled: false,
            },
        ),
    ]);
    let (shown, more) = visible_list_rows(&items, &overrides, DEFAULT_LIMIT);
    let ids: Vec<&str> = shown.iter().filter_map(|row| row.id.as_deref()).collect();
    assert_eq!(ids, ["r0", "r2", "r3", "r4", "r5", "r6"]);
    assert_eq!(more, 3);
}

#[test]
fn a_row_past_the_limit_opens_as_a_done_row_closes_so_the_card_keeps_its_height() {
    let items: Vec<ListItem> = ["a", "b", "c", "d", "e", "f", "g", "h"]
        .iter()
        .map(|id| ListItem {
            id: Some(id.to_string()),
            title: id.to_string(),
            ..ListItem::default()
        })
        .collect();
    let hidden = HashMap::from([(
        "b".to_string(),
        RowOverride {
            change: RowChange::Hidden,
            settled: false,
        },
    )]);
    let leaving = |id: &str| id == "b";
    fn phases<'a>(drawn: &DrawnList<'a>) -> Vec<(String, Drawn<'a>)> {
        drawn
            .rows
            .iter()
            .map(|(row, drawn)| (row.title.clone(), *drawn))
            .collect()
    }
    let whole = |ids: &[&str]| -> Vec<(String, Drawn<'static>)> {
        ids.iter()
            .map(|id| (id.to_string(), Drawn::Whole))
            .collect()
    };

    // Eight rows, six shown: b closes while g opens, and "+N more" stays.
    let drawn = drawn_list_rows(&items, &hidden, leaving, 6);
    let mut expected = whole(&["a"]);
    expected.push(("b".to_string(), Drawn::Leaving));
    expected.extend(whole(&["c", "d", "e", "f"]));
    expected.push(("g".to_string(), Drawn::Joining { with: "b" }));
    assert_eq!(phases(&drawn), expected);
    assert_eq!((drawn.more, drawn.more_closing), (1, None));
    // Once b is gone, the same rows stand whole.
    let drawn = drawn_list_rows(&items, &hidden, |_| false, 6);
    assert_eq!(phases(&drawn), whole(&["a", "c", "d", "e", "f", "g"]));
    assert_eq!((drawn.more, drawn.more_closing), (1, None));

    // Seven rows, six shown: g joins and "+1 more" closes up with b.
    let drawn = drawn_list_rows(&items[..7], &hidden, leaving, 6);
    assert_eq!(drawn.rows.len(), 7);
    assert_eq!((drawn.more, drawn.more_closing), (0, Some(1)));
    let drawn = drawn_list_rows(&items[..7], &hidden, |_| false, 6);
    assert_eq!((drawn.more, drawn.more_closing), (0, None));
}

#[test]
fn a_dropped_card_lands_between_the_right_widgets_when_some_are_hidden() {
    let all = ["a", "hidden-1", "b", "c", "hidden-2", "d"];
    let shown = ["a", "b", "c", "d"];
    // d dropped where b was: just before b.
    assert_eq!(drop_target(&all, &shown, "d", 1), 2);
    // a dropped last: just after d.
    assert_eq!(drop_target(&all, &shown, "a", 3), 5);
    // b dropped first: before a.
    assert_eq!(drop_target(&all, &shown, "b", 0), 0);
}

#[test]
fn wide_cards_pack_densely() {
    // narrow, wide, narrow, narrow in two columns: the third card fills the
    // hole the wide one left beside the first.
    assert_eq!(
        dense_cells(&[1, 2, 1, 1], 2),
        vec![(0, 0), (1, 0), (0, 1), (2, 0)]
    );
    assert_eq!(dense_cells(&[2, 1], 1), vec![(0, 0), (1, 0)]);
}

/// What the fake banner and record hooks saw.
#[derive(Default)]
struct Sent {
    banners: Vec<(String, String)>,
    records: Vec<(u8, String)>,
    /// Answers for the next records, in order; `Ok` once they run out.
    answers: Vec<Result<(), FetchError>>,
}

/// Home with a loose-ends widget and fake banners, as the owner's would be.
fn notifying_home(cx: &mut TestAppContext) -> (Entity<Home>, Rc<RefCell<Sent>>, tempfile::TempDir) {
    let (home, _paths, dir) = home(cx);
    let sent = Rc::new(RefCell::new(Sent::default()));
    let banner: notices::Banner = {
        let sent = sent.clone();
        Rc::new(move |title, body| {
            sent.borrow_mut()
                .banners
                .push((title.to_string(), body.to_string()))
        })
    };
    let record: notices::Record = {
        let sent = sent.clone();
        Rc::new(move |level, id, _| {
            let mut sent = sent.borrow_mut();
            sent.records.push((level, id));
            let answer = if sent.answers.is_empty() {
                Ok(())
            } else {
                sent.answers.remove(0)
            };
            Task::ready(answer)
        })
    };
    home.update(cx, |home, cx| {
        home.notices = Some(notices::Notices::new(banner, record));
        let mut loose_ends = manifest("loose-ends", "Loose ends");
        loose_ends.source = SourceSpec::Memory("loose-ends".to_string());
        home.apply_catalog(catalog(vec![loose_ends]), cx);
    });
    (home, sent, dir)
}

/// Loose ends as served: (id, title, notify).
fn due(rows: &[(&str, &str, Option<u8>)]) -> Payload {
    Payload {
        updated: None,
        errors: Vec::new(),
        body: Body::List(
            rows.iter()
                .map(|(id, title, notify)| ListItem {
                    id: Some(id.to_string()),
                    title: title.to_string(),
                    sub: Some("promise · someone waiting".to_string()),
                    notify: *notify,
                    ..ListItem::default()
                })
                .collect(),
        ),
    }
}

/// A fetch that began at `started` brings `payload`, and what it sends settles.
fn fetched(home: &Entity<Home>, started: Instant, payload: Payload, cx: &mut TestAppContext) {
    home.update(cx, |home, cx| {
        home.apply_fetch("loose-ends", started, Ok(payload), cx);
    });
    cx.run_until_parked();
}

#[gpui::test]
fn due_loose_ends_post_once_per_level_and_show_again_only_once_recorded(cx: &mut TestAppContext) {
    let (home, sent, _dir) = notifying_home(cx);
    let rows = due(&[
        ("hot", "Send Kofi the deck", Some(3)),
        ("calm", "Reply to Sam", None),
        ("fire", "Sign the lease", Some(4)),
    ]);
    let before = Instant::now();
    fetched(&home, before, rows.clone(), cx);
    assert_eq!(
        sent.borrow().banners,
        [
            (
                "Hot: Send Kofi the deck".to_string(),
                "promise · someone waiting".to_string()
            ),
            (
                "Burning: Sign the lease".to_string(),
                "promise · someone waiting".to_string()
            ),
        ]
    );
    assert_eq!(
        sent.borrow().records,
        [(3, "hot".to_string()), (4, "fire".to_string())]
    );

    // A refresh that began before the server recorded them still says due:
    // nothing new.
    fetched(&home, before, rows.clone(), cx);
    assert_eq!(sent.borrow().banners.len(), 2);
    assert_eq!(sent.borrow().records.len(), 2);

    // Due again in a refresh that began after the record: a burning repeat,
    // or a hot item the server reopened under the same id. Both show.
    let later = Instant::now() + Duration::from_millis(1);
    fetched(&home, later, rows, cx);
    assert_eq!(sent.borrow().banners.len(), 4);
    assert_eq!(sent.borrow().banners[2].0, "Hot: Send Kofi the deck");
    assert_eq!(sent.borrow().banners[3].0, "Burning: Sign the lease");
    assert_eq!(
        sent.borrow().records[2..],
        [(3, "hot".to_string()), (4, "fire".to_string())]
    );
}

#[gpui::test]
fn a_refused_banner_shows_again_only_when_a_later_refresh_says_its_due(cx: &mut TestAppContext) {
    let (home, sent, _dir) = notifying_home(cx);
    // "hot" closed between the fetch and the record; "fire" didn't.
    sent.borrow_mut().answers = vec![
        Err(FetchError::Door(keron_door::DoorError::Http {
            status: 409,
            error: None,
            message: "not open".to_string(),
        })),
        Ok(()),
    ];
    let before = Instant::now();
    let rows = due(&[
        ("hot", "Send the deck", Some(3)),
        ("fire", "Sign the lease", Some(4)),
    ]);
    fetched(&home, before, rows.clone(), cx);
    assert_eq!(sent.borrow().banners.len(), 2);

    // A refresh from before the refusal: nothing new, and "fire" stays
    // recorded despite its neighbour's refusal.
    fetched(&home, before, rows, cx);
    assert_eq!(sent.borrow().records.len(), 2);

    // Reopened and due again: a fresh banner, recorded again.
    let later = Instant::now() + Duration::from_millis(1);
    fetched(&home, later, due(&[("hot", "Send the deck", Some(3))]), cx);
    assert_eq!(sent.borrow().banners.len(), 3);
    assert_eq!(sent.borrow().records[2], (3, "hot".to_string()));
}

#[gpui::test]
fn a_banner_whose_record_failed_isnt_posted_again_and_the_record_is_retried(
    cx: &mut TestAppContext,
) {
    let (home, sent, _dir) = notifying_home(cx);
    sent.borrow_mut().answers = vec![Err(FetchError::Unsupported("offline".to_string()))];
    fetched(
        &home,
        Instant::now(),
        due(&[("hot", "Send the deck", Some(3))]),
        cx,
    );
    assert_eq!(sent.borrow().banners.len(), 1);

    // The next refresh sends the record again, even once quiet hours have
    // started and the row no longer says it's due, and posts nothing.
    let before = Instant::now();
    let later = before + Duration::from_millis(1);
    fetched(&home, later, due(&[("hot", "Send the deck", None)]), cx);
    assert_eq!(sent.borrow().banners.len(), 1);
    assert_eq!(
        sent.borrow().records,
        [(3, "hot".to_string()), (3, "hot".to_string())]
    );

    // Recorded now; a refresh from before that still says due: nothing more.
    fetched(&home, before, due(&[("hot", "Send the deck", Some(3))]), cx);
    assert_eq!(sent.borrow().banners.len(), 1);
    assert_eq!(sent.borrow().records.len(), 2);
}

#[gpui::test]
fn notifications_off_post_nothing_until_turned_on(cx: &mut TestAppContext) {
    let settings_dir = tempfile::tempdir().unwrap();
    let mut off = crate::settings::UiSettings::default();
    off.notifications_enabled = false;
    cx.update(|cx| crate::settings::init(off, settings_dir.path(), cx));
    let (home, sent, _dir) = notifying_home(cx);
    let rows = due(&[("hot", "Send the deck", Some(3))]);
    fetched(&home, Instant::now(), rows.clone(), cx);
    assert!(sent.borrow().banners.is_empty());
    assert!(
        sent.borrow().records.is_empty(),
        "nothing went out to record"
    );

    cx.update(|cx| {
        crate::settings::update(crate::settings::SavePolicy::Debounced, cx, |settings| {
            settings.notifications_enabled = true
        })
    });
    fetched(&home, Instant::now(), rows, cx);
    assert_eq!(sent.borrow().banners.len(), 1);
    assert_eq!(sent.borrow().records, [(3, "hot".to_string())]);
}

#[gpui::test]
fn done_on_a_slack_row_posts_to_its_source_and_hides_the_row(cx: &mut TestAppContext) {
    let (home, _paths, _dir) = home(cx);
    let posted = Rc::new(RefCell::new(Vec::new()));
    let slack_row = |id: &str, title: &str| ListItem {
        id: Some(id.to_string()),
        title: title.to_string(),
        actions: vec!["done".to_string()],
        ..ListItem::default()
    };
    home.update(cx, |home, cx| {
        home.poster = Some({
            let posted = posted.clone();
            Rc::new(move |request: loose_ends::Request, _: &App| {
                posted.borrow_mut().push((request.path, request.body));
                Task::ready(Ok(()))
            })
        });
        let mut slack = manifest("slack", "Slack · waiting on you");
        slack.source = SourceSpec::KeronSources("slack-waiting".to_string());
        home.apply_catalog(catalog(vec![slack]), cx);
        home.widgets.entry("slack".to_string()).or_default().payload = Some(Payload {
            updated: None,
            errors: Vec::new(),
            body: Body::List(vec![
                slack_row("C1:1728300000.000100", "Ana in #launch"),
                slack_row("D2:1728300100.000200", "Ben"),
            ]),
        });
        home.act("slack", "C1:1728300000.000100", Action::Done, cx);
    });
    cx.run_until_parked();

    assert_eq!(
        *posted.borrow(),
        [(
            "/sources/slack-waiting/done".to_string(),
            serde_json::json!({"ids": ["C1:1728300000.000100"]})
        )]
    );
    home.read_with(cx, |home, _| {
        let state = &home.widgets["slack"];
        let Some(Payload {
            body: Body::List(items),
            ..
        }) = &state.payload
        else {
            panic!("the payload went away");
        };
        let (shown, _) = visible_list_rows(items, &state.overrides, DEFAULT_LIMIT);
        let ids: Vec<&str> = shown.iter().filter_map(|row| row.id.as_deref()).collect();
        assert_eq!(ids, ["D2:1728300100.000200"]);
        assert!(state.action_error.is_none());
        assert!(state.overrides["C1:1728300000.000100"].settled);
    });
}

#[gpui::test]
fn done_on_a_row_the_source_already_dropped_still_hides_it(cx: &mut TestAppContext) {
    let (home, _paths, _dir) = home(cx);
    let row = |id: &str| ListItem {
        id: Some(id.to_string()),
        title: id.to_string(),
        actions: vec!["done".to_string()],
        ..ListItem::default()
    };
    home.update(cx, |home, cx| {
        home.poster = Some(Rc::new(|_: loose_ends::Request, _: &App| {
            Task::ready(Err(FetchError::Door(DoorError::Http {
                status: 404,
                error: Some("unknown_item".to_string()),
                message: "No row has that id.".to_string(),
            })))
        }));
        let mut slack = manifest("slack", "Slack · waiting on you");
        slack.source = SourceSpec::KeronSources("slack-waiting".to_string());
        let mut ends = manifest("ends", "Loose ends");
        ends.source = loose_ends::loose_ends();
        home.apply_catalog(catalog(vec![slack, ends]), cx);
        for widget in ["slack", "ends"] {
            home.widgets.entry(widget.to_string()).or_default().payload = Some(Payload {
                updated: None,
                errors: Vec::new(),
                body: Body::List(vec![row("gone")]),
            });
            home.act(widget, "gone", Action::Done, cx);
        }
    });
    cx.run_until_parked();

    home.read_with(cx, |home, _| {
        let slack = &home.widgets["slack"];
        assert!(
            slack.overrides["gone"].settled,
            "answered elsewhere is done"
        );
        assert!(slack.action_error.is_none());
        let ends = &home.widgets["ends"];
        assert!(
            !ends.overrides.contains_key("gone"),
            "a loose end comes back"
        );
        assert!(ends.action_error.is_some());
    });
}

#[gpui::test]
fn customize_adds_a_builtin_the_folder_is_missing(cx: &mut TestAppContext) {
    let (home, paths, _dir) = home(cx);
    // A widgets folder seeded before the app had Usage.
    std::fs::remove_file(paths.widgets_dir.join("usage.toml")).unwrap();
    std::fs::remove_file(paths.widgets_dir.join(".builtins")).unwrap();
    home.update(cx, |home, cx| home.reload(cx));
    cx.run_until_parked();
    home.read_with(cx, |home, _| {
        assert_eq!(home.missing_builtins(), [("usage", "Usage".to_string())]);
        assert!(!home.shows_usage());
    });

    home.update(cx, |home, cx| home.add_builtin("usage", cx));
    cx.run_until_parked();
    assert!(paths.widgets_dir.join("usage.toml").is_file());
    home.read_with(cx, |home, _| {
        assert!(home.missing_builtins().is_empty());
        // Not in home.toml yet, so it shows at the end.
        assert!(home.shows_usage());
        assert_eq!(
            home.arranged().last().map(|slot| slot.manifest.id.as_str()),
            Some("usage")
        );
    });
}

#[gpui::test]
fn a_done_row_closes_up_before_it_goes_and_at_once_under_reduce_motion(cx: &mut TestAppContext) {
    let (home, _paths, _dir) = home(cx);
    let row = |id: &str| ListItem {
        id: Some(id.to_string()),
        title: id.to_string(),
        actions: vec!["done".to_string()],
        ..ListItem::default()
    };
    let drawn = |home: &Home| -> Vec<(String, bool)> {
        let state = &home.widgets["ends"];
        let Some(Payload {
            body: Body::List(items),
            ..
        }) = &state.payload
        else {
            panic!("the payload went away");
        };
        let leaving = |id: &str| state.leaving.contains_key(id);
        drawn_list_rows(items, &state.overrides, leaving, DEFAULT_LIMIT)
            .rows
            .into_iter()
            .map(|(row, drawn)| (row.title.clone(), drawn == Drawn::Leaving))
            .collect()
    };
    let rows = |expected: &[(&str, bool)]| -> Vec<(String, bool)> {
        expected
            .iter()
            .map(|(id, leaving)| (id.to_string(), *leaving))
            .collect()
    };
    home.update(cx, |home, cx| {
        home.poster = Some(Rc::new(|_: loose_ends::Request, _: &App| {
            Task::ready(Ok(()))
        }));
        let mut ends = manifest("ends", "Loose ends");
        ends.source = loose_ends::loose_ends();
        home.apply_catalog(catalog(vec![ends]), cx);
        let state = home.widgets.entry("ends".to_string()).or_default();
        state.payload = Some(Payload {
            updated: None,
            errors: Vec::new(),
            body: Body::List(vec![row("a"), row("b"), row("c")]),
        });
        // Each row as last drawn.
        for id in ["a", "b", "c"] {
            state.row_ui.borrow_mut().insert(
                id.to_string(),
                RowUi {
                    focus: cx.focus_handle(),
                    bounds: Some(Bounds::new(
                        point(px(0.0), px(0.0)),
                        size(px(240.0), px(23.0)),
                    )),
                },
            );
        }
        home.act("ends", "a", Action::Done, cx);
        assert_eq!(
            drawn(home),
            rows(&[("a", true), ("b", false), ("c", false)])
        );
    });
    cx.executor().advance_clock(leave_duration());
    cx.run_until_parked();
    home.update(cx, |home, cx| {
        assert_eq!(drawn(home), rows(&[("b", false), ("c", false)]));
        cx.set_reduce_motion(true);
        home.act("ends", "b", Action::Done, cx);
        assert_eq!(drawn(home), rows(&[("c", false)]));
    });
}

fn ids(slots: Vec<Slot>) -> Vec<String> {
    slots.into_iter().map(|slot| slot.manifest.id).collect()
}

/// Home with one widget per case: `empty` and `blank` (a stat with nothing)
/// are loaded and have nothing; `full` has a row, `waiting` hasn't loaded,
/// `broken` failed, `number` is a stat with a value and `mail` is a door
/// widget, empty but signed out.
fn quiet_home(cx: &mut TestAppContext) -> (Entity<Home>, tempfile::TempDir) {
    let (home, _paths, dir) = home(cx);
    home.update(cx, |home, cx| {
        let stat = |id: &str| Manifest {
            kind: Kind::Stat,
            ..manifest(id, id)
        };
        let mut mail = manifest("mail", "Gmail");
        mail.source = SourceSpec::KeronSources("gmail-waiting".to_string());
        home.apply_catalog(
            catalog(vec![
                manifest("empty", "Loose ends"),
                manifest("full", "Full"),
                manifest("waiting", "Waiting"),
                manifest("broken", "Broken"),
                stat("number"),
                stat("blank"),
                mail,
            ]),
            cx,
        );
        let stat_payload = |value: &str| Payload {
            updated: None,
            errors: Vec::new(),
            body: Body::Stat(keron_home::Stat {
                value: value.to_string(),
                ..keron_home::Stat::default()
            }),
        };
        let mut set = |id: &str, payload: Payload, error: Option<&str>| {
            let state = home.widgets.entry(id.to_string()).or_default();
            state.payload = Some(payload);
            state.error = error.map(str::to_string);
        };
        set("empty", rows(&[]), None);
        set("full", rows(&["a"]), None);
        set("broken", rows(&[]), Some("the door said no"));
        set("number", stat_payload("42"), None);
        set("blank", stat_payload(""), None);
        set("mail", rows(&[]), None);
        home.sync_quiet(cx);
    });
    (home, dir)
}

#[gpui::test]
fn only_loaded_empty_widgets_leave_the_grid_for_chips(cx: &mut TestAppContext) {
    let (home, _dir) = quiet_home(cx);
    home.update(cx, |home, cx| {
        assert_eq!(ids(home.chip_slots()), ["empty", "blank"]);
        assert_eq!(
            ids(home.grid_slots()),
            ["full", "waiting", "broken", "number", "mail"]
        );

        // Signed in, the empty door widget is quiet too.
        home.sign_in = SignIn::SignedIn;
        assert_eq!(ids(home.chip_slots()), ["empty", "blank", "mail"]);

        // A row action in flight keeps the card up.
        home.widgets.get_mut("empty").unwrap().overrides.insert(
            "r1".to_string(),
            RowOverride {
                change: RowChange::Hidden,
                settled: false,
            },
        );
        home.sync_quiet(cx);
        assert_eq!(ids(home.chip_slots()), ["blank", "mail"]);
    });
}

#[gpui::test]
fn a_peek_shows_a_quiet_card_until_its_data_arrives(cx: &mut TestAppContext) {
    let (home, _dir) = quiet_home(cx);
    home.update(cx, |home, cx| {
        home.toggle_peek("empty", cx);
        assert!(ids(home.grid_slots()).contains(&"empty".to_string()));
        // Its chip stays, pressed.
        assert!(ids(home.chip_slots()).contains(&"empty".to_string()));
        home.toggle_peek("empty", cx);
        assert!(!ids(home.grid_slots()).contains(&"empty".to_string()));

        // Peeked, then a row arrives: it's a card on its own, peek gone.
        home.toggle_peek("empty", cx);
        home.apply_fetch("empty", Instant::now(), Ok(rows(&["a"])), cx);
        assert!(home.peeks.is_empty());
        assert!(ids(home.grid_slots()).contains(&"empty".to_string()));
        assert!(!ids(home.chip_slots()).contains(&"empty".to_string()));

        // Empty again: back to a chip, not peeked.
        home.apply_fetch("empty", Instant::now(), Ok(rows(&[])), cx);
        assert!(!ids(home.grid_slots()).contains(&"empty".to_string()));
        assert!(ids(home.chip_slots()).contains(&"empty".to_string()));
    });
}

#[gpui::test]
fn customize_and_the_switch_off_show_every_card(cx: &mut TestAppContext) {
    let (home, _dir) = quiet_home(cx);
    let every = [
        "empty", "full", "waiting", "broken", "number", "blank", "mail",
    ];
    home.update(cx, |home, cx| {
        home.toggle_customize(cx);
        assert_eq!(ids(home.grid_slots()), every);
        assert!(home.chip_slots().is_empty());
        home.toggle_customize(cx);
        assert_eq!(ids(home.chip_slots()), ["empty", "blank"]);

        home.set_collapse_empty(false, cx);
        assert_eq!(ids(home.grid_slots()), every);
        assert!(home.chip_slots().is_empty());
        assert!(!home.layout.collapse_empty);
    });
}
