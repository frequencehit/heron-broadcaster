// SPDX-License-Identifier: AGPL-3.0-or-later

// Backups (admins): the configuration file and the database dump taken
// every day, backing up now, downloading, restoring, encryption and tests
// (docs/backups.md).

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useRef, useState } from "react";
import {
  type BackupSettings,
  type BackupTest,
  type BackupView,
  type KeyConfirmed,
  type NewBackupKey,
  type RestoreReport,
  api,
  done,
  errorMessage,
  unwrap,
} from "../api";
import { BACKUP_KEYS, KeyCheckDialog, KeyInput, PublicKey } from "../components/BackupKey";
import {
  Banner,
  Card,
  ConfirmButton,
  ErrorView,
  Field,
  Icon,
  Loading,
  Modal,
  PageHead,
  useToast,
} from "../components/ui";
import { type PlainKey, t } from "../i18n";
import {
  backupTime,
  downloadText,
  isEncryptedFile,
  looksLikeKey,
  looksLikeRecipient,
  shortKey,
} from "../lib/backups";
import { bytes, dateTime } from "../lib/format";
import { useAction, useCan } from "../lib/session";

const KINDS: Record<BackupView["kind"], PlainKey> = {
  daily: "backups.kind.daily",
  manual: "backups.kind.manual",
  before_restore: "backups.kind.before_restore",
  uploaded: "backups.kind.uploaded",
};

/** What to restore: a backup of the list, or a file. */
type Restoring = { backup: BackupView } | { file: File; encrypted: boolean };

/** A line that went well. */
function Good({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <p style={{ margin: 0 }}>
      <span className="badge good">
        <Icon name="check" />
      </span>{" "}
      {children}
    </p>
  );
}

export function Backups() {
  const isAdmin = useCan("admin");
  const backups = useQuery({
    queryKey: ["backups"],
    queryFn: () => unwrap(api.GET("/api/v1/backups")),
    enabled: isAdmin,
  });
  const [report, setReport] = useState<RestoreReport | null>(null);
  const [restoring, setRestoring] = useState<Restoring | null>(null);
  const [testing, setTesting] = useState<BackupView | null>(null);
  const create = useAction(() => unwrap(api.POST("/api/v1/backups")), {
    success: (backup) => t("backups.created", { name: backup.name }),
    invalidate: BACKUP_KEYS,
  });
  const remove = useAction(
    (name: string) => done(api.DELETE("/api/v1/backups/{name}", { params: { path: { name } } })),
    { success: t("backups.deleted"), invalidate: BACKUP_KEYS },
  );
  const test = useAction(
    (name: string) => unwrap(api.POST("/api/v1/backups/{name}/test", { params: { path: { name } } })),
    { success: t("backups.testStarted"), invalidate: [["backups", "tests"]] },
  );
  const upload = useRef<HTMLInputElement>(null);

  if (!isAdmin) {
    return <Banner kind="warn">{t("backups.adminsOnly")}</Banner>;
  }
  return (
    <>
      <PageHead
        title={t("nav.backups")}
        description={t("backups.description")}
        actions={
          <>
            <button type="button" className="btn" onClick={() => upload.current?.click()}>
              <Icon name="upload" /> {t("backups.restoreFile")}
            </button>
            <button
              type="button"
              className="btn primary"
              disabled={create.isPending}
              onClick={() => create.mutate(undefined)}
            >
              <Icon name="download" /> {create.isPending ? t("backups.creating") : t("backups.create")}
            </button>
          </>
        }
      />
      <input
        ref={upload}
        type="file"
        accept="application/json,.json,.age"
        hidden
        onChange={(event) => {
          const file = event.target.files?.[0];
          event.target.value = "";
          if (file) {
            void isEncryptedFile(file).then((encrypted) => setRestoring({ file, encrypted }));
          }
        }}
      />
      <Card flush>
        {backups.isPending && <Loading />}
        {backups.isError && <ErrorView error={backups.error} />}
        {backups.data && backups.data.length === 0 && <div className="empty">{t("backups.none")}</div>}
        {backups.data && backups.data.length > 0 && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("backups.time")}</th>
                  <th className="hide-narrow">{t("backups.kind")}</th>
                  <th>{t("backups.config")}</th>
                  <th className="hide-narrow">{t("backups.database")}</th>
                  <th className="hide-narrow">{t("backups.encryption")}</th>
                  <th>
                    <span className="visually-hidden">{t("library.actions")}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {backups.data.map((backup) => (
                  <tr key={backup.name}>
                    <td title={backup.name}>
                      {dateTime(backup.created_at_unix_ms)}
                      {backup.damaged && (
                        <>
                          {" "}
                          <span className="badge bad" title={backup.damaged}>
                            {t("backups.damaged")}
                          </span>
                        </>
                      )}
                    </td>
                    <td className="hide-narrow">
                      <span className="badge">{t(KINDS[backup.kind])}</span>
                    </td>
                    <td>
                      {backup.config_size > 0 ? (
                        <a href={`/api/v1/backups/${encodeURIComponent(backup.name)}/config`} download>
                          {bytes(backup.config_size)}
                        </a>
                      ) : (
                        <span className="muted">–</span>
                      )}
                    </td>
                    <td className="hide-narrow">
                      {backup.database_size !== null && backup.database_size !== undefined ? (
                        <a href={`/api/v1/backups/${encodeURIComponent(backup.name)}/database`} download>
                          {bytes(backup.database_size)}
                        </a>
                      ) : backup.database_error ? (
                        <span className="badge bad" title={backup.database_error}>
                          {t("backups.databaseFailed")}
                        </span>
                      ) : (
                        <span className="muted">–</span>
                      )}
                    </td>
                    <td className="hide-narrow">
                      <EncryptionBadge backup={backup} />
                    </td>
                    <td className="actions">
                      <button type="button" className="btn small" onClick={() => setRestoring({ backup })}>
                        {t("backups.restore")}
                      </button>
                      <button
                        type="button"
                        className="btn small"
                        disabled={test.isPending}
                        onClick={() => (backup.encrypted ? setTesting(backup) : test.mutate(backup.name))}
                      >
                        {t("backups.test")}
                      </button>
                      <ConfirmButton
                        className="btn ghost small icon danger"
                        title={t("backups.delete")}
                        confirm={t("categories.deleteButton")}
                        question={<p>{t("backups.deleteQuestion", { name: dateTime(backup.created_at_unix_ms) })}</p>}
                        onConfirm={() => remove.mutate(backup.name)}
                      >
                        <Icon name="trash" />
                      </ConfirmButton>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
      <p className="muted small">{t("backups.databaseHint")}</p>
      <EncryptionCard />
      <TestsCard />
      <SettingsCard />
      {restoring && (
        <RestoreDialog restoring={restoring} onClose={() => setRestoring(null)} onRestored={setReport} />
      )}
      {testing && <TestDialog backup={testing} onClose={() => setTesting(null)} />}
      {report && <ReportDialog report={report} />}
    </>
  );
}

function EncryptionBadge({ backup }: Readonly<{ backup: BackupView }>) {
  if (!backup.encrypted) {
    return <span className="muted">{t("backups.inClear")}</span>;
  }
  if (backup.key === "earlier") {
    return (
      <span className="badge warn" title={t("backups.earlierKeyHint")}>
        {t("backups.earlierKey")}
      </span>
    );
  }
  if (backup.key === "unknown") {
    return (
      <span className="badge" title={t("backups.unknownKeyHint")}>
        {t("backups.unknownKey")}
      </span>
    );
  }
  return <span className="badge good">{t("backups.encrypted")}</span>;
}

/** Asks before a restore, with the key of an encrypted backup. */
function RestoreDialog({
  restoring,
  onClose,
  onRestored,
}: Readonly<{ restoring: Restoring; onClose: () => void; onRestored: (report: RestoreReport) => void }>) {
  const [key, setKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const encrypted = "backup" in restoring ? restoring.backup.encrypted : restoring.encrypted;
  const restore = async () => {
    setBusy(true);
    setError(null);
    try {
      let report: RestoreReport;
      if ("backup" in restoring) {
        report = await unwrap(
          api.POST("/api/v1/backups/{name}/restore", {
            params: { path: { name: restoring.backup.name } },
            body: encrypted ? { key } : {},
          }),
        );
      } else {
        // The file's bytes as they are: an encrypted one is not text.
        const content = await restoring.file.arrayBuffer();
        report = await unwrap(
          api.POST("/api/v1/backups/restore", {
            params: { header: encrypted ? { "X-Heron-Backup-Key": key } : {} },
            body: "",
            bodySerializer: () => content,
            headers: { "Content-Type": "application/octet-stream" },
          }),
        );
      }
      onClose();
      onRestored(report);
    } catch (error_) {
      setError(errorMessage(error_));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      title={t("backups.restore")}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("ui.cancel")}
          </button>
          <button
            type="button"
            className="btn danger solid"
            disabled={busy || (encrypted && !looksLikeKey(key))}
            onClick={() => void restore()}
          >
            {busy ? t("backups.restoring") : t("backups.restore")}
          </button>
        </>
      }
    >
      <div className="stack">
        <p style={{ margin: 0 }}>
          {"backup" in restoring
            ? t("backups.restoreQuestion", { name: dateTime(restoring.backup.created_at_unix_ms) })
            : t("backups.restoreFileQuestion", { name: restoring.file.name })}
        </p>
        {encrypted && (
          <>
            <p className="muted small" style={{ margin: 0 }}>
              {"backup" in restoring && restoring.backup.key === "earlier"
                ? t("backups.restoreEarlierKey")
                : t("backups.restoreKey")}
            </p>
            <KeyInput
              value={key}
              onChange={setKey}
              recipient={"backup" in restoring ? restoring.backup.recipient : null}
            />
            {!("backup" in restoring) && (
              <p className="muted small" style={{ margin: 0 }}>
                {t("backupKey.noPublicKey")}
              </p>
            )}
          </>
        )}
        {error && <Banner kind="bad">{error}</Banner>}
      </div>
    </Modal>
  );
}

/** Tests an encrypted backup with its key. */
function TestDialog({ backup, onClose }: Readonly<{ backup: BackupView; onClose: () => void }>) {
  const [key, setKey] = useState("");
  const test = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/backups/{name}/test", {
          params: { path: { name: backup.name } },
          body: { key },
        }),
      ),
    { success: t("backups.testStarted"), invalidate: [["backups", "tests"]], onSuccess: onClose },
  );
  return (
    <Modal
      title={t("backups.testTitle", { name: dateTime(backup.created_at_unix_ms) })}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("ui.cancel")}
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={test.isPending || !looksLikeKey(key)}
            onClick={() => test.mutate(undefined)}
          >
            {t("backups.test")}
          </button>
        </>
      }
    >
      <div className="stack">
        <p style={{ margin: 0 }}>{t("backups.testHint")}</p>
        {backup.key === "earlier" && (
          <p className="muted small" style={{ margin: 0 }}>
            {t("backups.restoreEarlierKey")}
          </p>
        )}
        <KeyInput value={key} onChange={setKey} recipient={backup.recipient} />
      </div>
    </Modal>
  );
}

/** Whether backups are encrypted, and the key: set, checked, changed, turned off. */
function EncryptionCard() {
  const key = useQuery({
    queryKey: ["backups", "key"],
    queryFn: () => unwrap(api.GET("/api/v1/backups/key")),
  });
  const [setup, setSetup] = useState<"new" | "confirm" | null>(null);
  const [checking, setChecking] = useState(false);
  const drop = useAction(() => unwrap(api.DELETE("/api/v1/backups/key")), { invalidate: BACKUP_KEYS });
  const data = key.data;
  let actions: ReactNode = null;
  if (data?.state === "on") {
    actions = (
      <>
        <button type="button" className="btn small" onClick={() => setChecking(true)}>
          {t("backups.checkKey")}
        </button>
        <button type="button" className="btn small" onClick={() => setSetup("new")}>
          {t("backups.changeKey")}
        </button>
        <ConfirmButton
          className="btn small danger"
          confirm={t("backups.turnOff")}
          question={<p>{t("backups.turnOffQuestion")}</p>}
          onConfirm={() => drop.mutate(undefined)}
        >
          {t("backups.turnOff")}
        </ConfirmButton>
      </>
    );
  } else if (data?.state === "off") {
    actions = (
      <button type="button" className="btn small primary" onClick={() => setSetup("new")}>
        {t("backups.encrypt")}
      </button>
    );
  }
  return (
    <Card title={t("backups.encryptionTitle")} actions={actions}>
      <div className="stack">
        {key.isError && <ErrorView error={key.error} />}
        {data?.state === "off" && <Banner kind="warn">{t("backups.encryptionOff")}</Banner>}
        {data?.state === "pending" && (
          <Banner kind="warn">
            {t("backups.keyPending")}{" "}
            <button type="button" className="btn small" onClick={() => setSetup("confirm")}>
              {t("backups.typeBack")}
            </button>{" "}
            <button type="button" className="btn small ghost" onClick={() => drop.mutate(undefined)}>
              {t("backups.dropKey")}
            </button>
          </Banner>
        )}
        {data?.current && (
          <dl className="facts">
            <dt>{t("backups.publicKey")}</dt>
            <dd className="mono" title={data.current.recipient}>
              {shortKey(data.current.recipient)}
            </dd>
            <dt>{t("backups.keySet")}</dt>
            <dd>
              {t("backups.keySetBy", { date: dateTime(data.current.since_unix_ms), user: data.current.set_by })}
            </dd>
          </dl>
        )}
        {data && data.previous.length > 0 && (
          <p className="muted small" style={{ margin: 0 }}>
            {t("backups.earlierKeys", { count: data.previous.length })}
          </p>
        )}
        <p className="muted small" style={{ margin: 0 }}>
          {t("backups.encryptionHint")}
        </p>
      </div>
      {setup && <SetupDialog start={setup} onClose={() => setSetup(null)} />}
      {checking && <KeyCheckDialog reminder={false} onClose={() => setChecking(false)} />}
    </Card>
  );
}

/** Sets a key: generated and shown once, or the admin's own; then typed back. */
function SetupDialog({ start, onClose }: Readonly<{ start: "new" | "confirm"; onClose: () => void }>) {
  const client = useQueryClient();
  const toast = useToast();
  const [step, setStep] = useState<"choose" | "show" | "confirm" | "done">(start === "new" ? "choose" : "confirm");
  const [own, setOwn] = useState(false);
  const [recipient, setRecipient] = useState("");
  const [created, setCreated] = useState<NewBackupKey | null>(null);
  const [typed, setTyped] = useState("");
  const [confirmed, setConfirmed] = useState<KeyConfirmed | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // The key waiting to be typed back, when the dialog opens on it.
  const pending = useQuery({
    queryKey: ["backups", "key"],
    queryFn: () => unwrap(api.GET("/api/v1/backups/key")),
  });
  const refresh = () => client.invalidateQueries({ queryKey: ["backups"] });
  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (error_) {
      setError(errorMessage(error_));
    } finally {
      setBusy(false);
    }
  };
  const create = () =>
    run(async () => {
      const result = await unwrap(api.POST("/api/v1/backups/key", { body: own ? { recipient } : {} }));
      setCreated(result);
      setStep(result.key ? "show" : "confirm");
      await refresh();
    });
  const confirm = () =>
    run(async () => {
      setConfirmed(await unwrap(api.POST("/api/v1/backups/key/confirm", { body: { key: typed } })));
      setTyped("");
      setStep("done");
      await refresh();
    });
  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      toast(t("backups.keyCopied"));
    } catch {
      toast(t("backups.keyCopyFailed"), true);
    }
  };
  // The key is not kept once the dialog is gone.
  const close = () => {
    setCreated(null);
    onClose();
  };

  let body: ReactNode;
  let footer: ReactNode;
  if (step === "choose") {
    body = (
      <>
        <p style={{ margin: 0 }}>{t("backups.setupHint")}</p>
        <label className="check">
          <input type="radio" checked={!own} onChange={() => setOwn(false)} /> {t("backups.generate")}
        </label>
        <label className="check">
          <input type="radio" checked={own} onChange={() => setOwn(true)} /> {t("backups.ownKey")}
        </label>
        {own && (
          <Field label={t("backups.publicKey")} hint={t("backups.ownKeyHint")}>
            <input
              className="input mono"
              spellCheck={false}
              placeholder="age1…"
              value={recipient}
              onChange={(event) => setRecipient(event.target.value)}
            />
          </Field>
        )}
      </>
    );
    footer = (
      <button
        type="button"
        className="btn primary"
        disabled={busy || (own && !looksLikeRecipient(recipient))}
        onClick={() => void create()}
      >
        {t("backups.next")}
      </button>
    );
  } else if (step === "show" && created?.key) {
    const secret = created.key;
    const file = created.key_file;
    body = (
      <>
        <Banner kind="warn">{t("backups.saveKey")}</Banner>
        <input
          className="input mono"
          readOnly
          value={secret}
          aria-label={t("backupKey.key")}
          onFocus={(event) => event.target.select()}
        />
        <dl className="facts">
          <dt>{t("backups.publicKey")}</dt>
          <dd>
            <PublicKey recipient={created.recipient} />
          </dd>
        </dl>
        <div className="row">
          <button type="button" className="btn small" onClick={() => void copy(secret)}>
            {t("backups.copyKey")}
          </button>
          {file && (
            <button type="button" className="btn small" onClick={() => downloadText("heron-backup-key.txt", file)}>
              <Icon name="download" /> {t("backups.downloadKey")}
            </button>
          )}
        </div>
        <p className="muted small" style={{ margin: 0 }}>
          {t("backups.saveKeyHint")} {t("backups.savePublicKey")}
        </p>
      </>
    );
    footer = (
      <button type="button" className="btn primary" onClick={() => setStep("confirm")}>
        {t("backups.savedKey")}
      </button>
    );
  } else if (step === "confirm") {
    body = (
      <>
        <p style={{ margin: 0 }}>{t("backups.typeBackHint")}</p>
        <KeyInput
          value={typed}
          onChange={setTyped}
          recipient={created?.recipient ?? pending.data?.pending_recipient}
        />
      </>
    );
    footer = (
      <button
        type="button"
        className="btn primary"
        disabled={busy || !looksLikeKey(typed)}
        onClick={() => void confirm()}
      >
        {busy ? t("backups.encrypting") : t("backups.turnOn")}
      </button>
    );
  } else {
    body = confirmed && (
      <>
        <Good>{t("backups.encryptionOn", { count: confirmed.encrypted.length })}</Good>
        {confirmed.left_in_clear.length > 0 && (
          <Banner kind="warn">{t("backups.leftInClear", { names: confirmed.left_in_clear.join(", ") })}</Banner>
        )}
        <p className="muted small" style={{ margin: 0 }}>
          {t("backups.copiesInClear")}
        </p>
      </>
    );
    footer = (
      <button type="button" className="btn primary" onClick={close}>
        {t("ui.close")}
      </button>
    );
  }
  return (
    <Modal title={t("backups.encryptionTitle")} open onClose={close} footer={footer}>
      <div className="stack">
        {body}
        {error && <Banner kind="bad">{error}</Banner>}
      </div>
    </Modal>
  );
}

/** A backup's time, from its name. */
function whenTaken(name: string): string {
  const time = backupTime(name);
  return time === null ? name : dateTime(time);
}

function testLine(test: BackupTest): string {
  const name = whenTaken(test.name);
  const date = dateTime(test.at_unix_ms);
  const user = test.by ?? t("backups.byHeron");
  return test.ok
    ? t("backups.testPassed", { name, date, user })
    : t("backups.testFailed", { name, date, user, error: test.error ?? "" });
}

/** The last test, and the one under way. */
function TestsCard() {
  const tests = useQuery({
    queryKey: ["backups", "tests"],
    queryFn: () => unwrap(api.GET("/api/v1/backups/tests")),
    refetchInterval: (query) => (query.state.data?.testing ? 2_000 : false),
  });
  const data = tests.data;
  return (
    <Card title={t("backups.testsTitle")}>
      <div className="stack">
        {tests.isError && <ErrorView error={tests.error} />}
        {data?.testing && (
          <p className="row" style={{ margin: 0 }}>
            <span className="spinner" aria-hidden="true" /> {t("backups.testing", { name: whenTaken(data.testing) })}
          </p>
        )}
        {data?.last &&
          (data.last.ok ? <Good>{testLine(data.last)}</Good> : <Banner kind="bad">{testLine(data.last)}</Banner>)}
        {data?.last && !data.last.ok && data.last_ok && (
          <p className="muted small" style={{ margin: 0 }}>
            {testLine(data.last_ok)}
          </p>
        )}
        {data && !data.last && !data.testing && <p style={{ margin: 0 }}>{t("backups.neverTested")}</p>}
        <p className="muted small" style={{ margin: 0 }}>
          {t("backups.testsHint")}
        </p>
      </div>
    </Card>
  );
}

function SettingsCard() {
  const settings = useQuery({
    queryKey: ["settings", "backups"],
    queryFn: () => unwrap(api.GET("/api/v1/settings/backups")),
  });
  const [draft, setDraft] = useState<BackupSettings | null>(null);
  useEffect(() => {
    if (settings.data) {
      setDraft(settings.data);
    }
  }, [settings.data]);
  const save = useAction((body: BackupSettings) => unwrap(api.PUT("/api/v1/settings/backups", { body })), {
    success: t("categories.saved"),
    invalidate: [["settings", "backups"]],
  });
  const changed = draft !== null && JSON.stringify(draft) !== JSON.stringify(settings.data);
  return (
    <Card
      title={t("backups.settings")}
      actions={
        <button
          type="button"
          className="btn small primary"
          disabled={!changed || save.isPending}
          onClick={() => draft && save.mutate(draft)}
        >
          {t("common.save")}
        </button>
      }
    >
      {settings.isError && <ErrorView error={settings.error} />}
      {draft && (
        <div className="form-grid">
          <label className="check">
            <input
              type="checkbox"
              checked={draft.enabled}
              onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })}
            />{" "}
            {t("backups.daily")}
          </label>
          <Field label={t("backups.at")} hint={t("backups.atHint")}>
            <input
              className="input"
              type="time"
              value={draft.time}
              disabled={!draft.enabled}
              onChange={(event) => setDraft({ ...draft, time: event.target.value })}
            />
          </Field>
          <Field label={t("backups.keepDays")} hint={t("backups.keepDaysHint")}>
            <input
              className="input"
              type="number"
              min={1}
              max={90}
              value={draft.keep_days}
              onChange={(event) => setDraft({ ...draft, keep_days: Number(event.target.value) })}
            />
          </Field>
          <label className="check">
            <input
              type="checkbox"
              checked={draft.database}
              onChange={(event) => setDraft({ ...draft, database: event.target.checked })}
            />{" "}
            {t("backups.withDatabase")}
          </label>
        </div>
      )}
    </Card>
  );
}

/** What a restore did; everyone signs in again after it. */
function ReportDialog({ report }: { report: RestoreReport }) {
  const leftOut = Object.entries(report.left_out ?? {});
  const restored = Object.entries(report.restored ?? {}).filter(([, count]) => count > 0);
  return (
    <Modal
      title={t("backups.restored")}
      open
      onClose={() => window.location.reload()}
      footer={
        <button type="button" className="btn primary" onClick={() => window.location.reload()}>
          {t("backups.signInAgain")}
        </button>
      }
    >
      <p>{t("backups.restoredHint", { name: report.before_restore })}</p>
      {leftOut.length > 0 && (
        <Banner kind="warn">
          {t("backups.leftOut", { count: String(leftOut.reduce((sum, [, count]) => sum + count, 0)) })}
          {report.missing_tracks_total > 0 && (
            <>
              {" "}
              {t("backups.missingTracks", { count: String(report.missing_tracks_total) })}
              <ul className="small">
                {report.missing_tracks.map((path) => (
                  <li key={path} className="mono">
                    {path}
                  </li>
                ))}
              </ul>
            </>
          )}
          {report.missing_artists_total > 0 && (
            <>
              {" "}
              {t("backups.missingArtists", { names: report.missing_artists.join(", ") })}
            </>
          )}
        </Banner>
      )}
      <dl className="facts small">
        {restored.map(([table, count]) => (
          <div key={table} style={{ display: "contents" }}>
            <dt className="mono">{table}</dt>
            <dd>{count}</dd>
          </div>
        ))}
      </dl>
    </Modal>
  );
}
