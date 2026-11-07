// SPDX-License-Identifier: AGPL-3.0-or-later

// Audience certification (docs/certification.md): where each stream
// server's access log is read, how its reading goes, how long its lines
// are kept, and the days to download. Admins only; no address is shown.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { api, unwrap } from "../api";
import { Card, ConfirmButton, Empty, ErrorView, Field, Loading, NumberInput, PageHead, Segmented } from "../components/ui";
import { number, t } from "../i18n";
import {
  type AccessLogView,
  type CertificationSettings,
  FORMATS,
  type LogDraft,
  draftOf,
  exportUrl,
  inputOf,
} from "../lib/certification";
import { dateTime } from "../lib/format";
import { addDays, dayOf } from "../lib/listeners";
import { useAction } from "../lib/session";
import { here, useStation } from "../lib/station";

export function Certification() {
  return (
    <>
      <PageHead title={t("nav.certification")} description={t("certification.description")} />
      <Sources />
      <Days />
      <Retention />
    </>
  );
}

function useServers() {
  const station = useStation();
  return useQuery({
    queryKey: ["stream-servers", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/stream-servers", { params: { path: here() } })),
    enabled: station !== null,
  });
}

function useSources() {
  const station = useStation();
  return useQuery({
    queryKey: ["certification", "sources", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/certification/sources", { params: { path: here() } })),
    enabled: station !== null,
    refetchInterval: 30_000,
  });
}

function Sources() {
  const servers = useServers();
  const sources = useSources();
  if (servers.isPending || sources.isPending) {
    return <Loading />;
  }
  if (servers.isError || sources.isError) {
    return <ErrorView error={servers.error ?? sources.error} />;
  }
  return (
    <Card title={t("certification.sources")}>
      {servers.data.length === 0 ? (
        <Empty>{t("certification.noServer")}</Empty>
      ) : (
        <ul className="list">
          {servers.data.map((server) => (
            <Source
              key={server.id}
              serverId={server.id}
              name={server.name}
              log={sources.data.find((source) => source.server_id === server.id) ?? null}
            />
          ))}
        </ul>
      )}
    </Card>
  );
}

function Source({ serverId, name, log }: Readonly<{ serverId: number; name: string; log: AccessLogView | null }>) {
  const [editing, setEditing] = useState(false);
  const remove = useAction(
    () =>
      unwrap(
        api.DELETE("/api/v1/stations/{station_id}/stream-servers/{server_id}/access-log", {
          params: { path: here({ server_id: serverId }) },
        }),
      ),
    { success: t("certification.removed"), invalidate: [["certification"]] },
  );
  return (
    <li className="stack" style={{ gap: "0.5rem", padding: "0.75rem 0" }}>
      <div className="row" style={{ justifyContent: "space-between", flexWrap: "wrap" }}>
        <div>
          <strong>{name}</strong>
          <div className="muted small">
            {log ? (log.source === "file" ? log.path : log.url) : t("certification.noLog")}
          </div>
          {log && <State log={log} />}
        </div>
        <div className="row" style={{ gap: "0.25rem" }}>
          <button type="button" className="btn small" onClick={() => setEditing(!editing)}>
            {log ? t("certification.edit") : t("certification.setLog")}
          </button>
          {log && (
            <ConfirmButton
              className="btn small danger"
              question={t("certification.removeQuestion")}
              confirm={t("certification.remove")}
              onConfirm={() => remove.mutate(undefined)}
            >
              {t("certification.remove")}
            </ConfirmButton>
          )}
        </div>
      </div>
      {editing && <LogForm serverId={serverId} log={log} onDone={() => setEditing(false)} />}
    </li>
  );
}

function State({ log }: Readonly<{ log: AccessLogView }>) {
  if (log.error) {
    return (
      <div className="small" style={{ color: "var(--bad)" }}>
        {t("certification.error", { since: dateTime(log.error_since_unix_ms) })} {log.error}
      </div>
    );
  }
  return (
    <div className="muted small">
      {log.read_ok_at_unix_ms
        ? t("certification.readAt", { at: dateTime(log.read_ok_at_unix_ms), bytes: number(log.position) })
        : t("certification.notReadYet")}
    </div>
  );
}

function LogForm({ serverId, log, onDone }: Readonly<{ serverId: number; log: AccessLogView | null; onDone: () => void }>) {
  const [draft, setDraft] = useState<LogDraft>(draftOf(log));
  const set = (change: Partial<LogDraft>) => setDraft((current) => ({ ...current, ...change }));
  const save = useAction(
    (body: LogDraft) =>
      unwrap(
        api.PUT("/api/v1/stations/{station_id}/stream-servers/{server_id}/access-log", {
          params: { path: here({ server_id: serverId }) },
          body: inputOf(body),
        }),
      ),
    { success: t("certification.saved"), invalidate: [["certification"]] },
  );
  return (
    <div className="form-grid">
      <Field label={t("certification.source")}>
        <Segmented
          label={t("certification.source")}
          value={draft.source}
          options={[
            { value: "file", label: t("certification.file") },
            { value: "url", label: t("certification.url") },
          ]}
          onChange={(source) => set({ source })}
        />
      </Field>
      {draft.source === "file" ? (
        <Field label={t("certification.path")} hint={t("certification.pathHint")}>
          <input className="input" value={draft.path} onChange={(e) => set({ path: e.target.value })} placeholder="main/access.log" />
        </Field>
      ) : (
        <>
          <Field label={t("certification.address")} hint={t("certification.addressHint")}>
            <input
              className="input"
              value={draft.url}
              onChange={(e) => set({ url: e.target.value })}
              placeholder="https://stream.example.org/logs/access.log"
            />
          </Field>
          <Field label={t("certification.username")}>
            <input className="input" value={draft.username} autoComplete="off" onChange={(e) => set({ username: e.target.value })} />
          </Field>
          <Field label={t("certification.password")} hint={log?.password_set ? t("certification.passwordKept") : undefined}>
            <input
              className="input"
              type="password"
              autoComplete="new-password"
              value={draft.password}
              onChange={(e) => set({ password: e.target.value })}
            />
          </Field>
        </>
      )}
      <div className="row" style={{ gap: "0.5rem" }}>
        <button
          type="button"
          className="btn primary"
          disabled={save.isPending}
          onClick={() => save.mutate(draft, { onSuccess: onDone })}
        >
          {t("common.save")}
        </button>
        <button type="button" className="btn" onClick={onDone}>
          {t("common.cancel")}
        </button>
      </div>
    </div>
  );
}

function Days() {
  const station = useStation();
  const zone = useQuery({
    queryKey: ["settings", "station"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/station", { params: { path: here() } })),
  }).data?.timezone;
  const today = dayOf(Date.now(), zone);
  const [to, setTo] = useState(today);
  const [mount, setMount] = useState("");
  const from = addDays(to, -13);
  const servers = useServers();
  const days = useQuery({
    queryKey: ["certification", "days", station, from, to],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/certification/days", { params: { path: here(), query: { from, to } } })),
    enabled: station !== null,
  });
  const nameOf = (id: number) => servers.data?.find((server) => server.id === id)?.name ?? `#${id}`;
  return (
    <Card
      title={t("certification.days")}
      actions={
        <div className="row" style={{ gap: "0.5rem" }}>
          <input
            className="input"
            style={{ width: "10rem" }}
            aria-label={t("certification.mount")}
            placeholder={t("certification.allMounts")}
            value={mount}
            onChange={(e) => setMount(e.target.value)}
          />
          <input
            className="input"
            type="date"
            style={{ width: "auto" }}
            aria-label={t("certification.until")}
            value={to}
            max={today}
            onChange={(e) => e.target.value && setTo(e.target.value)}
          />
        </div>
      }
    >
      {days.isPending ? (
        <Loading />
      ) : days.isError ? (
        <ErrorView error={days.error} />
      ) : days.data.length === 0 ? (
        <Empty>{t("certification.noDay")}</Empty>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th>{t("certification.day")}</th>
              <th>{t("certification.server")}</th>
              <th className="num">{t("certification.sessions")}</th>
              <th className="num">{t("certification.lines")}</th>
              <th>{t("certification.gaps")}</th>
              <th>{t("certification.download")}</th>
            </tr>
          </thead>
          <tbody>
            {days.data.map((day) => (
              <tr key={`${day.day}-${day.server_id}`}>
                <td>{day.day}</td>
                <td>{nameOf(day.server_id)}</td>
                <td className="num">{number(day.sessions)}</td>
                <td className="num">{number(day.lines)}</td>
                <td>
                  {day.gaps.length === 0 ? (
                    <span className="muted">—</span>
                  ) : (
                    <span
                      className="badge warn"
                      title={day.gaps
                        .map((gap) => `${dateTime(gap.from_unix_ms)} – ${dateTime(gap.to_unix_ms)} (${gap.reason})`)
                        .join("\n")}
                    >
                      {t("certification.gapCount", { count: day.gaps.length })}
                    </span>
                  )}
                </td>
                <td>
                  <div className="row" style={{ gap: "0.25rem", flexWrap: "wrap" }}>
                    {FORMATS.map((format) => (
                      <a key={format} className="btn small" href={exportUrl(day.day, day.server_id, format, mount)} download>
                        {format}
                      </a>
                    ))}
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Card>
  );
}

function Retention() {
  const settings = useQuery({
    queryKey: ["certification", "settings"],
    queryFn: () => unwrap(api.GET("/api/v1/settings/certification")),
  });
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return <RetentionForm initial={settings.data} />;
}

function RetentionForm({ initial }: Readonly<{ initial: CertificationSettings }>) {
  const [months, setMonths] = useState(initial.retention_months);
  const save = useAction(
    (body: CertificationSettings) => unwrap(api.PUT("/api/v1/settings/certification", { body })),
    { success: t("certification.saved"), invalidate: [["certification"]] },
  );
  return (
    <Card
      title={t("certification.retention")}
      actions={
        <button
          type="button"
          className="btn primary"
          disabled={months === initial.retention_months || save.isPending}
          onClick={() => save.mutate({ retention_months: months })}
        >
          {t("common.save")}
        </button>
      }
    >
      <Field label={t("certification.retentionMonths")} hint={t("certification.retentionHint")}>
        <NumberInput value={months} min={1} max={25} step={1} onChange={setMonths} suffix={t("certification.months")} />
      </Field>
    </Card>
  );
}
