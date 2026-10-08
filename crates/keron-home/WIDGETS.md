# Widgets

Home is the grid of cards under the composer in the Keron app. Each card is a
widget. This folder (`~/.keron/widgets`) holds them.

A widget is two things:

- a **manifest**: a small TOML file, `<id>.toml`, that says what the card is
  called, how it looks and where its data comes from;
- a **source**: where the data comes from. That can be your memory, the
  mini's mail, calendar and Slack connectors, the app itself, or a script of
  your own.

The app draws a few kinds of card (list, timeline, stat, agenda, devices). A
new kind needs a new version of the app. A new widget doesn't: drop a manifest
(and its script, if it has one) into this folder and the card appears on Home
straight away, with no rebuild and no restart.

## The manifest

The file name, without `.toml`, is the widget's id. Use lowercase letters,
digits, `-` and `_`, starting with a letter or digit, at most 48 characters.
`weather.toml` is the widget `weather`.

```toml
# What this widget shows, in one line.
title  = "Slack · waiting on you"
kind   = "list"
source = "keron-sources:slack-waiting"
every  = "1m"
icon   = "chat"
wide   = false
limit  = 8
empty  = "Nobody's waiting"
```

| Key | Needed | Default | What it does |
|---|---|---|---|
| `title` | yes | | The card's heading. At most 80 characters. |
| `kind` | yes | | How the card is drawn: `list`, `timeline`, `stat`, `agenda` or `devices`. |
| `source` | yes | | Where the data comes from. See "Sources". |
| `every` | no | `1m` | How often to refresh: `30s`, `5m`, `1h`. A bare number is seconds. The fastest is `5s`, the slowest `24h`. |
| `icon` | no | none | One of `flag`, `chat`, `check`, `calendar`, `tree`, `pulse`, `devices`, `pr`, `mail`, `mic`, `bars`, `widget`, `bell`, `star`, `list`, `globe`. |
| `wide` | no | `false` | Take two columns instead of one, until you resize it in Customize. |
| `limit` | no | all | The most rows shown, from 1 to 50. |
| `empty` | no | | The text shown when there are no rows. |

Any other key is an error, so a typo shows up instead of being ignored. A
manifest that doesn't parse is listed in Customize with the problem, so it
isn't silently missing.

## Sources

| Source | Where the data comes from |
|---|---|
| `keron-sources:<name>` | The mini's read-only connectors, through your memory's door: `gmail-needs-reply`, `calendar-today`, `slack-waiting`. |
| `memory:<name>` | Your memory, through the door: `loose-ends`, `today`. |
| `zeron:<name>` | What the app already knows: `sessions` (agent chats that are working, waiting on you or stuck), `devices`, `pull-requests`. `services` and `todos` are reserved; the app has no data for them yet. |
| `script:<path>` | A program on this Mac that prints the data as JSON. A relative path is inside this folder; `~/` starts at your home folder. |

Door sources need you signed in to your memory. When you aren't, the card
says so and offers the sign-in.

## The JSON each kind takes

Every source gives one JSON object. Two fields work on every kind:

- `updated`: when the data was fetched, as an ISO 8601 time
  (`"2026-10-08T09:30:00Z"`).
- `errors`: a list of short problems, like `["work: token expired"]`. The rows
  still show; the card notes the problems.

Fields the app doesn't know are ignored. Every field is optional except each
row's main text. A row that can't be read is skipped and counted in
`errors`, so one bad row doesn't blank the card.

### list

Rows in `items`. `title` is needed.

```json
{
  "items": [
    {"title": "Reply to the venue about Friday", "sub": "events@example.com",
     "badge": "work", "age": "3h", "link": "https://mail.example.com/t/1"},
    {"title": "Send the draft", "heat": 3, "at": "2026-10-08T07:12:00Z"}
  ]
}
```

| Field | What it is |
|---|---|
| `title` | The row's text. |
| `sub` | A second, quieter line. |
| `badge` | A short pill, like `work` or `draft`. |
| `age` | A short age, like `12m` or `3h`. |
| `at` | When it happened (ISO 8601). Used for the age when `age` is missing. |
| `heat` | 0 to 4. See "Heat". |
| `link` | Opened when you click the row. Only `http://` and `https://` links open. |
| `account` | Which account the row belongs to, like `personal` or `work`. |
| `status` | For agent rows: `working`, `waiting`, `error`, `done` or `idle`. |
| `id` | A stable id. Rows need it for `actions`. |
| `actions` | What you can do to the row from Home, shown as buttons when you hover it: `snooze`, `dismiss` ("Not a thing") and `done`. Only door sources take them, and the app posts each one to the row's own source (`/memory/loose-ends/done`, `/sources/slack-waiting/done`). Loose ends offer all three. Gmail and Slack rows offer `done`, meaning nobody needs a reply from you: the row stays hidden until a newer message comes. Rows from `zeron:` and `script:` sources get no buttons. |
| `notify` | Loose ends only: the heat level (3 hot, 4 burning) a Mac notification is due for, set by the memory server. Other widgets' `notify` is ignored. |

### timeline

Rows in `items`, newest first. `text` is needed.

```json
{
  "items": [
    {"time": "14:05", "kind": "agent", "tag": "mbp/claude-code", "text": "Finished the layout tests", "n": 412},
    {"time": "09:10", "kind": "did", "text": "Wrote the weekly plan"}
  ]
}
```

`time` is `HH:MM` local time. `kind` is one of `did`, `said`, `heard`,
`agent` or `nudge`. `tag` is where the note came from. `n` is the note's
number in your memory.

### stat

One big number, at the top level (no `items`).

```json
{"value": 1240, "label": "Steps today", "series": [800, 950, 1100, 1240], "delta": "+12%"}
```

`value` can be a number or a string (`"4.2k"`). `series` draws small bars,
oldest first; the last one is now.

### agenda

Rows in `items`. `title` and `start` are needed.

```json
{
  "items": [
    {"title": "Design review", "start": "2026-10-08T15:00:00+01:00", "end": "2026-10-08T15:30:00+01:00",
     "attendees": 4, "video_link": "https://meet.example.com/abc", "warn": "deck not sent"},
    {"title": "Holiday", "start": "2026-10-08", "all_day": true}
  ]
}
```

`start` and `end` are ISO 8601 times, or `YYYY-MM-DD` dates for all-day
events. `warn` is a short warning chip.

### devices

Rows in `items`. `name` is needed.

```json
{
  "items": [
    {"name": "Mac mini", "platform": "macos", "online": true, "role": "memory server", "detail": "4,214 notes"},
    {"name": "iPhone", "platform": "ios", "online": false, "detail": "seen 3h ago"}
  ]
}
```

## Scripts

A `script:` source runs a program and reads what it prints. The contract:

- It prints **one JSON object** on stdout, in the shape of the widget's kind,
  and exits with status 0.
- It finishes within **10 seconds**, or it is stopped.
- It prints at most **1 MiB**.
- It runs **in this folder**, with your login shell's `PATH`, so `uv`,
  `node`, `python3` and Homebrew tools are found.
- It runs **directly, not through a shell**. Make it executable
  (`chmod +x my-widget.sh`) and start it with a `#!` line, like
  `#!/bin/sh` or `#!/usr/bin/env python3`.
- It gets no input. A non-zero exit shows the first line of what it printed
  on stderr.

It runs again every `every`. It is stopped if you leave Home before it
finishes.

## Heat

Loose ends carry a heat from 0 to 4, drawn as four rising bars in one colour
with a word:

| Heat | Word | Meaning |
|---|---|---|
| 0 | off | Done, dismissed or expired. |
| 1 | quiet | Open, nothing pressing yet. |
| 2 | warm | It has been open a while, a deadline is near, or someone is waiting. |
| 3 | hot | It needs attention soon. |
| 4 | burning | It needs you now. The bars pulse slowly (not with Reduce Motion). |

Heat rises as an item stays open, as a deadline comes closer, when a person
is waiting on you, and each time it was shown without a move. Snoozing holds
it still until the snooze ends.

Any list widget can use `heat`. Values outside 0 to 4 are clamped.

## Layout: home.toml

Which widgets show, in what order and how wide lives in `~/.keron/home.toml`.
Customize on Home writes it for you: toggle widgets, drag to reorder, and
switch between one and two columns.

```toml
[[widget]]
id = "loose-ends"
shown = true
width = 2

[[widget]]
id = "devices"
shown = false
```

- The order of the `[[widget]]` tables is the order on Home.
- `shown` defaults to `true`.
- `width` is 1 or 2 columns. When it's missing, the manifest's `wide` decides.
- A widget that isn't listed yet (one you just added) shows at the end.
- An entry whose manifest is gone is kept, so the widget gets its place back
  if you add it again.

## Adding a widget

1. Write `<id>.toml` in this folder.
2. For a `script:` source, put the script next to it and make it executable.
3. Home picks it up straight away. If it doesn't parse, Customize shows why.

To remove a widget, delete its manifest, or hide it in Customize. This folder
is only filled once, the first time the app runs; the built-in widgets you
delete stay deleted.

You can also ask for one. "Describe a widget" in Customize sends your
request to Keron's main chat, where an agent writes the manifest and script
into this folder, following this file.

Example: a script that counts files in your Downloads folder.

```toml
# How many files are sitting in Downloads.
title  = "Downloads"
kind   = "stat"
source = "script:downloads.sh"
every  = "5m"
icon   = "bars"
```

```sh
#!/bin/sh
count=$(ls -1 "$HOME/Downloads" | wc -l | tr -d ' ')
printf '{"value": %s, "label": "files in Downloads"}\n' "$count"
```
