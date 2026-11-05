// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { BackupKeyView, BackupState } from "../api";
import { t } from "../i18n";
import { KEY, RECIPIENT } from "../test/backups";
import { Failure, answer, renderApp, sent } from "../test/render";
import { BackupWarnings, KeyCheckDialog, KeyInput } from "./BackupKey";

const ON: BackupKeyView = {
  state: "on",
  current: { recipient: RECIPIENT, since_unix_ms: Date.UTC(2026, 10, 1), set_by: "grace" },
  previous: [],
};

function typeKey(key = KEY) {
  fireEvent.change(screen.getByPlaceholderText("AGE-SECRET-KEY-1…"), { target: { value: key } });
}

describe("KeyInput", () => {
  it("names the public key the key must match, when it is known", () => {
    renderApp(<KeyInput value="" onChange={() => {}} recipient={RECIPIENT} />);
    expect(screen.getByText(RECIPIENT)).toBeTruthy();
  });

  it("says nothing of a public key it does not know", () => {
    renderApp(<KeyInput value="" onChange={() => {}} recipient={null} />);
    expect(screen.queryByText(/age1/)).toBeNull();
  });
});

describe("KeyCheckDialog", () => {
  it("checks the key, then closes with the next check", async () => {
    answer({
      "GET /api/v1/backups/key": ON,
      "POST /api/v1/backups/key/check": { next_check_unix_ms: Date.UTC(2026, 10, 4), testing: null },
    });
    const onClose = vi.fn();
    renderApp(<KeyCheckDialog reminder={false} onClose={onClose} />);
    expect(await screen.findByText(RECIPIENT)).toBeTruthy();
    const check = screen.getByRole("button", { name: t("backupKey.check") });
    expect(check).toHaveProperty("disabled", true);
    typeKey();
    expect(check).toHaveProperty("disabled", false);
    fireEvent.click(check);
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(sent("POST /api/v1/backups/key/check")[0]?.options).toEqual({ body: { key: KEY } });
    expect(await screen.findByText(/^Key checked\. Next check/)).toBeTruthy();
  });

  it("says when the newest backup is being read back", async () => {
    answer({
      "GET /api/v1/backups/key": ON,
      "POST /api/v1/backups/key/check": { next_check_unix_ms: Date.UTC(2026, 10, 4), testing: "x" },
    });
    renderApp(<KeyCheckDialog reminder={false} onClose={() => {}} />);
    typeKey();
    fireEvent.submit(screen.getByPlaceholderText("AGE-SECRET-KEY-1…").closest("form") as HTMLFormElement);
    expect(await screen.findByText(/newest backup is being read back/)).toBeTruthy();
  });

  it("shows a wrong key and stays open", async () => {
    answer({
      "GET /api/v1/backups/key": ON,
      "POST /api/v1/backups/key/check": new Failure(400, "invalid", "this is not the backup key"),
    });
    const onClose = vi.fn();
    renderApp(<KeyCheckDialog reminder={false} onClose={onClose} />);
    typeKey();
    fireEvent.click(screen.getByRole("button", { name: t("backupKey.check") }));
    expect(await screen.findByText("this is not the backup key")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("closes on cancel without asking anything", () => {
    answer({ "GET /api/v1/backups/key": ON });
    const onClose = vi.fn();
    renderApp(<KeyCheckDialog reminder={false} onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: t("ui.cancel") }));
    expect(onClose).toHaveBeenCalled();
    expect(sent("POST /api/v1/backups/key/later")).toHaveLength(0);
  });

  it("as a reminder, waits a day on Later", async () => {
    answer({ "GET /api/v1/backups/key": ON, "POST /api/v1/backups/key/later": undefined });
    const onClose = vi.fn();
    renderApp(<KeyCheckDialog reminder onClose={onClose} />);
    expect(screen.getByText(t("backupKey.reminder"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("backupKey.later") }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(sent("POST /api/v1/backups/key/later")).toHaveLength(1);
  });

  it("as a reminder, closes even when Later fails", async () => {
    answer({
      "GET /api/v1/backups/key": ON,
      "POST /api/v1/backups/key/later": new Failure(500, "internal", "no"),
    });
    const onClose = vi.fn();
    renderApp(<KeyCheckDialog reminder onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: t("ui.close") }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(await screen.findByText(t("error.internal"))).toBeTruthy();
  });

  it("tells an admin without the key who set it", async () => {
    answer({ "GET /api/v1/backups/key": ON });
    renderApp(<KeyCheckDialog reminder onClose={() => {}} />);
    await screen.findByText(RECIPIENT);
    fireEvent.click(screen.getByRole("button", { name: t("backupKey.dontHave") }));
    expect(screen.getByText("grace")).toBeTruthy();
    expect(screen.getByRole("link", { name: t("nav.backups") })).toBeTruthy();
  });

  it("says when there is no key any more", async () => {
    answer({ "GET /api/v1/backups/key": { state: "off", previous: [] } });
    renderApp(<KeyCheckDialog reminder onClose={() => {}} />);
    await waitFor(() => expect(sent("GET /api/v1/backups/key")).toHaveLength(1));
    fireEvent.click(screen.getByRole("button", { name: t("backupKey.dontHave") }));
    expect(await screen.findByText(t("backupKey.missingNoKey"))).toBeTruthy();
  });
});

describe("BackupWarnings", () => {
  const state = (changes: Partial<BackupState>): BackupState => ({
    encryption: "on",
    check_due: false,
    damaged: 0,
    test_due: false,
    ...changes,
  });

  it("shows nothing when all is well", async () => {
    answer({ "GET /api/v1/backups/state": state({}) });
    const { container } = renderApp(<BackupWarnings />);
    await waitFor(() => expect(sent("GET /api/v1/backups/state")).toHaveLength(1));
    expect(container.textContent).toBe("");
  });

  it("shows a banner for each problem, with a link to the backups", async () => {
    answer({ "GET /api/v1/backups/state": state({ encryption: "off", damaged: 2, test_due: true }) });
    renderApp(<BackupWarnings />);
    expect(await screen.findByText(/Backups are not encrypted/)).toBeTruthy();
    expect(screen.getByText(/No backup has been read back/)).toBeTruthy();
    expect(screen.getAllByRole("link", { name: t("nav.backups") })).toHaveLength(3);
  });

  it("asks for the key when a check is due, once", async () => {
    answer({
      "GET /api/v1/backups/state": state({ check_due: true }),
      "GET /api/v1/backups/key": ON,
      "POST /api/v1/backups/key/later": undefined,
    });
    renderApp(<BackupWarnings />);
    expect(await screen.findByText(t("backupKey.reminder"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("backupKey.later") }));
    await waitFor(() => expect(screen.queryByText(t("backupKey.reminder"))).toBeNull());
  });
});
