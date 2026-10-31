// SPDX-License-Identifier: AGPL-3.0-or-later

// Backups (admins): the configuration file and the database dump taken
// every day, backing up now, downloading, restoring (docs/backups.md).

import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import {
  type BackupSettings,
  type BackupView,
  type RestoreReport,
  api,
  done,
  errorMessage,
  unwrap,
} from "../api";
import { Banner, Card, ConfirmButton, ErrorView, Field, Icon, Loading, Modal, PageHead } from "../components/ui";
import { type PlainKey, t } from "../i18n";
import { bytes, dateTime } from "../lib/format";
import { useAction, useCan } from "../lib/session";

const KINDS: Record<BackupView["kind"], PlainKey> = {
  daily: "backups.kind.daily",
  manual: "backups.kind.manual",
  before_restore: "backups.kind.before_restore",
  uploaded: "backups.kind.uploaded",
};

export function Backups() {
  const isAdmin = useCan("admin");
  const backups = useQuery({
    queryKey: ["backups"],
    queryFn: () => unwrap(api.GET("/api/v1/backups")),
    enabled: isAdmin,
  });
  const [report, setReport] = useState<RestoreReport | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const create = useAction(() => unwrap(api.POST("/api/v1/backups")), {
    success: (backup) => t("backups.created", { name: backup.name }),
    invalidate: [["backups"]],
  });
  const remove = useAction(
    (name: string) => done(api.DELETE("/api/v1/backups/{name}", { params: { path: { name } } })),
    { success: t("backups.deleted"), invalidate: [["backups"]] },
  );
  const restore = useAction(
    (name: string) => unwrap(api.POST("/api/v1/backups/{name}/restore", { params: { path: { name } } })),
    { onSuccess: setReport },
  );
  const upload = useRef<HTMLInputElement>(null);
  const restoreFile = async (file: File) => {
    setFailed(null);
    try {
      const text = await file.text();
      const result = await unwrap(
        api.POST("/api/v1/backups/restore", {
          body: text,
          bodySerializer: (body: string) => body,
          headers: { "Content-Type": "application/json" },
        }),
      );
      setReport(result);
    } catch (error) {
      setFailed(errorMessage(error));
    }
  };

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
        accept="application/json,.json"
        hidden
        onChange={(event) => {
          const file = event.target.files?.[0];
          event.target.value = "";
          if (file && window.confirm(t("backups.restoreQuestion", { name: file.name }))) {
            void restoreFile(file);
          }
        }}
      />
      {failed && <Banner kind="bad">{failed}</Banner>}
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
                  <th>{t("backups.kind")}</th>
                  <th>{t("backups.config")}</th>
                  <th>{t("backups.database")}</th>
                  <th>
                    <span className="visually-hidden">{t("library.actions")}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {backups.data.map((backup) => (
                  <tr key={backup.name}>
                    <td title={backup.name}>{dateTime(backup.created_at_unix_ms)}</td>
                    <td>
                      <span className="badge">{t(KINDS[backup.kind])}</span>
                    </td>
                    <td>
                      <a href={`/api/v1/backups/${encodeURIComponent(backup.name)}/config`} download>
                        {bytes(backup.config_size)}
                      </a>
                    </td>
                    <td>
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
                    <td className="actions">
                      <ConfirmButton
                        className="btn small"
                        confirm={t("backups.restore")}
                        question={<p>{t("backups.restoreQuestion", { name: dateTime(backup.created_at_unix_ms) })}</p>}
                        disabled={restore.isPending}
                        onConfirm={() => restore.mutate(backup.name)}
                      >
                        {restore.isPending && restore.variables === backup.name
                          ? t("backups.restoring")
                          : t("backups.restore")}
                      </ConfirmButton>
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
      <SettingsCard />
      {report && <ReportDialog report={report} />}
    </>
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
