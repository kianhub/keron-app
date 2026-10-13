# Keron app: instructions for agents

This folder is the Keron app, a fork of Zeron. It's the `app/` submodule of
the Keron repo, whose main checkout is `~/Developer/keron`. Keron's
instructions and settled decisions live there, so read these first:

1. `~/Developer/keron/AGENTS.md`
2. `~/Developer/keron/docs/architecture.md`

(From the main app checkout, `~/Developer/keron/app`, those are
`../AGENTS.md` and `../docs/architecture.md`.)

## Working on the app from inside Keron Dev

You may be running inside Keron Dev, the development build of this app. When
the owner quits it, every agent in it stops, and so does anything an agent
started.

- Never quit, kill or reopen Keron Dev yourself, and don't run
  `scripts/run-macos-dev.sh` while it's running.
- After changing the app and testing it, run
  `sh ~/Developer/keron/ops/keron-dev.sh relaunch`, from any folder. It
  builds `~/Developer/keron/app` as it is on disk first, so give it a long
  timeout. A change that's only in a worktree isn't in that build.
- It leaves its request in `~/.keron`. If it says your shell can't write
  there, you're in a sandbox: ask the owner for full access, or to run it.
- Then tell the owner it's ready: when they quit Keron Dev (⌘Q), it reopens
  with the new build, and their chats are kept. Your session ends when they
  quit, so put everything that's left (what changed, what to check) in that
  reply.
- `sh ~/Developer/keron/ops/keron-dev.sh cancel` takes the request back, and
  `status` shows what's waiting.
