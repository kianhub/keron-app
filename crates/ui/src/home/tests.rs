use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AppContext as _, Entity, InteractiveElement as _, IntoElement, KeyUpEvent, Keystroke,
    Modifiers, MouseButton, ParentElement as _, Render, ScrollDelta, Styled as _, Task,
    TestAppContext, TouchPhase, VisualTestContext, div, point, px, size,
};
use keron_home::catalog::Catalog;
use keron_home::{
    Body, HomePaths, Kind, Layout, ListItem, Manifest, Payload, SourceSpec, TimelineItem,
};

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

fn place(x: f32, y: f32, width: f32, stretch: impl Into<Option<f32>>) -> Place {
    Place {
        x,
        y,
        width,
        stretch: stretch.into(),
    }
}

#[test]
fn cards_fill_each_row_across_and_share_its_height() {
    // Loose ends (wide), Today, Memory (wide) and Usage in four columns:
    // two rows split two thirds and one third, each as tall as its tallest.
    let (places, height) = pack_rows(
        &[2, 1, 2, 1],
        &[300.0, 180.0, 120.0, 260.0],
        4,
        1210.0,
        10.0,
    );
    assert_eq!(
        places,
        vec![
            place(0.0, 0.0, 800.0, 300.0),
            place(810.0, 0.0, 400.0, 300.0),
            place(0.0, 310.0, 800.0, 260.0),
            place(810.0, 310.0, 400.0, 260.0),
        ]
    );
    assert_eq!(height, 570.0);
}

#[test]
fn a_card_that_does_not_fit_starts_the_next_row_and_fills_it() {
    // Three narrow cards fill the first row in thirds; the wide one can't
    // join them and has the next row to itself, at its own height.
    let (places, height) = pack_rows(&[1, 1, 1, 2], &[50.0, 90.0, 70.0, 40.0], 4, 1220.0, 10.0);
    assert_eq!(
        places,
        vec![
            place(0.0, 0.0, 400.0, 90.0),
            place(410.0, 0.0, 400.0, 90.0),
            place(820.0, 0.0, 400.0, 90.0),
            place(0.0, 100.0, 1220.0, None),
        ]
    );
    assert_eq!(height, 140.0);
    // One column: a plain stack, wide cards included.
    let (places, height) = pack_rows(&[2, 1], &[50.0, 40.0], 1, 300.0, 10.0);
    assert_eq!(
        places,
        vec![place(0.0, 0.0, 300.0, None), place(0.0, 60.0, 300.0, None)]
    );
    assert_eq!(height, 100.0);
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
        home.act(
            "slack",
            "C1:1728300000.000100",
            Action::Done { comment: None },
            cx,
        );
    });
    cx.executor().advance_clock(UNDO_WINDOW);
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
            home.act(widget, "gone", Action::Done { comment: None }, cx);
        }
    });
    cx.executor().advance_clock(UNDO_WINDOW);
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
    // A widgets folder seeded before the app had Usage and Mini.
    std::fs::remove_file(paths.widgets_dir.join("usage.toml")).unwrap();
    std::fs::remove_file(paths.widgets_dir.join("dashboard.toml")).unwrap();
    std::fs::remove_file(paths.widgets_dir.join(".builtins")).unwrap();
    home.update(cx, |home, cx| home.reload(cx));
    cx.run_until_parked();
    home.read_with(cx, |home, _| {
        assert_eq!(
            home.missing_builtins(),
            [
                ("usage", "Usage".to_string()),
                ("dashboard", "Mini".to_string())
            ]
        );
        assert!(!home.shows_usage());
    });

    home.update(cx, |home, cx| home.add_builtin("usage", cx));
    cx.run_until_parked();
    assert!(paths.widgets_dir.join("usage.toml").is_file());
    home.read_with(cx, |home, _| {
        assert_eq!(home.missing_builtins(), [("dashboard", "Mini".to_string())]);
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
                    bounds: Some(Bounds::new(
                        point(px(0.0), px(0.0)),
                        size(px(240.0), px(23.0)),
                    )),
                    ..RowUi::new(cx)
                },
            );
        }
        home.act("ends", "a", Action::Done { comment: None }, cx);
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
        home.act("ends", "b", Action::Done { comment: None }, cx);
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

#[gpui::test]
fn only_cards_lift_the_composer(cx: &mut TestAppContext) {
    let (home, _dir) = quiet_home(cx);
    home.update(cx, |home, cx| {
        assert!(home.has_shown_widget());
        home.sign_in = SignIn::SignedIn;
        for id in ["full", "waiting", "broken", "number"] {
            let state = home.widgets.entry(id.to_string()).or_default();
            state.payload = Some(rows(&[]));
            state.error = None;
        }
        home.sync_quiet(cx);
        assert!(home.grid_slots().is_empty());
        // Chips alone sit in the toolbar: the composer stays put.
        assert!(!home.has_shown_widget());

        home.toggle_peek("empty", cx);
        assert!(home.has_shown_widget());
        home.toggle_peek("empty", cx);
        home.toggle_customize(cx);
        assert!(home.has_shown_widget());
    });
}

/// A loose end as the door sends it: a sub line, a link, and Snooze and
/// Done.
fn loose_end(ix: usize) -> ListItem {
    ListItem {
        id: Some(format!("e{ix}")),
        title: format!("Loose end {ix}"),
        sub: Some("promise · someone waiting".to_string()),
        link: Some(format!("https://example.com/{ix}")),
        actions: vec!["snooze".to_string(), "done".to_string()],
        ..ListItem::default()
    }
}

/// What went to the door, as `(path, body)`.
type Posted = Rc<RefCell<Vec<(String, serde_json::Value)>>>;

/// A poster that notes each request and never answers, so the card doesn't
/// fetch again.
fn unanswered(posted: &Posted) -> Poster {
    let posted = posted.clone();
    Rc::new(move |request: loose_ends::Request, cx: &App| {
        posted.borrow_mut().push((request.path, request.body));
        cx.background_spawn(std::future::pending())
    })
}

/// A key down and up: a button clicks on the up.
fn press(keys: &str, cx: &mut VisualTestContext) {
    for key in keys.split(' ') {
        cx.simulate_keystrokes(key);
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).unwrap(),
        });
        cx.run_until_parked();
    }
}

/// Give the Loose ends card `count` rows, and draw the frame they arrive in.
fn draw_loose_ends(home: &Entity<Home>, count: usize, cx: &mut VisualTestContext) {
    home.update(cx, |home, cx| {
        home.widgets.get_mut("ends").unwrap().payload = Some(Payload {
            updated: None,
            errors: Vec::new(),
            body: Body::List((0..count).map(loose_end).collect()),
        });
        cx.notify();
    });
    cx.run_until_parked();
}

/// Give the Loose ends card `count` rows, and let the grid settle.
fn set_loose_ends(home: &Entity<Home>, count: usize, cx: &mut VisualTestContext) {
    draw_loose_ends(home, count, cx);
    settle(cx);
}

/// Deliver the frames Home asks for until it stops, and say whether it did
/// within ten: the grid lays out from the last frame's card heights, and a
/// test window has no frame loop. A card fading in asks for frames until it
/// is in, which a test doesn't wait for.
fn settle(cx: &mut VisualTestContext) -> bool {
    for _ in 0..10 {
        if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
            return true;
        }
        cx.run_until_parked();
    }
    false
}

/// Home drawn in a short window, signed in, with a Loose ends card of
/// `count` rows whose actions go nowhere. The page itself can scroll a
/// little.
fn loose_ends_home(
    count: usize,
    cx: &mut TestAppContext,
) -> (Entity<Home>, &mut VisualTestContext, tempfile::TempDir) {
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(crate::theme::Theme::default());
    });
    let dir = tempfile::tempdir().unwrap();
    let paths = HomePaths::under_home(dir.path());
    let (home, cx) = cx.add_window_view(|_, cx| Home::with_parts(paths, None, cx));
    cx.simulate_resize(size(px(800.0), px(360.0)));
    cx.run_until_parked();
    home.update(cx, |home, cx| {
        home.poster = Some(Rc::new(|_: loose_ends::Request, _: &App| {
            Task::ready(Ok(()))
        }));
        let mut ends = manifest("ends", "Loose ends");
        ends.source = loose_ends::loose_ends();
        home.apply_catalog(catalog(vec![ends]), cx);
        home.widgets.entry("ends".to_string()).or_default();
        home.sign_in = SignIn::SignedIn;
        home.set_frame(0.0, 600.0, 600.0, 1.0, cx);
    });
    set_loose_ends(&home, count, cx);
    (home, cx, dir)
}

fn list_scroll(home: &Entity<Home>, cx: &mut VisualTestContext) -> ScrollHandle {
    home.read_with(cx, |home, _| {
        home.list_scrolls.borrow()["ends"].page.scroll.clone()
    })
}

fn wheel(at: Point<Pixels>, dy: f32, touch_phase: TouchPhase, cx: &mut VisualTestContext) {
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.0), px(dy))),
        touch_phase,
        ..Default::default()
    });
    cx.run_until_parked();
}

#[gpui::test]
fn loose_ends_draw_every_row_in_a_card_that_stops_growing(cx: &mut TestAppContext) {
    let (home, cx, _dir) = loose_ends_home(13, cx);
    let card_height = |home: &Entity<Home>, cx: &mut VisualTestContext| {
        home.read_with(cx, |home, _| {
            f32::from(home.card_bounds.borrow()["ends"].size.height)
        })
    };
    // Every row is in the list, none left to a "+N more" line.
    assert_eq!(list_scroll(&home, cx).children_count(), 13);
    let tall = card_height(&home, cx);
    set_loose_ends(&home, 30, cx);
    assert_eq!(list_scroll(&home, cx).children_count(), 30);
    assert!((card_height(&home, cx) - tall).abs() < 0.5);
    // A short list keeps its own height, from the frame it arrives in: a
    // card alone in its row doesn't wait a frame to shrink.
    draw_loose_ends(&home, 2, cx);
    assert!(card_height(&home, cx) < tall - 100.0);

    // Other lists still stop at their limit.
    let rows: Vec<ListItem> = (0..13).map(loose_end).collect();
    let mut slack = manifest("slack", "Slack · waiting on you");
    slack.source = SourceSpec::KeronSources("slack-waiting".to_string());
    let drawn = drawn_list_rows(&rows, &HashMap::new(), |_| false, drawn_limit(&slack));
    assert_eq!(
        (drawn.rows.len(), drawn.more),
        (DEFAULT_LIMIT, 13 - DEFAULT_LIMIT)
    );
}

#[gpui::test]
fn the_memory_timeline_scrolls_through_every_note_too(cx: &mut TestAppContext) {
    let (home, cx, _dir) = loose_ends_home(0, cx);
    let set_notes = |home: &Entity<Home>, count: usize, cx: &mut VisualTestContext| {
        home.update(cx, |home, cx| {
            home.widgets.get_mut("mem").unwrap().payload = Some(Payload {
                updated: None,
                errors: Vec::new(),
                body: Body::Timeline(
                    (0..count)
                        .map(|ix| TimelineItem {
                            time: Some(format!("10:{ix:02}")),
                            kind: Some("did".to_string()),
                            text: format!("Note {ix}"),
                            ..TimelineItem::default()
                        })
                        .collect(),
                ),
            });
            cx.notify();
        });
        cx.run_until_parked();
    };
    home.update(cx, |home, cx| {
        let mut mem = manifest("mem", "Memory · today");
        mem.kind = Kind::Timeline;
        mem.source = SourceSpec::Memory("today".to_string());
        // Added signed out, as the loose ends card is, so nothing fetches.
        let signed_in = std::mem::replace(&mut home.sign_in, SignIn::NoDoor);
        home.apply_catalog(catalog(vec![mem]), cx);
        home.widgets.entry("mem".to_string()).or_default();
        home.sign_in = signed_in;
    });
    set_notes(&home, 20, cx);
    let (rows, height) = home.read_with(cx, |home, _| {
        let rows = home.list_scrolls.borrow()["mem"]
            .page
            .scroll
            .children_count();
        (
            rows,
            f32::from(home.card_bounds.borrow()["mem"].size.height),
        )
    });
    assert_eq!(rows, 20); // every note, none left to "+N more"
    set_notes(&home, 40, cx);
    let taller = home.read_with(cx, |home, _| {
        f32::from(home.card_bounds.borrow()["mem"].size.height)
    });
    assert!((taller - height).abs() < 0.5, "the card stops growing");
}

#[gpui::test]
fn a_row_shrinks_with_its_tallest_card_after_stretching_the_others(cx: &mut TestAppContext) {
    let (home, cx, _dir) = loose_ends_home(13, cx);
    home.update(cx, |home, cx| {
        let mut ends = manifest("ends", "Loose ends");
        ends.source = loose_ends::loose_ends();
        // Added signed out, as the loose ends card was, so nothing fetches.
        let signed_in = std::mem::replace(&mut home.sign_in, SignIn::NoDoor);
        home.apply_catalog(catalog(vec![ends, manifest("other", "Other")]), cx);
        home.sign_in = signed_in;
        home.widgets.entry("other".to_string()).or_default().payload = Some(rows(&["a"]));
        // Wide enough for two columns: both cards in one row.
        home.set_frame(0.0, 700.0, 600.0, 1.0, cx);
    });
    cx.run_until_parked();
    // A stretched card's height doesn't feed back into its row's.
    assert!(settle(cx), "the grid settles");
    let drawn = |home: &Entity<Home>, cx: &mut VisualTestContext| {
        home.read_with(cx, |home, _| {
            let bounds = home.card_bounds.borrow();
            let height = |id: &str| f32::from(bounds[id].size.height);
            (height("ends"), height("other"))
        })
    };
    let (tall, other) = drawn(&home, cx);
    assert!(
        (other - tall).abs() < 0.5,
        "the short card stretches to its row"
    );
    // A card dragged over the stretched part of the short one lands there.
    home.update(cx, |home, cx| {
        let corner = home.card_bounds.borrow()["other"].bottom_left();
        home.drag_over("ends", 0, corner + point(px(20.0), px(-10.0)), cx);
        assert_eq!(home.drag.as_ref().map(|drag| drag.over), Some(1));
        home.cancel_drag(cx);
    });
    set_loose_ends(&home, 2, cx);
    assert!(settle(cx), "and settles again");
    let (short, other) = drawn(&home, cx);
    assert!(short < tall - 100.0);
    assert!((other - short).abs() < 0.5, "and shrinks with it");
}

#[gpui::test]
fn the_wheel_scrolls_loose_ends_to_their_end_and_the_page_from_the_next_gesture(
    cx: &mut TestAppContext,
) {
    let (home, cx, _dir) = loose_ends_home(13, cx);
    let list = list_scroll(&home, cx);
    let page = home.read_with(cx, |home, _| home.scroll.clone());
    let over_list = list.bounds().origin + point(px(40.0), px(40.0));
    let y = |scroll: &ScrollHandle| f32::from(scroll.offset().y);

    wheel(over_list, -60.0, TouchPhase::Started, cx);
    assert!(y(&list) < 0.0);
    assert_eq!(y(&page), 0.0);
    // The flick runs the list to its end, and what's left of it stays there.
    wheel(over_list, -10_000.0, TouchPhase::Moved, cx);
    wheel(over_list, -80.0, TouchPhase::Moved, cx);
    let end = -f32::from(list.max_offset().y);
    assert!(end < 0.0);
    assert_eq!((y(&list), y(&page)), (end, 0.0));

    // A new gesture there scrolls the page, and keeps scrolling it even back
    // up, though the list could move that way now.
    wheel(over_list, -40.0, TouchPhase::Started, cx);
    let scrolled = y(&page);
    assert!(scrolled < 0.0);
    wheel(over_list, 20.0, TouchPhase::Moved, cx);
    assert!(y(&page) > scrolled);
    assert_eq!(y(&list), end);
}

#[gpui::test]
fn tabbing_to_a_loose_end_out_of_view_scrolls_it_in(cx: &mut TestAppContext) {
    let (home, cx, _dir) = loose_ends_home(13, cx);
    let list = list_scroll(&home, cx);
    let focus = |home: &Entity<Home>, cx: &mut VisualTestContext| {
        home.read_with(cx, |home, _| {
            (home.keyboard_row.clone(), home.keyboard_in.clone())
        })
    };
    let row = |id: &str| Some(("ends".to_string(), id.to_string()));
    assert_eq!(list.offset().y, px(0.0));
    cx.simulate_keystrokes("a");
    // Shift-Tab from nowhere: the last row's Done, below the card's fold.
    cx.update(|window, cx| window.focus_prev(cx));
    cx.run_until_parked();
    assert_eq!(focus(&home, cx), (row("e12"), row("e12")));
    let last = list.bounds_for_item(12).unwrap();
    let offset = list.offset().y;
    assert!(offset < px(0.0));
    assert!(last.bottom() + offset <= list.bounds().bottom() + px(0.5));
    // Tab round past Customize to the first row, by its link: back to the top.
    for _ in 0..2 {
        cx.update(|window, cx| window.focus_next(cx));
        cx.run_until_parked();
    }
    assert_eq!(focus(&home, cx), (None, row("e0")));
    assert_eq!(list.offset().y, px(0.0));
    // Scrolled away from it, each Tab on within the row brings it back:
    // to its Snooze, then its Done.
    let over_list = list.bounds().origin + point(px(40.0), px(40.0));
    for _ in 0..2 {
        wheel(over_list, -60.0, TouchPhase::Started, cx);
        wheel(over_list, -10_000.0, TouchPhase::Moved, cx);
        assert!(list.offset().y < px(0.0));
        cx.update(|window, cx| window.focus_next(cx));
        cx.run_until_parked();
        assert_eq!(focus(&home, cx), (row("e0"), row("e0")));
        assert_eq!(list.offset().y, px(0.0));
    }
}

#[test]
fn a_wheel_that_rests_lets_the_next_turn_scroll_past_a_list_at_its_end() {
    let list = || WheelTarget::List("ends".to_string());
    let mut wheel = Wheel::default();
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    assert!(wheel.takes(list(), true, false, at(0)));
    // A mouse wheel has no gestures: turns close together stay with the list
    // at its end.
    assert!(wheel.takes(list(), false, false, at(150)));
    assert!(wheel.takes(list(), false, false, at(300)));
    assert!(!wheel.takes(WheelTarget::Page, true, false, at(310)));
    // After a rest, the page takes them; and holds on to them.
    assert!(!wheel.takes(list(), false, false, at(600)));
    assert!(wheel.takes(WheelTarget::Page, true, false, at(600)));
    assert!(!wheel.takes(list(), true, false, at(650)));
}

#[gpui::test]
fn a_note_typed_under_a_loose_end_goes_with_it_and_escape_drops_it(cx: &mut TestAppContext) {
    let (home, cx, _dir) = loose_ends_home(3, cx);
    cx.update(|_, cx| crate::composer::init(cx, Default::default()));
    let posted = Rc::new(RefCell::new(Vec::new()));
    home.update(cx, |home, _| home.poster = Some(unanswered(&posted)));
    let open = |row: &str, kind: NoteKind, cx: &mut VisualTestContext| {
        home.update(cx, |home, cx| home.open_note("ends", row, kind, cx));
        cx.run_until_parked();
    };
    let field_open = |cx: &mut VisualTestContext| home.read_with(cx, |home, _| home.note.is_some());

    // Escape drops what was typed, and nothing goes to the door.
    open("e0", NoteKind::Done, cx);
    assert!(cx.debug_bounds("home-note-ends-e0").is_some());
    cx.simulate_input("never mind");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!field_open(cx));
    assert!(posted.borrow().is_empty());

    // Enter sends it with Done, and the field goes as the row closes.
    open("e0", NoteKind::Done, cx);
    cx.simulate_input("called her, all lines work now");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!field_open(cx));
    assert!(cx.debug_bounds("home-note-ends-e0").is_none());
    home.read_with(cx, |home, _| {
        assert_eq!(
            home.widgets["ends"].overrides["e0"].change,
            RowChange::Hidden
        );
    });

    // "Not a thing" with the field left empty sends no comment.
    open("e1", NoteKind::Dismiss, cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!field_open(cx));

    // Typing stops at what the door takes, so Enter always sends.
    open("e2", NoteKind::Done, cx);
    cx.simulate_input(&"x".repeat(loose_ends::COMMENT_LIMIT + 5));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!field_open(cx));
    cx.executor().advance_clock(UNDO_WINDOW);
    cx.run_until_parked();
    assert_eq!(
        *posted.borrow(),
        [
            (
                "/memory/loose-ends/done".to_string(),
                serde_json::json!({"ids": ["e0"], "comment": "called her, all lines work now"})
            ),
            (
                "/memory/loose-ends/dismiss".to_string(),
                serde_json::json!({"ids": ["e1"]})
            ),
            (
                "/memory/loose-ends/done".to_string(),
                serde_json::json!({"ids": ["e2"], "comment": "x".repeat(loose_ends::COMMENT_LIMIT)})
            ),
        ]
    );
}

#[gpui::test]
fn a_loose_end_closes_with_a_note_from_the_keyboard(cx: &mut TestAppContext) {
    let (home, cx, _dir) = loose_ends_home(3, cx);
    cx.update(|_, cx| crate::composer::init(cx, Default::default()));
    let posted = Rc::new(RefCell::new(Vec::new()));
    home.update(cx, |home, _| home.poster = Some(unanswered(&posted)));
    let (snooze, menu) = home.read_with(cx, |home, _| {
        let rows = home.widgets["ends"].row_ui.borrow();
        (rows["e0"].snooze.clone(), home.menu_focus.clone())
    });
    let focused = |handle: &FocusHandle, cx: &mut VisualTestContext| {
        cx.update(|window, _| handle.is_focused(window))
    };
    let menu_open =
        |cx: &mut VisualTestContext| home.read_with(cx, |home, _| home.snooze_menu.is_open());

    // Tab past Customize and the first row's link to its Snooze.
    cx.simulate_keystrokes("a");
    for _ in 0..3 {
        cx.update(|window, cx| window.focus_next(cx));
        cx.run_until_parked();
    }
    assert!(focused(&snooze, cx));
    // Enter opens the menu with focus in it; Escape closes it back to Snooze.
    press("enter", cx);
    assert!(menu_open(cx));
    assert!(focused(&menu, cx));
    press("escape", cx);
    assert!(!menu_open(cx));
    assert!(focused(&snooze, cx));

    // Its last item is "Done with a note…": the field opens focused, and
    // Enter sends what was typed with Done.
    press("enter end enter", cx);
    assert!(!menu_open(cx));
    assert!(cx.debug_bounds("home-note-ends-e0").is_some());
    cx.simulate_input("called her back");
    press("enter", cx);
    assert!(home.read_with(cx, |home, _| home.note.is_none()));
    cx.executor().advance_clock(UNDO_WINDOW);
    cx.run_until_parked();
    assert_eq!(
        *posted.borrow(),
        [(
            "/memory/loose-ends/done".to_string(),
            serde_json::json!({"ids": ["e0"], "comment": "called her back"})
        )]
    );
}

#[gpui::test]
fn a_note_the_door_turned_down_is_back_in_its_field(cx: &mut TestAppContext) {
    let (home, cx, _dir) = loose_ends_home(3, cx);
    cx.update(|_, cx| crate::composer::init(cx, Default::default()));
    home.update(cx, |home, _| {
        home.poster = Some(Rc::new(|_: loose_ends::Request, _: &App| {
            Task::ready(Err(FetchError::Unsupported("door down".to_string())))
        }));
    });
    let open = |cx: &mut VisualTestContext| {
        home.update(cx, |home, cx| {
            home.open_note("ends", "e1", NoteKind::Done, cx)
        });
        cx.run_until_parked();
    };
    open(cx);
    cx.simulate_input("called her back");
    cx.simulate_keystrokes("enter");
    cx.executor().advance_clock(UNDO_WINDOW);
    cx.run_until_parked();
    home.read_with(cx, |home, _| {
        assert!(home.note.is_none());
        assert!(!home.widgets["ends"].overrides.contains_key("e1"));
        assert!(home.widgets["ends"].action_error.is_some());
    });
    // Opened again on that row, the field has the note to send again.
    open(cx);
    let typed = home.read_with(cx, |home, cx| {
        home.note
            .as_ref()
            .unwrap()
            .input
            .read(cx)
            .text()
            .to_string()
    });
    assert_eq!(typed, "called her back");
}

/// The ids of a list card's rows as drawn, rows done or "not a thing" left
/// out.
fn shown(home: &Home, widget: &str) -> Vec<String> {
    let state = &home.widgets[widget];
    let Some(Payload {
        body: Body::List(items),
        ..
    }) = &state.payload
    else {
        panic!("{widget} has no rows");
    };
    let (rows, _) = visible_list_rows(items, &state.overrides, usize::MAX);
    rows.iter().filter_map(|row| row.id.clone()).collect()
}

/// Home on screen with a Loose ends card (e0 to e2) and a Slack card (s1,
/// s2), every action noted and answered yes. It has no door, so nothing
/// fetches.
fn held_home(cx: &mut TestAppContext) -> (Entity<Home>, Posted, tempfile::TempDir) {
    let (home, _paths, dir) = home(cx);
    let posted = Posted::default();
    home.update(cx, |home, cx| {
        home.poster = Some({
            let posted = posted.clone();
            Rc::new(move |request: loose_ends::Request, _: &App| {
                posted.borrow_mut().push((request.path, request.body));
                Task::ready(Ok(()))
            })
        });
        let mut ends = manifest("ends", "Loose ends");
        ends.source = loose_ends::loose_ends();
        let mut slack = manifest("slack", "Slack · waiting on you");
        slack.source = SourceSpec::KeronSources("slack-waiting".to_string());
        home.apply_catalog(catalog(vec![ends, slack]), cx);
        let slack_row = |id: &str| ListItem {
            id: Some(id.to_string()),
            title: format!("Ana in #{id}"),
            actions: vec!["done".to_string()],
            ..ListItem::default()
        };
        home.widgets.entry("ends".to_string()).or_default().payload =
            Some(list((0..3).map(loose_end).collect()));
        home.widgets.entry("slack".to_string()).or_default().payload =
            Some(list(vec![slack_row("s1"), slack_row("s2")]));
        home.set_frame(0.0, 600.0, 600.0, 1.0, cx);
    });
    (home, posted, dir)
}

/// A list payload of these rows.
fn list(items: Vec<ListItem>) -> Payload {
    Payload {
        updated: None,
        errors: Vec::new(),
        body: Body::List(items),
    }
}

fn snooze() -> Action {
    Action::Snooze {
        until: "+2h".to_string(),
    }
}

fn done() -> Action {
    Action::Done { comment: None }
}

fn paths(posted: &Posted) -> Vec<String> {
    posted
        .borrow()
        .iter()
        .map(|(path, _)| path.clone())
        .collect()
}

#[gpui::test]
fn row_actions_wait_out_their_undo_window_before_they_go(cx: &mut TestAppContext) {
    let (home, posted, _dir) = held_home(cx);
    home.update(cx, |home, cx| {
        home.act("ends", "e0", done(), cx);
        home.act("ends", "e1", snooze(), cx);
        home.act("slack", "s1", done(), cx);
    });
    cx.executor()
        .advance_clock(UNDO_WINDOW - Duration::from_millis(1));
    cx.run_until_parked();
    assert!(posted.borrow().is_empty());

    // A refetch meanwhile keeps the rows as the owner left them.
    home.update(cx, |home, cx| {
        let rows = list((0..3).map(loose_end).collect());
        home.apply_fetch("ends", Instant::now(), Ok(rows), cx);
        assert_eq!(shown(home, "ends"), ["e1", "e2"]);
        assert_eq!(
            home.widgets["ends"].overrides["e1"].change,
            RowChange::Snoozed
        );
    });

    // Then they go, in the order taken.
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(
        paths(&posted),
        [
            "/memory/loose-ends/done",
            "/memory/loose-ends/snooze",
            "/sources/slack-waiting/done"
        ]
    );
    home.read_with(cx, |home, _| {
        assert!(home.held.is_empty());
        assert!(home.widgets["ends"].overrides["e0"].settled);
    });
}

#[gpui::test]
fn undo_takes_back_held_actions_newest_first_and_nothing_goes(cx: &mut TestAppContext) {
    let (home, posted, _dir) = held_home(cx);
    home.update(cx, |home, cx| {
        let note = Action::Done {
            comment: Some("called her back".to_string()),
        };
        home.act("ends", "e0", note, cx);
        home.act("ends", "e1", snooze(), cx);
        home.act("slack", "s1", done(), cx);
        // Snoozed, then done: undone once, it's snoozed again.
        home.act("ends", "e2", snooze(), cx);
        home.act("ends", "e2", done(), cx);
        assert_eq!(shown(home, "ends"), ["e1"]);

        assert!(home.undo(cx));
        assert_eq!(
            home.widgets["ends"].overrides["e2"].change,
            RowChange::Snoozed
        );
        assert!(home.undo(cx));
        assert!(!home.widgets["ends"].overrides.contains_key("e2"));
        assert!(home.undo(cx));
        assert_eq!(shown(home, "slack"), ["s1", "s2"]);
        assert!(home.undo(cx));
        assert!(!home.widgets["ends"].overrides.contains_key("e1"));
        assert!(!home.sent_notes.is_empty());
        assert!(home.undo(cx));
        assert_eq!(shown(home, "ends"), ["e0", "e1", "e2"]);
        assert!(home.sent_notes.is_empty(), "no note waits on e0");
        assert!(!home.undo(cx));
    });
    cx.executor().advance_clock(UNDO_WINDOW * 2);
    cx.run_until_parked();
    assert!(posted.borrow().is_empty());
}

#[gpui::test]
fn a_row_done_while_its_snooze_goes_stays_hidden_until_the_done_does(cx: &mut TestAppContext) {
    let (home, posted, _dir) = held_home(cx);
    home.update(cx, |home, cx| home.act("ends", "e2", snooze(), cx));
    cx.executor().advance_clock(UNDO_WINDOW / 2);
    home.update(cx, |home, cx| home.act("ends", "e2", done(), cx));
    cx.executor().advance_clock(UNDO_WINDOW / 2);
    cx.run_until_parked();
    assert_eq!(paths(&posted), ["/memory/loose-ends/snooze"]);

    // The door took the snooze and the card fetches again: the Done, still
    // held, keeps e2 hidden. Taken back, e2 is there, snoozed.
    home.update(cx, |home, cx| {
        let mut rows: Vec<ListItem> = (0..3).map(loose_end).collect();
        rows[2].snoozed_until = Some("2026-10-12T20:00:00Z".to_string());
        home.apply_fetch("ends", Instant::now(), Ok(list(rows)), cx);
        assert_eq!(shown(home, "ends"), ["e0", "e1"]);
        assert!(home.undo(cx));
        assert_eq!(shown(home, "ends"), ["e0", "e1", "e2"]);
    });
    cx.executor().advance_clock(UNDO_WINDOW);
    cx.run_until_parked();
    assert_eq!(paths(&posted), ["/memory/loose-ends/snooze"]);
}

#[gpui::test]
fn held_actions_go_at_once_when_home_goes_or_the_app_quits(cx: &mut TestAppContext) {
    let (home, posted, _dir) = held_home(cx);
    home.update(cx, |home, cx| home.act("ends", "e0", done(), cx));
    cx.update(|_| drop(home));
    cx.run_until_parked();
    assert_eq!(paths(&posted), ["/memory/loose-ends/done"]);

    let (home, posted, _dir) = held_home(cx);
    home.update(cx, |home, cx| home.act("slack", "s1", done(), cx));
    cx.quit();
    assert_eq!(paths(&posted), ["/sources/slack-waiting/done"]);
}

#[gpui::test]
fn quitting_sends_held_actions_and_waits_a_little_on_the_doors_answers(cx: &mut TestAppContext) {
    // Answered: the app can go once the answer is in.
    let (home, posted, _dir) = held_home(cx);
    home.update(cx, |home, cx| {
        home.act("ends", "e0", done(), cx);
        assert!(!home.ready_to_exit(cx));
    });
    assert_eq!(paths(&posted), ["/memory/loose-ends/done"]);
    cx.run_until_parked();
    assert!(home.update(cx, |home, cx| home.ready_to_exit(cx)));

    // Never answered: it goes anyway once the wait is up.
    let (home, _, _dir) = held_home(cx);
    let posted = Posted::default();
    home.update(cx, |home, cx| {
        home.poster = Some(unanswered(&posted));
        home.act("ends", "e0", done(), cx);
        assert!(!home.ready_to_exit(cx));
    });
    assert_eq!(paths(&posted), ["/memory/loose-ends/done"]);
    cx.executor()
        .advance_clock(EXIT_WAIT - Duration::from_millis(1));
    cx.run_until_parked();
    assert!(home.read_with(cx, |home, _| home.sending()));
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert!(home.update(cx, |home, cx| home.ready_to_exit(cx)));
}

#[gpui::test]
fn held_actions_wait_out_a_refused_sign_in_and_go_once_connected(cx: &mut TestAppContext) {
    // Signed in, the cards fetch on Tokio (and find no door).
    cx.update(gpui_tokio::init);
    let (home, posted, _dir) = held_home(cx);
    home.update(cx, |home, cx| {
        home.act("ends", "e0", done(), cx);
        home.act("ends", "e1", snooze(), cx);
        home.signed_out(cx);
        // Past undoing now, and off the card.
        assert!(!home.undo(cx));
        assert!(home.newest_held("ends").is_none());
    });
    cx.executor().advance_clock(UNDO_WINDOW);
    cx.run_until_parked();
    assert!(posted.borrow().is_empty());

    home.update(cx, |home, cx| home.signed_in(cx));
    assert_eq!(
        paths(&posted),
        ["/memory/loose-ends/done", "/memory/loose-ends/snooze"]
    );
    home.update(cx, |home, cx| {
        // Back before the door has answered, the rows are as they were left.
        home.apply_fetch(
            "ends",
            Instant::now(),
            Ok(list((0..3).map(loose_end).collect())),
            cx,
        );
        assert_eq!(shown(home, "ends"), ["e1", "e2"]);
        assert_eq!(
            home.widgets["ends"].overrides["e1"].change,
            RowChange::Snoozed
        );
    });
}

/// The new-chat screen in small: a composer, Home under it, and Undo routed
/// to Home as the shell routes it.
struct UndoHost {
    composer: Entity<ComposerInput>,
    home: Entity<Home>,
}

impl Render for UndoHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_action(undo_listener(self.home.clone()))
            .child(div().h(px(40.0)).child(self.composer.clone()))
            .child(div().flex_1().child(self.home.clone()))
    }
}

/// [`UndoHost`] with the app's key bindings and three loose ends whose
/// actions are noted and never answered.
fn undo_host(
    cx: &mut TestAppContext,
) -> (
    Entity<Home>,
    Entity<ComposerInput>,
    Posted,
    &mut VisualTestContext,
    tempfile::TempDir,
) {
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(crate::theme::Theme::default());
        crate::composer::init(cx, Default::default());
        crate::app_menus::bind_keys(cx);
    });
    let dir = tempfile::tempdir().unwrap();
    let paths = HomePaths::under_home(dir.path());
    let (host, cx) = cx.add_window_view(|_, cx| UndoHost {
        composer: cx.new(|cx| ComposerInput::new("Do anything…", cx)),
        home: cx.new(|cx| Home::with_parts(paths, None, cx)),
    });
    cx.simulate_resize(size(px(800.0), px(480.0)));
    cx.run_until_parked();
    let (home, composer) = host.read_with(cx, |host, _| (host.home.clone(), host.composer.clone()));
    let posted = Posted::default();
    home.update(cx, |home, cx| {
        home.poster = Some(unanswered(&posted));
        let mut ends = manifest("ends", "Loose ends");
        ends.source = loose_ends::loose_ends();
        home.apply_catalog(catalog(vec![ends]), cx);
        home.widgets.entry("ends".to_string()).or_default();
        home.sign_in = SignIn::SignedIn;
        home.set_frame(0.0, 600.0, 600.0, 1.0, cx);
    });
    set_loose_ends(&home, 3, cx);
    (home, composer, posted, cx, dir)
}

#[gpui::test]
fn cmd_z_takes_back_a_row_action_unless_the_composer_was_typed_in_since(cx: &mut TestAppContext) {
    let (home, composer, posted, cx, _dir) = undo_host(cx);
    let text =
        |cx: &mut VisualTestContext| composer.read_with(cx, |input, _| input.text().to_string());
    let rows = |cx: &mut VisualTestContext| home.read_with(cx, |home, _| shown(home, "ends"));
    cx.update(|window, cx| window.focus(&composer.focus_handle(cx), cx));
    cx.simulate_input("draft");

    // Done clicked, focus still in the composer: ⌘Z brings the row back,
    // and the line saying it was done goes.
    home.update(cx, |home, cx| home.act("ends", "e0", done(), cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("home-undo-ends").is_some());
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(rows(cx), ["e0", "e1", "e2"]);
    assert_eq!(text(cx), "draft");
    assert!(cx.debug_bounds("home-undo-ends").is_none());

    // Typed after the action: ⌘Z takes back the typing, then the action.
    home.update(cx, |home, cx| home.act("ends", "e1", done(), cx));
    cx.simulate_input(" more");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(cx), "draft");
    assert_eq!(rows(cx), ["e0", "e2"]);
    // Edit › Undo does as ⌘Z.
    cx.dispatch_action(Undo);
    cx.run_until_parked();
    assert_eq!(rows(cx), ["e0", "e1", "e2"]);
    assert_eq!(text(cx), "draft");
    // With nothing held, ⌘Z is the composer's again.
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(cx), "");

    // A word begun before a row action and finished after it is newer.
    cx.simulate_input("o");
    home.update(cx, |home, cx| home.act("ends", "e2", done(), cx));
    cx.simulate_input("k");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(cx), "");
    assert_eq!(rows(cx), ["e0", "e1"]);
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(rows(cx), ["e0", "e1", "e2"]);

    cx.executor().advance_clock(UNDO_WINDOW);
    cx.run_until_parked();
    assert!(posted.borrow().is_empty());
}

#[gpui::test]
fn cmd_z_or_the_cards_undo_takes_back_a_row_action_off_the_keyboard(cx: &mut TestAppContext) {
    let (home, _composer, posted, cx, _dir) = undo_host(cx);
    let snooze = home.read_with(cx, |home, _| {
        home.widgets["ends"].row_ui.borrow()["e0"].snooze.clone()
    });
    let change = |cx: &mut VisualTestContext| {
        home.read_with(cx, |home, _| {
            home.widgets["ends"]
                .overrides
                .get("e0")
                .map(|change| change.change)
        })
    };
    cx.simulate_keystrokes("a");
    cx.update(|window, cx| window.focus(&snooze, cx));
    // The menu opens on Enter, and Enter picks its first snooze; focus is
    // back on Snooze, where ⌘Z takes it back.
    press("enter enter", cx);
    assert_eq!(change(cx), Some(RowChange::Snoozed));
    assert!(cx.update(|window, _| snooze.is_focused(window)));
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(change(cx), None);

    // The card's own Undo does too.
    home.update(cx, |home, cx| home.act("ends", "e0", done(), cx));
    cx.executor().advance_clock(leave_duration());
    cx.run_until_parked();
    let undo = cx.debug_bounds("home-undo-ends").unwrap();
    cx.simulate_click(undo.center(), Modifiers::none());
    assert_eq!(change(cx), None);

    cx.executor().advance_clock(UNDO_WINDOW);
    cx.run_until_parked();
    assert!(posted.borrow().is_empty());
}

#[gpui::test]
fn the_card_says_its_newest_held_action_and_its_undo_takes_that_one_back(cx: &mut TestAppContext) {
    let (home, _composer, posted, cx, _dir) = undo_host(cx);
    let line = |cx: &mut VisualTestContext| {
        home.read_with(cx, |home, _| home.newest_held("ends").map(HeldAction::line))
    };
    home.update(cx, |home, cx| {
        home.act("ends", "e0", Action::Dismiss { comment: None }, cx);
        home.act("ends", "e1", snooze(), cx);
    });
    cx.run_until_parked();
    assert_eq!(line(cx).as_deref(), Some("Snoozed: Loose end 1"));

    let undo = cx.debug_bounds("home-undo-ends").unwrap();
    cx.simulate_click(undo.center(), Modifiers::none());
    assert_eq!(line(cx).as_deref(), Some("Not a thing: Loose end 0"));
    home.read_with(cx, |home, _| {
        assert_eq!(shown(home, "ends"), ["e1", "e2"]);
        assert!(!home.widgets["ends"].overrides.contains_key("e1"));
    });

    cx.executor().advance_clock(UNDO_WINDOW);
    cx.run_until_parked();
    assert_eq!(paths(&posted), ["/memory/loose-ends/dismiss"]);
}

#[gpui::test]
fn cmd_z_leaves_held_row_actions_be_while_home_is_off_screen(cx: &mut TestAppContext) {
    let (home, composer, posted, cx, _dir) = undo_host(cx);
    let text =
        |cx: &mut VisualTestContext| composer.read_with(cx, |input, _| input.text().to_string());
    let rows = |cx: &mut VisualTestContext| home.read_with(cx, |home, _| shown(home, "ends"));
    cx.update(|window, cx| window.focus(&composer.focus_handle(cx), cx));
    cx.simulate_input("draft");
    home.update(cx, |home, cx| home.act("ends", "e0", done(), cx));

    // A chat opens over Home: ⌘Z there is the composer's, and Undo that no
    // text input takes leaves the Done be too.
    home.update(cx, |home, cx| home.hide(cx));
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(cx), "");
    assert!(!home.update(cx, |home, cx| home.undo(cx)));
    assert_eq!(rows(cx), ["e1", "e2"]);

    // Back on Home inside the window, ⌘Z takes it back.
    home.update(cx, |home, cx| home.set_frame(0.0, 600.0, 600.0, 1.0, cx));
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(rows(cx), ["e0", "e1", "e2"]);

    // Off screen as its window passes, a Done goes.
    home.update(cx, |home, cx| {
        home.act("ends", "e1", done(), cx);
        home.hide(cx);
    });
    cx.simulate_keystrokes("cmd-z");
    cx.executor().advance_clock(UNDO_WINDOW);
    cx.run_until_parked();
    let done: Vec<serde_json::Value> = posted
        .borrow()
        .iter()
        .filter(|(path, _)| path == "/memory/loose-ends/done")
        .map(|(_, body)| body["ids"].clone())
        .collect();
    assert_eq!(done, [serde_json::json!(["e1"])]);
}

#[test]
fn a_series_keeps_its_color_by_what_it_is() {
    let jobs = [
        "loose-ends",
        "brief",
        "naps",
        "judge",
        "screen-notes",
        "compare",
        "checks",
        "backup",
    ];
    let apps = [
        "claude-code",
        "codex",
        "claude-ai",
        "cursor",
        "chatgpt",
        "keron-app",
        "capture",
        "other",
    ];
    // Each set has every slot once, so neighbours in a stack stay apart.
    for set in [jobs, apps] {
        let slots: Vec<usize> = set.iter().map(|id| dashboard::slot(id)).collect();
        assert_eq!(slots, (0..8).collect::<Vec<_>>());
    }
    assert_ne!(dashboard::slot("rounds"), dashboard::slot("failures"));
    // Anything new lands on a slot of its own name, the same every time.
    let new = dashboard::slot("a-new-job");
    assert!(new < crate::theme::CHART_SLOTS);
    assert_eq!(dashboard::slot("a-new-job"), new);
}

/// A made-up Mini payload: Today with three tiles (the card names two),
/// 7 days with one tile and a table, a year of days, and one problem.
fn mini_payload() -> Payload {
    let tile = |id: &str, text: &str| serde_json::json!({"id": id, "label": id, "text": text});
    let chart = serde_json::json!({
        "id": "tokens", "title": "Tokens by job", "unit": "tokens", "labels": ["00", "01", "02"],
        "series": [{"id": "naps", "label": "Naps", "values": [0, 1200, 300]},
                   {"id": "judge", "label": "Judge", "values": [100, 0, 0]}],
        "texts": ["00:00 · 100 tokens", "01:00 · 1.2k tokens", "02:00 · 300 tokens"],
        "line": {"value": 1000, "label": "a line"}
    });
    let served = serde_json::json!({
        "source": "dashboard", "kind": "dashboard", "errors": ["unpriced: a-model"],
        "note": "Dollars are estimates.",
        "card": {"period": "day", "tiles": ["tokens", "runs"], "chart": "tokens"},
        "periods": {
            "day": {"label": "Today", "range": "Mon 12 Oct",
                    "tiles": [tile("tokens", "1.6k"), tile("runs", "3"), tile("notes", "9")],
                    "charts": [chart.clone()], "breakdowns": []},
            "week": {"label": "7 days", "range": "6–12 Oct",
                     "tiles": [tile("tokens", "8.4k")], "charts": [chart],
                     "breakdowns": [{"id": "jobs", "title": "By job", "columns": ["Tokens"],
                                     "rows": [{"label": "Naps", "cells": ["8.4k"], "share": 1}]}]}
        },
        "heatmap": {"label": "Tokens per day", "end": "2026-10-12", "levels": [100, 500, 1000],
                    "days": [{"date": "2026-10-12", "value": 1600, "text": "Mon 12 Oct · 1.6k tokens"}]}
    });
    keron_home::kinds::parse_payload(Kind::Dashboard, &served).unwrap()
}

/// Home drawn signed in with the Mini card, and `others` cards after it.
fn mini_home<'a>(
    others: &[&str],
    cx: &'a mut TestAppContext,
) -> (Entity<Home>, &'a mut VisualTestContext, tempfile::TempDir) {
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(crate::theme::Theme::default());
    });
    let dir = tempfile::tempdir().unwrap();
    let paths = HomePaths::under_home(dir.path());
    let (home, cx) = cx.add_window_view(|_, cx| Home::with_parts(paths, None, cx));
    cx.simulate_resize(size(px(800.0), px(900.0)));
    cx.run_until_parked();
    home.update(cx, |home, cx| {
        let mini = Manifest {
            kind: Kind::Dashboard,
            source: SourceSpec::Memory("dashboard".to_string()),
            wide: true,
            ..manifest("mini", "Mini")
        };
        let mut manifests = vec![mini];
        manifests.extend(others.iter().map(|id| manifest(id, id)));
        // Signed in only once it's added, and with a refresh loop standing
        // in for its own, so nothing fetches.
        home.apply_catalog(catalog(manifests), cx);
        let state = home.widgets.entry("mini".to_string()).or_default();
        state.payload = Some(mini_payload());
        state.refresh = Some(Task::ready(()));
        for id in others {
            home.widgets.entry(id.to_string()).or_default().payload = Some(rows(&[id]));
        }
        home.sign_in = SignIn::SignedIn;
        home.set_frame(0.0, 600.0, 600.0, 1.0, cx);
    });
    cx.run_until_parked();
    settle(cx);
    (home, cx, dir)
}

fn page_open(home: &Entity<Home>, cx: &mut VisualTestContext) -> bool {
    home.read_with(cx, |home, _| home.dashboard_open())
}

fn click(selector: &'static str, cx: &mut VisualTestContext) {
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} isn't drawn"))
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn the_mini_card_draws_its_tiles_and_a_click_opens_its_page(cx: &mut TestAppContext) {
    let (home, cx, _dir) = mini_home(&[], cx);
    // The card's two tiles of today, not the third, and the problem, quietly.
    assert!(cx.debug_bounds("home-dashboard-tile-tokens").is_some());
    assert!(cx.debug_bounds("home-dashboard-tile-runs").is_some());
    assert!(cx.debug_bounds("home-dashboard-tile-notes").is_none());
    assert!(cx.debug_bounds("home-dashboard-problems").is_some());
    assert!(!page_open(&home, cx));

    let card = home.read_with(cx, |home, _| home.card_bounds.borrow()["mini"]);
    cx.simulate_click(card.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(page_open(&home, cx));
    // The page in the card's place, on Week: its tiles, not today's.
    assert!(cx.debug_bounds("home-dashboard-page").is_some());
    assert!(cx.debug_bounds("home-dashboard-tile-tokens").is_none());
    assert!(cx.debug_bounds("home-dashboard-page-tile-tokens").is_some());
    assert!(cx.debug_bounds("home-dashboard-page-tile-runs").is_none());
}

#[gpui::test]
fn the_period_switch_changes_the_tiles_and_stays_through_a_refresh(cx: &mut TestAppContext) {
    let (home, cx, _dir) = mini_home(&[], cx);
    click("home-dashboard-open", cx);
    assert_eq!(home.read_with(cx, |home, _| home.period), Period::Week);

    click("home-dashboard-period-day", cx);
    assert!(cx.debug_bounds("home-dashboard-page-tile-runs").is_some());
    assert!(cx.debug_bounds("home-dashboard-page-tile-notes").is_some());

    // A refresh keeps the period, and so does going back and in again.
    home.update(cx, |home, cx| {
        home.apply_fetch("mini", Instant::now(), Ok(mini_payload()), cx);
        home.close_dashboard(cx);
    });
    cx.run_until_parked();
    click("home-dashboard-open", cx);
    assert_eq!(home.read_with(cx, |home, _| home.period), Period::Day);
    assert!(cx.debug_bounds("home-dashboard-page-tile-runs").is_some());
}

#[gpui::test]
fn escape_or_back_closes_the_page(cx: &mut TestAppContext) {
    let (home, cx, _dir) = mini_home(&[], cx);
    click("home-dashboard-open", cx);
    assert!(page_open(&home, cx));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!page_open(&home, cx));
    assert!(cx.debug_bounds("home-dashboard-tile-tokens").is_some());

    click("home-dashboard-open", cx);
    click("home-dashboard-back", cx);
    assert!(!page_open(&home, cx));
    assert!(cx.debug_bounds("home-dashboard-page").is_none());
}

#[gpui::test]
fn hovering_a_heatmap_day_shows_its_tooltip(cx: &mut TestAppContext) {
    let (home, cx, _dir) = mini_home(&[], cx);
    let heatmap = cx.debug_bounds("home-dashboard-heatmap-card-mini").unwrap();
    let pitch = f32::from(heatmap.size.width) / keron_home::dashboard::WEEKS as f32;
    assert!(pitch > 4.0 && f32::from(heatmap.size.height) > 6.0 * pitch);
    let hovered = |cx: &mut VisualTestContext| {
        home.read_with(cx, |home, _| {
            home.heat_hover.as_ref().map(|hover| hover.day)
        })
    };
    // Today, a Monday: the top of the last week's column.
    let today = heatmap.origin + point(px(52.5 * pitch), px(0.5 * pitch));
    cx.simulate_mouse_move(today, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(hovered(cx), Some(52 * 7));
    assert!(cx.debug_bounds("home-dashboard-heat-tip").is_some());
    // The day under it in that column is still to come: nothing there.
    cx.simulate_mouse_move(today + point(px(0.0), px(pitch)), None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(hovered(cx), None);
    assert!(cx.debug_bounds("home-dashboard-heat-tip").is_none());
    // The first week's Monday.
    cx.simulate_mouse_move(
        heatmap.origin + point(px(0.5 * pitch), px(0.5 * pitch)),
        None,
        Modifiers::none(),
    );
    cx.run_until_parked();
    assert_eq!(hovered(cx), Some(0));
}

#[gpui::test]
fn customize_still_drags_the_mini_card(cx: &mut TestAppContext) {
    let (home, cx, _dir) = mini_home(&["other"], cx);
    home.update(cx, |home, cx| home.toggle_customize(cx));
    cx.run_until_parked();
    settle(cx);
    let (mini, other) = home.read_with(cx, |home, _| {
        let bounds = home.card_bounds.borrow();
        (bounds["mini"].center(), bounds["other"].center())
    });
    // A click there doesn't open the page...
    cx.simulate_click(mini, Modifiers::none());
    cx.run_until_parked();
    assert!(!page_open(&home, cx));

    // ...and a drag moves the card.
    cx.simulate_mouse_down(mini, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(other, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_move(other, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(other, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert!(!page_open(&home, cx));
    let order = home.read_with(cx, |home, _| ids(home.arranged()));
    assert_eq!(order, ["other", "mini"]);
}
