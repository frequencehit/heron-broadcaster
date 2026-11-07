// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { OutputView, Schemas } from "../api";
import { t } from "../i18n";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent } from "../test/render";
import { Outputs } from "./Outputs";

type StreamServerView = Schemas["StreamServerView"];

const MASTER: StreamServerView = {
  id: 1,
  name: "Main server",
  kind: "icecast",
  host: "icecast",
  port: 8000,
  tls: false,
  source_username: null,
  source_password_set: true,
  admin_username: null,
  admin_password_set: true,
  public_url: null,
  relay_of: null,
  relay_mounts: null,
  relay_subtract_one: false,
  outputs: [{ id: 5, name: "MP3" }],
  relays: [{ id: 2, name: "Relay" }],
};
const RELAY: StreamServerView = {
  ...MASTER,
  id: 2,
  name: "Relay",
  host: "relay",
  tls: true,
  port: 443,
  source_password_set: false,
  admin_password_set: false,
  relay_of: 1,
  relay_mounts: [{ mount: "/live" }],
  outputs: [],
  relays: [],
};
const OUTPUT: OutputView = {
  id: 5,
  name: "MP3",
  kind: "icecast",
  server_id: 1,
  encoder_profile_id: 1,
  config: { mount: "/live" } as never,
  details: { name: "Station HD" },
  secrets_set: [],
  metadata_enabled: true,
  enabled: true,
  listen_url: "http://icecast:8000/live",
  status: null,
};

function show() {
  chooseStation(1);
  answer({
    "GET /api/v1/stations/{id}/modules": { on: { medleys: true, cover_art: true, listener_statistics: true, webhooks: true } },
    "GET /api/v1/stations/{station_id}/outputs": [OUTPUT],
    "GET /api/v1/stations/{station_id}/stream-servers": [MASTER, RELAY],
    "GET /api/v1/stations/{station_id}/settings/stream-details": {
      name: "Station",
      description: null,
      genre: "Pop",
      url: null,
      public: true,
    },
    "GET /api/v1/encoder-profiles": [
      { id: 1, name: "MP3 128k", encoder: "mp3", bitrate_kbps: 128, sample_rate: 44100, channels: 2, options: {}, outputs: 1 },
    ],
    "GET /api/v1/stations/{station_id}/listeners/now": {
      outputs: [],
      total: null,
      servers: [],
    },
    "POST /api/v1/stations/{station_id}/outputs/{id}/duplicate": { ...OUTPUT, id: 6, enabled: false },
    "PUT /api/v1/stations/{station_id}/outputs/{id}": OUTPUT,
  });
  renderApp(<Outputs />);
}

describe("Outputs", () => {
  it("lists the outputs with their server and the servers with their relays", async () => {
    show();
    // The output's mount, and the mount its relay repeats.
    const [mount] = await screen.findAllByText("/live");
    const row = mount?.closest("tr");
    expect(row && within(row).getByText("Main server")).toBeTruthy();
    expect(within(row as HTMLElement).getByText("MP3 128k")).toBeTruthy();
    expect(screen.getByText(t("outputs.relayOf", { name: "Main server" }))).toBeTruthy();
    expect(screen.getByText("https://relay:443")).toBeTruthy();
    // A server in use cannot be deleted.
    expect(screen.getAllByTitle(t("outputs.serverUsed")).length).toBe(1);
  });

  it("duplicates an output", async () => {
    show();
    fireEvent.click(await screen.findByRole("button", { name: t("outputs.duplicate") }));
    await waitFor(() => expect(sent("POST /api/v1/stations/{station_id}/outputs/{id}/duplicate")).toHaveLength(1));
  });

  it("keeps the server and the overrides when an output is turned off", async () => {
    show();
    fireEvent.click(await screen.findByRole("button", { name: t("track.disable") }));
    await waitFor(() => expect(sent("PUT /api/v1/stations/{station_id}/outputs/{id}")).toHaveLength(1));
    expect(sent("PUT /api/v1/stations/{station_id}/outputs/{id}")[0]?.options).toMatchObject({
      body: { server_id: 1, config: { mount: "/live" }, details: { name: "Station HD" }, enabled: false },
    });
  });

  it("edits an output's details over the station's", async () => {
    show();
    fireEvent.click(await screen.findByRole("button", { name: t("categories.edit", { name: "MP3" }) }));
    const name = await screen.findByDisplayValue("Station HD");
    expect(screen.getByPlaceholderText("Pop")).toBeTruthy();
    fireEvent.change(name, { target: { value: "" } });
    fireEvent.change(screen.getByPlaceholderText("Pop"), { target: { value: "Hits" } });
    fireEvent.click(screen.getByRole("button", { name: t("common.save") }));
    await waitFor(() => expect(sent("PUT /api/v1/stations/{station_id}/outputs/{id}").length).toBeGreaterThan(0));
    expect(sent("PUT /api/v1/stations/{station_id}/outputs/{id}").at(-1)?.options).toMatchObject({
      body: { server_id: 1, config: { mount: "/live" }, details: { genre: "Hits" } },
    });
  });
});
