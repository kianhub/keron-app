import { describe, expect, it } from "vitest";
import { apnsConfig, apnsStatus, type Env } from "./env";

const env = (vars: Partial<Env>): Env => vars as Env;
const key = { APNS_KEY_P8: "p8", APNS_KEY_ID: "KEYID" };

describe("APNs settings", () => {
  it("are off without the key secrets", () => {
    expect(apnsConfig(env({ APNS_TEAM_ID: "TEAM", APNS_TOPIC: "com.example.keron.ios" }))).toBeUndefined();
    expect(apnsStatus(env({}))).toEqual({ pushConfigured: false });
  });

  it("never fall back to another app's Apple identity", () => {
    expect(() => apnsConfig(env(key))).toThrow("APNS_TEAM_ID and APNS_TOPIC unset");
    expect(() => apnsConfig(env({ ...key, APNS_TEAM_ID: "TEAM" }))).toThrow("APNS_TOPIC unset");
    expect(apnsStatus(env({ ...key, APNS_TOPIC: "com.example.keron.ios" }))).toEqual({
      pushConfigured: false,
      pushError: "push is misconfigured: APNS_TEAM_ID unset (edge/wrangler.jsonc vars)"
    });
  });

  it("use the deployment's own team and topic", () => {
    const vars = { ...key, APNS_TEAM_ID: "TEAM", APNS_TOPIC: "com.example.keron.ios" };
    expect(apnsConfig(env(vars))).toEqual({ keyP8: "p8", keyId: "KEYID", teamId: "TEAM", topic: "com.example.keron.ios" });
    expect(apnsStatus(env(vars))).toEqual({ pushConfigured: true });
  });
});
