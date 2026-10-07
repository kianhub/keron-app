import type { ApnsConfig } from "./apns";

export interface Env {
  SESSION_ROOMS: DurableObjectNamespace;
  DEVICE_ROOMS: DurableObjectNamespace;
  PREVIEW_ROOMS: DurableObjectNamespace;
  /** Per-user workspace registries (`reg1/{orgId}/{userId}`) — the row-table
   * replacement for the Loro workspace doc (docs/registry-sync.md). */
  REGISTRY_ROOMS: DurableObjectNamespace;
  /** chat2 session rooms (`chat2/{chatId}`) — dumb authenticated log relays
   * replacing SessionRoom's loro-aware s2 rooms (docs/chat2-sync.md). */
  CHAT_ROOMS: DurableObjectNamespace;
  BLOBS: R2Bucket;
  /** Release artifacts (headless tarballs, dmgs, latest.txt) served at
   * /releases/* for the curl-install flow. */
  RELEASES: R2Bucket;
  WORKOS_CLIENT_ID: string;
  /** "workos" (verify AuthKit JWTs) or "dev" (bearer == userId, never prod). */
  AUTH_MODE: string;
  /** Optional overrides for the WorkOS trust anchor. */
  WORKOS_ISSUER?: string;
  WORKOS_JWKS_URL?: string;
  /** WorkOS secret API key (wrangler secret) — powers the absorbed /auth/*
   * routes (code exchange, refresh, orgs). Unset ⇒ those routes answer 501,
   * matching the old apps/server dev-mode behavior. */
  WORKOS_API_KEY?: string;
  /** APNs auth key (contents of AuthKey_XXXX.p8, wrangler secret) and its
   * key id. Unset ⇒ session notifications are decided and logged, not sent. */
  APNS_KEY_P8?: string;
  APNS_KEY_ID?: string;
  /** Apple team id and the iPhone app's bundle id. No defaults: push needs
   * both once the APNs key is set (see `apnsConfig`). */
  APNS_TEAM_ID?: string;
  APNS_TOPIC?: string;
}

/** APNs settings, when push is set up for this deployment: undefined without
 * the key secrets. With them, a missing APNS_TEAM_ID or APNS_TOPIC throws —
 * there is no fallback Apple identity to send as. */
export const apnsConfig = (env: Env): ApnsConfig | undefined => {
  const { APNS_KEY_P8: keyP8, APNS_KEY_ID: keyId, APNS_TEAM_ID: teamId, APNS_TOPIC: topic } = env;
  if (!keyP8 || !keyId) return undefined;
  if (!teamId || !topic) {
    const missing = [!teamId && "APNS_TEAM_ID", !topic && "APNS_TOPIC"].filter(Boolean).join(" and ");
    throw new Error(`push is misconfigured: ${missing} unset (edge/wrangler.jsonc vars)`);
  }
  return { keyP8, keyId, teamId, topic };
};

/** `pushConfigured` for /stats, with `pushError` when the settings are
 * incomplete (see `apnsConfig`). */
export const apnsStatus = (env: Env): { pushConfigured: boolean; pushError?: string } => {
  try {
    return { pushConfigured: apnsConfig(env) !== undefined };
  } catch (err) {
    return { pushConfigured: false, pushError: (err as Error).message };
  }
};

/** Header the Worker stamps on requests it forwards into DOs after verifying
 * the caller's JWT. DOs trust it blindly — they are only reachable through
 * the Worker (design §2: "DO never sees an unauthenticated frame"). */
export const AUTH_USER_HEADER = "x-zeron-auth-user";

/** Header the Worker stamps on requests forwarded into workspace-doc rooms
 * (`ws/{orgId}`). Membership (JWT org claim == orgId) is enforced at the
 * Worker; the SessionRoom DO sees this and skips its per-chat
 * claim-on-first-join ownership discipline for the room. */
export const ROOM_KIND_HEADER = "x-zeron-room-kind";
