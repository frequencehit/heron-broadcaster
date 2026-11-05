// SPDX-License-Identifier: AGPL-3.0-or-later

// The backup key: typed for a check, a restore or a test, and the backup
// banners of every page (docs/backups.md#encryption).

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { Link } from "../lib/router";
import { api, done, errorMessage, unwrap } from "../api";
import { rich, t } from "../i18n";
import { type BackupWarning, backupWarnings, looksLikeKey } from "../lib/backups";
import { dateTime } from "../lib/format";
import { Banner, Field, Modal, useToast } from "./ui";

/** A public key in full, `age1…`: what tells one saved key from another. */
export function PublicKey({ recipient }: Readonly<{ recipient: string }>) {
  return (
    <code className="mono" style={{ overflowWrap: "anywhere" }}>
      {recipient}
    </code>
  );
}

/**
 * A field for the secret key: hidden as a password, never kept. With
 * `recipient`, the public key it must match, so that the right one is
 * picked from the password manager.
 */
export function KeyInput({
  value,
  onChange,
  recipient,
}: Readonly<{
  value: string;
  onChange: (value: string) => void;
  recipient?: string | null | undefined;
}>) {
  return (
    <>
      <Field label={t("backupKey.key")} hint={t("backupKey.keyHint")}>
        <input
          className="input mono"
          type="password"
          autoComplete="off"
          spellCheck={false}
          placeholder="AGE-SECRET-KEY-1…"
          value={value}
          onChange={(event) => onChange(event.target.value)}
        />
      </Field>
      {recipient && (
        <p className="muted small" style={{ margin: 0 }}>
          {rich("backupKey.matches", { key: <PublicKey recipient={recipient} /> })}
        </p>
      )}
    </>
  );
}

/** Invalidated whenever the key or a backup changes: every query under `backups`. */
export const BACKUP_KEYS = [["backups"]];

/**
 * Asks an admin to type the backup key back: their next check further
 * away, and the newest backup tested with it. As a reminder, it can wait a
 * day, or say how to change a key the admin does not have.
 */
export function KeyCheckDialog({ reminder, onClose }: Readonly<{ reminder: boolean; onClose: () => void }>) {
  const client = useQueryClient();
  const toast = useToast();
  const [key, setKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [missing, setMissing] = useState(false);
  const current = useQuery({
    queryKey: ["backups", "key"],
    queryFn: () => unwrap(api.GET("/api/v1/backups/key")),
  });
  const refresh = () => client.invalidateQueries({ queryKey: ["backups"] });
  const check = async () => {
    setBusy(true);
    setError(null);
    try {
      const checked = await unwrap(api.POST("/api/v1/backups/key/check", { body: { key } }));
      toast(
        checked.testing
          ? t("backupKey.checkedTesting", { date: dateTime(checked.next_check_unix_ms) })
          : t("backupKey.checked", { date: dateTime(checked.next_check_unix_ms) }),
      );
      setKey("");
      await refresh();
      onClose();
    } catch (error_) {
      setError(errorMessage(error_));
    } finally {
      setBusy(false);
    }
  };
  const later = async () => {
    try {
      await done(api.POST("/api/v1/backups/key/later"));
    } catch (error_) {
      toast(errorMessage(error_), true);
    }
    await refresh();
    onClose();
  };
  const owner = current.data?.current;
  return (
    <Modal
      title={t("backupKey.checkTitle")}
      open
      onClose={reminder ? () => void later() : onClose}
      footer={
        <>
          {reminder ? (
            <>
              <button type="button" className="btn ghost" onClick={() => setMissing(true)}>
                {t("backupKey.dontHave")}
              </button>
              <button type="button" className="btn" onClick={() => void later()}>
                {t("backupKey.later")}
              </button>
            </>
          ) : (
            <button type="button" className="btn" onClick={onClose}>
              {t("ui.cancel")}
            </button>
          )}
          <button
            type="button"
            className="btn primary"
            disabled={busy || !looksLikeKey(key)}
            onClick={() => void check()}
          >
            {busy ? t("backupKey.checking") : t("backupKey.check")}
          </button>
        </>
      }
    >
      <form
        className="stack"
        onSubmit={(event) => {
          event.preventDefault();
          if (looksLikeKey(key)) {
            void check();
          }
        }}
      >
        <p style={{ margin: 0 }}>{reminder ? t("backupKey.reminder") : t("backupKey.checkHint")}</p>
        <KeyInput value={key} onChange={setKey} recipient={owner?.recipient} />
        {error && <Banner kind="bad">{error}</Banner>}
        {missing && (
          <Banner kind="warn">
            {owner
              ? rich("backupKey.missing", {
                  user: <strong>{owner.set_by}</strong>,
                  date: dateTime(owner.since_unix_ms),
                  link: (
                    <Link to="/backups" onClick={onClose}>
                      {t("nav.backups")}
                    </Link>
                  ),
                })
              : t("backupKey.missingNoKey")}
          </Banner>
        )}
      </form>
    </Modal>
  );
}

/**
 * For admins: backups not encrypted, damaged or not tested lately, and the
 * reminder to check the backup key when it is due.
 */
export function BackupWarnings() {
  const state = useQuery({
    queryKey: ["backups", "state"],
    queryFn: () => unwrap(api.GET("/api/v1/backups/state")),
    refetchInterval: 60_000,
  });
  const [keyCheckClosed, setKeyCheckClosed] = useState(false);
  if (!state.data) {
    return null;
  }
  const link = <Link to="/backups">{t("nav.backups")}</Link>;
  const messages: Record<BackupWarning, ReactNode> = {
    notEncrypted: rich("app.backupsNotEncrypted", { link }),
    damaged: rich("app.backupsDamaged", { count: state.data.damaged, link }),
    testDue: rich("app.backupsTestDue", { link }),
  };
  return (
    <>
      {backupWarnings(state.data).map((warning) => (
        <Banner key={warning} kind="warn">
          {messages[warning]}
        </Banner>
      ))}
      {state.data.check_due && !keyCheckClosed && <KeyCheckDialog reminder onClose={() => setKeyCheckClosed(true)} />}
    </>
  );
}
