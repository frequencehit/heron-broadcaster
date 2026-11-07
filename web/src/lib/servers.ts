// SPDX-License-Identifier: AGPL-3.0-or-later

// Stream servers and the station details outputs override (see
// docs/outputs.md#stream-servers).

import type { Schemas } from "../api";

type RelayMount = Schemas["RelayMount"];
type Overrides = Schemas["DetailOverrides"];

/** A relay's mounts as lines: `/live`, or `/live /relay-live` when renamed. */
export function relayMountsText(mounts: RelayMount[] | null | undefined): string {
  return (mounts ?? []).map((m) => (m.relay_mount ? `${m.mount} ${m.relay_mount}` : m.mount)).join("\n");
}

/** The lines of a relay's mounts; none (all of the master's) when empty. */
export function parseRelayMounts(text: string): RelayMount[] | null {
  const mounts = text
    .split("\n")
    .map((line) => line.trim().split(/\s+/).filter(Boolean))
    .filter((parts) => parts.length > 0)
    .map(([mount = "", relay]): RelayMount => (relay && relay !== mount ? { mount, relay_mount: relay } : { mount }));
  return mounts.length > 0 ? mounts : null;
}

/** `host:port`, with brackets around an IPv6 address. */
export function serverAddress(server: Pick<Schemas["StreamServerView"], "host" | "port" | "tls">): string {
  const host = server.host.includes(":") ? `[${server.host}]` : server.host;
  return `${server.tls ? "https://" : ""}${host}:${server.port}`;
}

export type DetailTexts = Record<"name" | "description" | "genre" | "url", string>;

/** What an output overrides: the texts typed (empty follows the station), and `public` when chosen. */
export function overridesFrom(texts: DetailTexts, listed: boolean | null): Overrides {
  const overrides: Overrides = {};
  for (const key of ["name", "description", "genre", "url"] as const) {
    const value = texts[key].trim();
    if (value !== "") {
      overrides[key] = value;
    }
  }
  if (listed !== null) {
    overrides.public = listed;
  }
  return overrides;
}
