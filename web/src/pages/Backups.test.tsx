// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { BackupKeyView, BackupTests, BackupView, RestoreReport } from "../api";
import { t } from "../i18n";
import { bytes } from "../lib/format";
import { EARLIER_RECIPIENT, KEY, RECIPIENT } from "../test/backups";
import { Failure, answer, renderApp, sent, user } from "../test/render";
import { Backups } from "./Backups";

const DAY = 86_400_000;
const NOW = Date.UTC(2026, 10, 3, 3, 30);

function backup(name: string, changes: Partial<BackupView> = {}): BackupView {
  return {
    name,
    kind: "daily",
    created_at_unix_ms: NOW,
    config_size: 2048,
    database_size: 4096,
    encrypted: true,
    key: "current",
    recipient: RECIPIENT,
    ...changes,
  };
}

const CURRENT = backup("2026-11-03T03-30-00Z-daily");
const EARLIER = backup("2026-10-01T03-30-00Z-manual", {
  kind: "manual",
  created_at_unix_ms: NOW - 33 * DAY,
  key: "earlier",
  recipient: EARLIER_RECIPIENT,
  damaged: "heron-database-2026-10-01T03-30-00Z-manual.sql.gz.age: does not match its checksum",
});
const UPLOADED = backup("2026-10-02T10-00-00Z-uploaded", { kind: "uploaded", key: "unknown", recipient: null });
const IN_CLEAR = backup("2026-09-01T03-30-00Z-before_restore", {
  kind: "before_restore",
  encrypted: false,
  key: null,
  recipient: null,
  database_size: null,
  database_error: "mariadb-dump failed",
});
const NO_FILES = backup("2026-08-01T03-30-00Z-daily", {
  encrypted: false,
  key: null,
  config_size: 0,
  database_size: null,
});

const ON: BackupKeyView = {
  state: "on",
  current: { recipient: RECIPIENT, since_unix_ms: NOW - DAY, set_by: "grace" },
  previous: [{ recipient: EARLIER_RECIPIENT, since_unix_ms: NOW - 60 * DAY, until_unix_ms: NOW - DAY, set_by: "ada" }],
  pending_recipient: null,
};

const REPORT: RestoreReport = {
  before_restore: "2026-11-03T12-00-00Z-before_restore",
  restored: { stations: 1, categories: 12, clocks: 0 },
  left_out: { track_categories: 3 },
  missing_tracks: ["Music/gone.flac"],
  missing_tracks_total: 1,
  missing_artists: ["Nobody"],
  missing_artists_total: 1,
};

/** The page's requests, answered for a station whose backups are encrypted. */
function station(answers: Record<string, unknown> = {}) {
  answer({
    "GET /api/v1/backups": [CURRENT, EARLIER, UPLOADED, IN_CLEAR, NO_FILES],
    "GET /api/v1/backups/key": ON,
    "GET /api/v1/backups/tests": { testing: null, last: null, last_ok: null } satisfies BackupTests,
    "GET /api/v1/settings/backups": { enabled: true, time: "03:30", keep_days: 14, database: true },
    ...answers,
  });
  renderApp(<Backups />);
}

/** The row of a backup in the list. */
async function row(item: BackupView) {
  return within((await screen.findByTitle(item.name)).closest("tr") as HTMLElement);
}

function dialog() {
  return within(screen.getByRole("dialog"));
}

function keyField() {
  return screen.getByPlaceholderText("AGE-SECRET-KEY-1…");
}

describe("Backups", () => {
  it("is for admins only", () => {
    answer({});
    renderApp(<Backups />, user("operator"));
    expect(screen.getByText(t("backups.adminsOnly"))).toBeTruthy();
  });

  it("lists backups with their kind, files and key", async () => {
    station();
    const current = await row(CURRENT);
    expect(current.getByText(t("backups.encrypted"))).toBeTruthy();
    expect(current.getByRole("link", { name: bytes(2048) }).getAttribute("href")).toBe(
      `/api/v1/backups/${encodeURIComponent(CURRENT.name)}/config`,
    );
    const earlier = await row(EARLIER);
    expect(earlier.getByText(t("backups.earlierKey"))).toBeTruthy();
    expect(earlier.getByTitle(EARLIER.damaged as string).textContent).toBe(t("backups.damaged"));
    expect((await row(UPLOADED)).getByText(t("backups.unknownKey"))).toBeTruthy();
    const inClear = await row(IN_CLEAR);
    expect(inClear.getByText(t("backups.inClear"))).toBeTruthy();
    expect(inClear.getByText(t("backups.databaseFailed"))).toBeTruthy();
    expect(inClear.getByText(t("backups.kind.before_restore"))).toBeTruthy();
    expect((await row(NO_FILES)).getAllByText("–")).toHaveLength(2);
  });

  it("says when there is no backup yet", async () => {
    station({ "GET /api/v1/backups": [] });
    expect(await screen.findByText(t("backups.none"))).toBeTruthy();
  });

  it("shows why the list cannot be read", async () => {
    station({ "GET /api/v1/backups": new Failure(500, "conflict", "the folder cannot be read") });
    expect(await screen.findByText("the folder cannot be read")).toBeTruthy();
  });

  it("backs up now", async () => {
    station({ "POST /api/v1/backups": backup("2026-11-03T12-00-00Z-manual", { kind: "manual" }) });
    fireEvent.click(screen.getByRole("button", { name: t("backups.create") }));
    expect(await screen.findByText(t("backups.created", { name: "2026-11-03T12-00-00Z-manual" }))).toBeTruthy();
  });

  it("deletes a backup once asked", async () => {
    station({ "DELETE /api/v1/backups/{name}": undefined });
    fireEvent.click((await row(CURRENT)).getByRole("button", { name: t("backups.delete") }));
    fireEvent.click(dialog().getByRole("button", { name: t("categories.deleteButton") }));
    expect(await screen.findByText(t("backups.deleted"))).toBeTruthy();
    expect(sent("DELETE /api/v1/backups/{name}")[0]?.options).toEqual({ params: { path: { name: CURRENT.name } } });
  });
});

describe("restoring", () => {
  it("restores an encrypted backup with the key of its public key, then reports", async () => {
    station({ "POST /api/v1/backups/{name}/restore": REPORT });
    fireEvent.click((await row(CURRENT)).getByRole("button", { name: t("backups.restore") }));
    expect(dialog().getByText(t("backups.restoreKey"))).toBeTruthy();
    expect(dialog().getByText(RECIPIENT)).toBeTruthy();
    const restore = dialog().getByRole("button", { name: t("backups.restore") });
    expect(restore).toHaveProperty("disabled", true);
    fireEvent.change(keyField(), { target: { value: KEY } });
    fireEvent.click(restore);
    expect(await screen.findByText(t("backups.restored"))).toBeTruthy();
    expect(sent("POST /api/v1/backups/{name}/restore")[0]?.options).toEqual({
      params: { path: { name: CURRENT.name } },
      body: { key: KEY },
    });
    const report = dialog();
    expect(report.getByText(/3 items were left out/)).toBeTruthy();
    expect(report.getByText("Music/gone.flac")).toBeTruthy();
    expect(report.getByText(/Artists not found: Nobody/)).toBeTruthy();
    expect(report.getByText("categories")).toBeTruthy();
    expect(report.queryByText("clocks")).toBeNull();
  });

  it("asks for the earlier key of an older backup, and shows a wrong one", async () => {
    station({ "POST /api/v1/backups/{name}/restore": new Failure(400, "invalid", "this is not its key") });
    fireEvent.click((await row(EARLIER)).getByRole("button", { name: t("backups.restore") }));
    expect(dialog().getByText(t("backups.restoreEarlierKey"))).toBeTruthy();
    expect(dialog().getByText(EARLIER_RECIPIENT)).toBeTruthy();
    fireEvent.change(keyField(), { target: { value: KEY } });
    fireEvent.click(dialog().getByRole("button", { name: t("backups.restore") }));
    expect(await screen.findByText("this is not its key")).toBeTruthy();
  });

  it("restores a backup in clear without a key, and cancels", async () => {
    station({ "POST /api/v1/backups/{name}/restore": REPORT });
    fireEvent.click((await row(IN_CLEAR)).getByRole("button", { name: t("backups.restore") }));
    expect(screen.queryByPlaceholderText("AGE-SECRET-KEY-1…")).toBeNull();
    fireEvent.click(dialog().getByRole("button", { name: t("ui.cancel") }));
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click((await row(IN_CLEAR)).getByRole("button", { name: t("backups.restore") }));
    fireEvent.click(dialog().getByRole("button", { name: t("backups.restore") }));
    await screen.findByText(t("backups.restored"));
    expect(sent("POST /api/v1/backups/{name}/restore")[0]?.options).toMatchObject({ body: {} });
  });

  it("restores an encrypted file with its key in a header", async () => {
    station({ "POST /api/v1/backups/restore": { ...REPORT, left_out: {}, restored: {} } });
    const file = new File(["age-encryption.org/v1\n-> X25519 abc\n"], "heron-config.json.age");
    const input = document.querySelector<HTMLInputElement>('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [file] } });
    expect(await screen.findByText(t("backupKey.noPublicKey"))).toBeTruthy();
    expect(dialog().getByText(/Restore the file heron-config\.json\.age/)).toBeTruthy();
    fireEvent.change(keyField(), { target: { value: KEY } });
    fireEvent.click(dialog().getByRole("button", { name: t("backups.restore") }));
    await screen.findByText(t("backups.restored"));
    expect(sent("POST /api/v1/backups/restore")[0]?.options).toMatchObject({
      params: { header: { "X-Heron-Backup-Key": KEY } },
      headers: { "Content-Type": "application/octet-stream" },
    });
  });

  it("restores a file in clear without a key", async () => {
    station({ "POST /api/v1/backups/restore": REPORT });
    const input = document.querySelector<HTMLInputElement>('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [new File(["{}"], "heron-config.json")] } });
    await screen.findByText(/Restore the file heron-config\.json\?/);
    expect(screen.queryByPlaceholderText("AGE-SECRET-KEY-1…")).toBeNull();
    fireEvent.click(dialog().getByRole("button", { name: t("backups.restore") }));
    await screen.findByText(t("backups.restored"));
    expect(sent("POST /api/v1/backups/restore")[0]?.options).toMatchObject({ params: { header: {} } });
  });
});

describe("testing", () => {
  it("tests a backup in clear at once", async () => {
    station({ "POST /api/v1/backups/{name}/test": { testing: IN_CLEAR.name } });
    fireEvent.click((await row(IN_CLEAR)).getByRole("button", { name: t("backups.test") }));
    expect(await screen.findByText(t("backups.testStarted"))).toBeTruthy();
    expect(sent("POST /api/v1/backups/{name}/test")[0]?.options).toEqual({ params: { path: { name: IN_CLEAR.name } } });
  });

  it("tests an encrypted backup with its key", async () => {
    station({ "POST /api/v1/backups/{name}/test": { testing: EARLIER.name } });
    fireEvent.click((await row(EARLIER)).getByRole("button", { name: t("backups.test") }));
    expect(dialog().getByText(t("backups.restoreEarlierKey"))).toBeTruthy();
    expect(dialog().getByText(EARLIER_RECIPIENT)).toBeTruthy();
    fireEvent.change(keyField(), { target: { value: KEY } });
    fireEvent.click(dialog().getByRole("button", { name: t("backups.test") }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(sent("POST /api/v1/backups/{name}/test")[0]?.options).toEqual({
      params: { path: { name: EARLIER.name } },
      body: { key: KEY },
    });
  });

  it("closes the test of a backup without testing it", async () => {
    station();
    fireEvent.click((await row(CURRENT)).getByRole("button", { name: t("backups.test") }));
    expect(dialog().queryByText(t("backups.restoreEarlierKey"))).toBeNull();
    fireEvent.click(dialog().getByRole("button", { name: t("ui.cancel") }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows the test under way", async () => {
    station({ "GET /api/v1/backups/tests": { testing: CURRENT.name, last: null, last_ok: null } });
    expect(await screen.findByText(/Reading back the backup of/)).toBeTruthy();
  });

  it("shows the last test that passed", async () => {
    station({
      "GET /api/v1/backups/tests": {
        last: { name: CURRENT.name, at_unix_ms: NOW, ok: true, by: "ada" },
        last_ok: { name: CURRENT.name, at_unix_ms: NOW, ok: true, by: "ada" },
      },
    });
    expect(await screen.findByText(/was read back whole on .* \(ada\)/)).toBeTruthy();
  });

  it("shows a failed test, and the last one that passed", async () => {
    station({
      "GET /api/v1/backups/tests": {
        last: { name: "upload.json", at_unix_ms: NOW, ok: false, by: null, error: "it is cut short" },
        last_ok: { name: CURRENT.name, at_unix_ms: NOW - DAY, ok: true, by: null },
      },
    });
    expect(await screen.findByText(/upload\.json failed its test .* \(by Heron\): it is cut short/)).toBeTruthy();
    expect(screen.getByText(/was read back whole/)).toBeTruthy();
  });

  it("says when no backup was ever tested", async () => {
    station();
    expect(await screen.findByText(t("backups.neverTested"))).toBeTruthy();
  });
});

describe("encryption", () => {
  it("shows the key, who set it and the earlier ones", async () => {
    station();
    expect(await screen.findByTitle(RECIPIENT)).toBeTruthy();
    expect(screen.getByText(/by grace/)).toBeTruthy();
    expect(screen.getByText(/1 earlier key/)).toBeTruthy();
  });

  it("checks the key", async () => {
    station();
    fireEvent.click(await screen.findByRole("button", { name: t("backups.checkKey") }));
    expect(dialog().getByText(t("backupKey.checkHint"))).toBeTruthy();
    fireEvent.click(dialog().getByRole("button", { name: t("ui.cancel") }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("turns encryption off once asked", async () => {
    station({ "DELETE /api/v1/backups/key": ON });
    fireEvent.click(await screen.findByRole("button", { name: t("backups.turnOff") }));
    fireEvent.click(dialog().getByRole("button", { name: t("backups.turnOff") }));
    await waitFor(() => expect(sent("DELETE /api/v1/backups/key")).toHaveLength(1));
  });

  it("generates a key, shows it once, and turns encryption on when it is typed back", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const createObjectURL = vi.fn(() => "blob:key");
    Object.assign(URL, { createObjectURL, revokeObjectURL: vi.fn() });
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    station({
      "GET /api/v1/backups/key": { state: "off", previous: [] },
      "POST /api/v1/backups/key": { key: KEY, key_file: `# public key: ${RECIPIENT}\n${KEY}\n`, recipient: RECIPIENT },
      "POST /api/v1/backups/key/confirm": {
        key: ON,
        encrypted: [CURRENT.name, EARLIER.name],
        left_in_clear: [NO_FILES.name],
        backup: null,
      },
    });
    expect(await screen.findByText(t("backups.encryptionOff"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("backups.encrypt") }));
    fireEvent.click(dialog().getByRole("button", { name: t("backups.next") }));
    expect(await screen.findByDisplayValue(KEY)).toBeTruthy();
    expect(sent("POST /api/v1/backups/key")[0]?.options).toEqual({ body: {} });
    expect(dialog().getByText(RECIPIENT)).toBeTruthy();
    fireEvent.click(dialog().getByRole("button", { name: t("backups.copyKey") }));
    expect(await screen.findByText(t("backups.keyCopied"))).toBeTruthy();
    expect(writeText).toHaveBeenCalledWith(KEY);
    fireEvent.click(dialog().getByRole("button", { name: t("backups.downloadKey") }));
    expect(click).toHaveBeenCalled();
    fireEvent.click(dialog().getByRole("button", { name: t("backups.savedKey") }));
    expect(dialog().getByText(t("backups.typeBackHint"))).toBeTruthy();
    expect(dialog().getByText(RECIPIENT)).toBeTruthy();
    fireEvent.change(keyField(), { target: { value: KEY } });
    fireEvent.click(dialog().getByRole("button", { name: t("backups.turnOn") }));
    expect(await screen.findByText(t("backups.copiesInClear"))).toBeTruthy();
    expect(sent("POST /api/v1/backups/key/confirm")[0]?.options).toEqual({ body: { key: KEY } });
    expect(dialog().getByText(new RegExp(NO_FILES.name))).toBeTruthy();
    fireEvent.click(dialog().getAllByRole("button", { name: t("ui.close") }).at(-1) as HTMLElement);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says when the key cannot be copied", async () => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn().mockRejectedValue(new Error("denied")) },
      configurable: true,
    });
    station({
      "GET /api/v1/backups/key": { state: "off", previous: [] },
      "POST /api/v1/backups/key": { key: KEY, key_file: null, recipient: RECIPIENT },
    });
    fireEvent.click(await screen.findByRole("button", { name: t("backups.encrypt") }));
    fireEvent.click(dialog().getByRole("button", { name: t("backups.next") }));
    await screen.findByDisplayValue(KEY);
    expect(dialog().queryByRole("button", { name: t("backups.downloadKey") })).toBeNull();
    fireEvent.click(dialog().getByRole("button", { name: t("backups.copyKey") }));
    expect(await screen.findByText(t("backups.keyCopyFailed"))).toBeTruthy();
  });

  it("takes an admin's own public key, then asks for its key", async () => {
    station({
      "POST /api/v1/backups/key": ({ body }: { body: { recipient?: string } }) =>
        // heron-api trims the key, as typed with spaces around it.
        body.recipient?.trim() === RECIPIENT
          ? { key: null, key_file: null, recipient: RECIPIENT }
          : new Failure(400, "invalid", "not an age public key"),
    });
    fireEvent.click(await screen.findByRole("button", { name: t("backups.changeKey") }));
    fireEvent.click(dialog().getByLabelText(t("backups.ownKey")));
    const next = dialog().getByRole("button", { name: t("backups.next") });
    expect(next).toHaveProperty("disabled", true);
    fireEvent.change(dialog().getByPlaceholderText("age1…"), { target: { value: `${RECIPIENT} ` } });
    fireEvent.click(next);
    expect(await screen.findByText(t("backups.typeBackHint"))).toBeTruthy();
    expect(dialog().getByText(RECIPIENT)).toBeTruthy();
  });

  it("shows why a key is refused", async () => {
    station({ "POST /api/v1/backups/key": new Failure(409, "conflict", "backups are already encrypted with this key") });
    fireEvent.click(await screen.findByRole("button", { name: t("backups.changeKey") }));
    fireEvent.click(dialog().getByLabelText(t("backups.ownKey")));
    fireEvent.click(dialog().getByLabelText(t("backups.generate")));
    fireEvent.click(dialog().getByRole("button", { name: t("backups.next") }));
    expect(await screen.findByText("backups are already encrypted with this key")).toBeTruthy();
  });

  it("types back a key that waits, against its public key, or drops it", async () => {
    station({
      "GET /api/v1/backups/key": { state: "pending", previous: [], pending_recipient: RECIPIENT },
      "DELETE /api/v1/backups/key": { state: "off", previous: [] },
    });
    expect(await screen.findByText(t("backups.keyPending"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("backups.typeBack") }));
    expect(dialog().getByText(RECIPIENT)).toBeTruthy();
    fireEvent.click(dialog().getByRole("button", { name: t("ui.close") }));
    fireEvent.click(screen.getByRole("button", { name: t("backups.dropKey") }));
    await waitFor(() => expect(sent("DELETE /api/v1/backups/key")).toHaveLength(1));
  });

  it("shows why the key state cannot be read", async () => {
    station({ "GET /api/v1/backups/key": new Failure(500, "conflict", "the key cannot be read") });
    expect(await screen.findByText("the key cannot be read")).toBeTruthy();
  });
});

describe("daily backup settings", () => {
  it("saves what changed", async () => {
    station({ "PUT /api/v1/settings/backups": { enabled: false, time: "04:00", keep_days: 7, database: false } });
    const save = screen.getByRole("button", { name: t("common.save") });
    const time = await screen.findByDisplayValue("03:30");
    expect(save).toHaveProperty("disabled", true);
    fireEvent.change(time, { target: { value: "04:00" } });
    fireEvent.change(screen.getByDisplayValue("14"), { target: { value: "7" } });
    fireEvent.click(screen.getByLabelText(t("backups.withDatabase")));
    fireEvent.click(screen.getByLabelText(t("backups.daily")));
    expect(time).toHaveProperty("disabled", true);
    fireEvent.click(save);
    expect(await screen.findByText(t("categories.saved"))).toBeTruthy();
    expect(sent("PUT /api/v1/settings/backups")[0]?.options).toEqual({
      body: { enabled: false, time: "04:00", keep_days: 7, database: false },
    });
  });

  it("shows why the settings cannot be read", async () => {
    station({ "GET /api/v1/settings/backups": new Failure(500, "conflict", "the settings cannot be read") });
    expect(await screen.findByText("the settings cannot be read")).toBeTruthy();
  });
});
