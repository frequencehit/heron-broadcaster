// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { t } from "../i18n";
import type { CertificationDelivery, DeliveryRun } from "../lib/certification";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent } from "../test/render";
import { Deliveries } from "./CertificationDeliveries";

const FAILED: DeliveryRun = {
  id: 12,
  day: "2026-11-07",
  manual: false,
  state: "failed",
  file_name: "main_20261107.log.gz",
  bytes: 2048,
  gaps: ["2026-11-07T12:00:00Z/2026-11-07T12:30:00Z"],
  error: "login refused: 530 bad password",
  created_at_unix_ms: Date.UTC(2026, 10, 8, 4),
  finished_at_unix_ms: Date.UTC(2026, 10, 8, 6),
  next_attempt_unix_ms: null,
  attempts: [1, 2, 3, 4, 5].map((n) => ({
    at_unix_ms: Date.UTC(2026, 10, 8, 4, n),
    ok: false,
    error: "login refused: 530 bad password",
    file_name: "main_20261107.log.gz",
    bytes: 2048,
  })),
};

const DELIVERED: DeliveryRun = {
  ...FAILED,
  id: 11,
  day: "2026-11-06",
  manual: true,
  state: "delivered",
  gaps: [],
  error: null,
  attempts: [{ at_unix_ms: Date.UTC(2026, 10, 7, 4), ok: true, error: null, file_name: "main_20261106.log.gz", bytes: 2048 }],
};

const DELIVERY: CertificationDelivery = {
  id: 4,
  name: "Certifier",
  enabled: true,
  server_id: 3,
  server_name: "Main",
  mount: null,
  protocol: "ftps",
  host: "ftp.example.org",
  port: 21,
  username: "station",
  password_set: true,
  remote_dir: "/incoming",
  file_name: "{station}_{yyyy}{mm}{dd}.{ext}",
  format: "raw",
  compression: "gzip",
  send_at: "04:00",
  days_back: 1,
  last: FAILED,
};

function page(answers: Record<string, unknown> = {}) {
  answer({
    "GET /api/v1/stations/{station_id}/certification/deliveries": [DELIVERY, { ...DELIVERY, id: 5, name: "Other", enabled: false, last: null }],
    "GET /api/v1/stations/{station_id}/stream-servers": [{ id: 3, name: "Main" }],
    "GET /api/v1/stations/{station_id}/settings/station": { timezone: "UTC" },
    "GET /api/v1/stations": [{ id: 1, code: "main", name: "Main" }],
    ...answers,
  });
  renderApp(<Deliveries />);
}

async function item(name: string) {
  return within((await screen.findByText(name)).closest("li") as HTMLElement);
}

describe("certification deliveries", () => {
  beforeEach(() => chooseStation(1));

  it("lists the deliveries with their schedule and last day sent", async () => {
    page();
    const certifier = await item("Certifier");
    expect(certifier.getByText("ftps://station@ftp.example.org:21/incoming", { exact: false })).toBeTruthy();
    expect(certifier.getByText(t("certification.schedule", { time: "04:00", day: t("certification.dayBefore") }))).toBeTruthy();
    expect(certifier.getByText(t("certification.state.failed"))).toBeTruthy();
    expect(certifier.getByText("login refused: 530 bad password", { exact: false })).toBeTruthy();
    const other = await item("Other");
    expect(other.getByText(t("certification.off"))).toBeTruthy();
    expect(other.getByText(t("certification.notSentYet"))).toBeTruthy();
  });

  it("shows the last 30 days sent with their attempts and gaps", async () => {
    page({ "GET /api/v1/stations/{station_id}/certification/deliveries/{id}/runs": [FAILED, DELIVERED] });
    fireEvent.click((await item("Certifier")).getByRole("button", { name: t("certification.log") }));
    const log = within(await screen.findByRole("table", { name: t("certification.log") }));
    const rows = log.getAllByRole("row");
    expect(rows).toHaveLength(3);
    const failed = within(rows[1] as HTMLElement);
    expect(failed.getByText("main_20261107.log.gz")).toBeTruthy();
    expect(failed.getByText("5")).toBeTruthy();
    expect(failed.getByText(t("certification.gapCount", { count: 1 })).getAttribute("title")).toBe(FAILED.gaps[0]);
    const delivered = within(rows[2] as HTMLElement);
    expect(delivered.getByText(t("certification.state.delivered"))).toBeTruthy();
    expect(delivered.getByText(t("certification.manual"), { exact: false })).toBeTruthy();
    expect(sent("GET /api/v1/stations/{station_id}/certification/deliveries/{id}/runs")[0]?.options).toEqual({
      params: { path: { station_id: 1, id: 4 } },
    });
  });

  it("sends a chosen day now", async () => {
    page({ "POST /api/v1/stations/{station_id}/certification/deliveries/{id}/send": { ...FAILED, day: "2026-11-01", state: "pending" } });
    const certifier = await item("Certifier");
    fireEvent.change(certifier.getByLabelText(t("certification.sendDay")), { target: { value: "2026-11-01" } });
    fireEvent.click(certifier.getByRole("button", { name: t("certification.sendNow") }));
    expect(await screen.findByText(t("certification.sending", { day: "2026-11-01" }))).toBeTruthy();
    expect(sent("POST /api/v1/stations/{station_id}/certification/deliveries/{id}/send")[0]?.options).toEqual({
      params: { path: { station_id: 1, id: 4 } },
      body: { day: "2026-11-01" },
    });
  });

  it("tests the connection and shows why it failed", async () => {
    page({
      "POST /api/v1/stations/{station_id}/certification/deliveries/{id}/test": { ok: false, error: "TLS refused by ftp.example.org:21" },
    });
    fireEvent.click((await item("Certifier")).getByRole("button", { name: t("certification.test") }));
    expect((await screen.findByRole("status")).textContent).toBe(
      t("certification.testFailed", { error: "TLS refused by ftp.example.org:21" }),
    );
  });

  it("adds a delivery, with a preview of its file name", async () => {
    page({ "POST /api/v1/stations/{station_id}/certification/deliveries": DELIVERY });
    fireEvent.click(await screen.findByRole("button", { name: t("certification.addDelivery") }));
    const form = within(screen.getByRole("form", { name: t("certification.deliveryForm") }));
    fireEvent.change(form.getByLabelText(t("certification.deliveryName"), { exact: false }), { target: { value: "Certifier" } });
    fireEvent.change(form.getByLabelText(t("certification.host"), { exact: false }), { target: { value: "ftp.example.org" } });
    fireEvent.change(form.getByLabelText(t("certification.username")), { target: { value: "station" } });
    fireEvent.change(form.getByLabelText(t("certification.password")), { target: { value: "secret" } });
    fireEvent.change(form.getByLabelText(t("certification.fileName"), { exact: false }), { target: { value: "{nope}.log" } });
    expect(form.getByRole("button", { name: t("common.save") })).toHaveProperty("disabled", true);
    fireEvent.change(form.getByLabelText(t("certification.fileName"), { exact: false }), {
      target: { value: "{station}_{mount}.{ext}" },
    });
    expect(await form.findByText(/Gives main_all\.log\./)).toBeTruthy();
    fireEvent.click(form.getByRole("button", { name: t("common.save") }));
    await waitFor(() => expect(sent("POST /api/v1/stations/{station_id}/certification/deliveries")).toHaveLength(1));
    expect(sent("POST /api/v1/stations/{station_id}/certification/deliveries")[0]?.options).toMatchObject({
      body: {
        name: "Certifier",
        server_id: 3,
        protocol: "ftps",
        host: "ftp.example.org",
        port: 21,
        username: "station",
        password: "secret",
        file_name: "{station}_{mount}.{ext}",
        send_at: "04:00",
        days_back: 1,
      },
    });
  });

  it("keeps the password left empty when editing", async () => {
    page({ "PUT /api/v1/stations/{station_id}/certification/deliveries/{id}": DELIVERY });
    fireEvent.click((await item("Certifier")).getByRole("button", { name: t("certification.edit") }));
    const form = within(screen.getByRole("form", { name: t("certification.deliveryForm") }));
    expect(form.getByText(t("certification.passwordKept"))).toBeTruthy();
    fireEvent.click(form.getByRole("button", { name: t("common.save") }));
    await waitFor(() => expect(sent("PUT /api/v1/stations/{station_id}/certification/deliveries/{id}")).toHaveLength(1));
    const body = (sent("PUT /api/v1/stations/{station_id}/certification/deliveries/{id}")[0]?.options as { body: object }).body;
    expect("password" in body).toBe(false);
    expect(body).toMatchObject({ compression: "gzip", remote_dir: "/incoming" });
  });

  it("deletes a delivery once asked", async () => {
    page({ "DELETE /api/v1/stations/{station_id}/certification/deliveries/{id}": undefined });
    fireEvent.click((await item("Certifier")).getByRole("button", { name: t("certification.deleteDelivery") }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: t("certification.deleteDelivery") }));
    expect(await screen.findByText(t("certification.deliveryDeleted"))).toBeTruthy();
  });
});
