use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{AppContext as _, Entity, TestAppContext};
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
        problems: Vec::new(),
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
