# Keron relay

The relay is Zeron's edge Worker (`edge/`), deployed to the owner's own
Cloudflare account. It carries chats, the workspace registry and device
control between the Keron app on the MacBook, the iPhone and the Mac mini's
agent host. The app finds it through `keron.toml` at the repository root.

Run everything below from `edge/` on a machine with Node 22. Nothing here is
secret except the three `secret put` values, which go straight into
Cloudflare and never into a file in this repository.

## Values to fill in first

| Value | Where it goes |
|---|---|
| Relay hostname, e.g. `relay.example.com` | `keron.toml` `relay.host`; `edge/wrangler.jsonc` `routes[0].pattern` |
| WorkOS client id of the relay's AuthKit app | `keron.toml` `relay.workos_client_id`; `edge/wrangler.jsonc` `vars.WORKOS_CLIENT_ID` |
| Apple team id | `keron.toml` `apple.team_id`; `edge/wrangler.jsonc` `vars.APNS_TEAM_ID` |
| Bundle id prefix, e.g. `com.example` | `keron.toml` `apple.bundle_prefix`; `edge/wrangler.jsonc` `vars.APNS_TOPIC` = `<prefix>.keron.ios` |
| Cloudflare account id | `edge/wrangler.jsonc` `account_id` |

Then, from the repository root:

```sh
cargo run -p keron-config --features cli -- xcconfig > apps/ios/Keron.xcconfig
cargo run -p keron-config --features cli -- check
```

`check` must print no `mismatch:` and no `placeholder:` lines before you
deploy.

## WorkOS

Create an AuthKit application for the relay (separate from the public door's)
and one organization with your account in it. Add these redirect URIs:

| Redirect URI | Used by | Code |
|---|---|---|
| `http://127.0.0.1:27641/callback` | Desktop app sign-in (loopback) | `crates/engine/src/auth.rs` `start_sign_in`; port from `crates/engine/src/lib.rs` (`ZERON_CALLBACK_PORT`, default 27641) |
| `https://<relay host>/auth/cli/callback` | `keron login` on a headless machine (paste-code page) | `crates/engine/src/auth.rs` `start_headless_sign_in`; page in `edge/src/auth-routes.ts` |
| `keron://callback` | iPhone app | `crates/client/src/auth.rs` `CALLBACK_SCHEME`; `apps/ios/Zeron/Info.plist` `CFBundleURLSchemes` |

Copy the client id into `keron.toml` and `wrangler.jsonc`, and keep the API
key for the `WORKOS_API_KEY` secret below.

## Deploy

```sh
cd edge
npm ci
npx wrangler login
npx wrangler r2 bucket create keron-blobs
npx wrangler r2 bucket create keron-releases
npx wrangler deploy
npx wrangler secret put WORKOS_API_KEY
npx wrangler secret put APNS_KEY_ID
npx wrangler secret put APNS_KEY_P8 < /path/to/AuthKey_XXXXXXXXXX.p8
```

- R2 needs a payment method on the Cloudflare account. Workers Paid avoids the
  free plan's daily Durable Object row-write limit.
- `secret put` prompts for the value (or reads it from the pipe), so it never
  lands in shell history. Each `secret put` rolls out a new version.
- The relay's hostname must be on a zone in the same Cloudflare account;
  `deploy` creates the DNS record and the certificate for the custom domain.
- Never deploy with `AUTH_MODE` set to `dev`: it accepts any bearer without
  verifying it. `npm run dev` uses it for a local `wrangler dev` only.

## Check it

```sh
curl -fsS https://<relay host>/health
```

prints `{"ok":true,"auth":"workos"}`.

```sh
npx wrangler secret list
```

lists `WORKOS_API_KEY`, `APNS_KEY_ID` and `APNS_KEY_P8` (names only).
`pushConfigured` in `GET /registry/<org id>/stats` is true exactly when
`APNS_KEY_P8` and `APNS_KEY_ID` are both set (`edge/src/env.ts`
`apnsConfig`). That route needs an org-scoped WorkOS access token
(`Authorization: Bearer …`); the response's `pushTargets` and `pushLog` show
the iPhone registering and each push decision.

## Other configs in edge/

`wrangler.chat2test.jsonc` and `wrangler.transport-test.jsonc` are Zeron's
own throwaway test deployments (the first names Zeron's Cloudflare account).
Both run with `AUTH_MODE=dev`, so never deploy them anywhere reachable. Only
`wrangler.jsonc` is the relay.

## Redeploying

`npx wrangler deploy` again after any change to `edge/`. Never rename the
worker (`keron-edge`): Durable Object storage is bound to its name.
