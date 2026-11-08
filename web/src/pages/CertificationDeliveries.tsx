// SPDX-License-Identifier: AGPL-3.0-or-later

// Certification deliveries (docs/certification.md#automatic-delivery): a
// day's export sent by FTPS to a certification body at a set time, its last
// days sent with their attempts, Send now and Test connection. Admins only.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { api, done, unwrap } from "../api";
import { Card, ConfirmButton, Empty, ErrorView, Field, Loading, Segmented } from "../components/ui";
import { t } from "../i18n";
import {
  type CertificationDelivery,
  type DeliveryDraft,
  type DeliveryRun,
  FILE_NAME_TOKENS,
  FORMATS,
  deliveryDraftOf,
  deliveryInputOf,
  previewFileName,
} from "../lib/certification";
import { bytes, dateTime } from "../lib/format";
import { addDays, dayOf } from "../lib/listeners";
import { useAction } from "../lib/session";
import { here, useStation } from "../lib/station";

const DAYS_BACK = [1, 2, 3, 4, 5, 6, 7];

function useDeliveries() {
  const station = useStation();
  return useQuery({
    queryKey: ["certification", "deliveries", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/certification/deliveries", { params: { path: here() } })),
    enabled: station !== null,
    refetchInterval: 15_000,
  });
}

function useServers() {
  const station = useStation();
  return useQuery({
    queryKey: ["stream-servers", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/stream-servers", { params: { path: here() } })),
    enabled: station !== null,
  });
}

/** The station's own day, for the days to send. */
function useToday(): string {
  const zone = useQuery({
    queryKey: ["settings", "station"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/station", { params: { path: here() } })),
  }).data?.timezone;
  return dayOf(Date.now(), zone);
}

export function Deliveries() {
  const deliveries = useDeliveries();
  const servers = useServers();
  const [adding, setAdding] = useState(false);
  if (deliveries.isPending || servers.isPending) {
    return <Loading />;
  }
  if (deliveries.isError || servers.isError) {
    return <ErrorView error={deliveries.error ?? servers.error} />;
  }
  return (
    <Card
      title={t("certification.deliveries")}
      actions={
        servers.data.length > 0 && (
          <button type="button" className="btn small" onClick={() => setAdding(!adding)}>
            {t("certification.addDelivery")}
          </button>
        )
      }
    >
      <p className="muted small">{t("certification.deliveriesHelp")}</p>
      {adding && <DeliveryForm delivery={null} onDone={() => setAdding(false)} />}
      {deliveries.data.length === 0 ? (
        !adding && <Empty>{servers.data.length === 0 ? t("certification.noServer") : t("certification.noDelivery")}</Empty>
      ) : (
        <ul className="list">
          {deliveries.data.map((delivery) => (
            <Delivery key={delivery.id} delivery={delivery} />
          ))}
        </ul>
      )}
    </Card>
  );
}

function StateBadge({ run }: Readonly<{ run: DeliveryRun }>) {
  const kind = { delivered: "good", failed: "bad", pending: "warn", sending: "warn" }[run.state];
  return <span className={`badge ${kind}`}>{t(`certification.state.${run.state}`)}</span>;
}

function schedule(delivery: CertificationDelivery): string {
  const day = delivery.days_back === 1 ? t("certification.dayBefore") : t("certification.daysBefore", { count: delivery.days_back });
  return t("certification.schedule", { time: delivery.send_at, day });
}

function Delivery({ delivery }: Readonly<{ delivery: CertificationDelivery }>) {
  const today = useToday();
  const [editing, setEditing] = useState(false);
  const [showLog, setShowLog] = useState(false);
  const [day, setDay] = useState<string | null>(null);
  const [test, setTest] = useState<{ ok: boolean; error?: string | null } | null>(null);
  const path = { params: { path: here({ id: delivery.id }) } };
  const remove = useAction(() => done(api.DELETE("/api/v1/stations/{station_id}/certification/deliveries/{id}", path)), {
    success: t("certification.deliveryDeleted"),
    invalidate: [["certification", "deliveries"]],
  });
  const send = useAction(
    (sent: string) =>
      unwrap(api.POST("/api/v1/stations/{station_id}/certification/deliveries/{id}/send", { ...path, body: { day: sent } })),
    { success: (run) => t("certification.sending", { day: run.day }), invalidate: [["certification", "deliveries"], ["certification", "runs"]] },
  );
  const check = useAction(() => unwrap(api.POST("/api/v1/stations/{station_id}/certification/deliveries/{id}/test", path)), {
    onSuccess: (result) => setTest(result),
  });
  const last = delivery.last;
  const chosen = day ?? addDays(today, -delivery.days_back);
  return (
    <li className="stack" style={{ gap: "0.5rem", padding: "0.75rem 0" }}>
      <div className="row" style={{ justifyContent: "space-between", flexWrap: "wrap", alignItems: "flex-start" }}>
        <div>
          <strong>{delivery.name}</strong> {!delivery.enabled && <span className="badge">{t("certification.off")}</span>}
          <div className="muted small">
            {`ftps://${delivery.username}@${delivery.host}:${delivery.port}/${delivery.remote_dir.replace(/^\//, "")}`} ·{" "}
            {delivery.server_name} · {delivery.mount ?? t("certification.allMounts")} · {delivery.format}
            {delivery.compression === "gzip" ? " · gzip" : ""}
          </div>
          <div className="muted small">{schedule(delivery)}</div>
          {last ? (
            <div className="small">
              <StateBadge run={last} /> {t("certification.lastDay", { day: last.day })}
              {last.error && last.state !== "delivered" && <span style={{ color: "var(--bad)" }}> {last.error}</span>}
            </div>
          ) : (
            <div className="muted small">{t("certification.notSentYet")}</div>
          )}
          {test && (
            <div className="small" role="status" style={{ color: test.ok ? "var(--good)" : "var(--bad)" }}>
              {test.ok ? t("certification.testOk") : t("certification.testFailed", { error: test.error ?? "" })}
            </div>
          )}
        </div>
        <div className="row" style={{ gap: "0.25rem", flexWrap: "wrap" }}>
          <button type="button" className="btn small" disabled={check.isPending} onClick={() => check.mutate(undefined)}>
            {check.isPending ? t("certification.testing") : t("certification.test")}
          </button>
          <input
            className="input"
            type="date"
            style={{ width: "auto" }}
            aria-label={t("certification.sendDay")}
            value={chosen}
            max={today}
            onChange={(e) => e.target.value && setDay(e.target.value)}
          />
          <button type="button" className="btn small" disabled={send.isPending} onClick={() => send.mutate(chosen)}>
            {t("certification.sendNow")}
          </button>
          <button type="button" className="btn small" aria-expanded={showLog} onClick={() => setShowLog(!showLog)}>
            {t("certification.log")}
          </button>
          <button type="button" className="btn small" onClick={() => setEditing(!editing)}>
            {t("certification.edit")}
          </button>
          <ConfirmButton
            className="btn small danger"
            question={t("certification.deleteDeliveryQuestion")}
            confirm={t("certification.deleteDelivery")}
            onConfirm={() => remove.mutate(undefined)}
          >
            {t("certification.deleteDelivery")}
          </ConfirmButton>
        </div>
      </div>
      {editing && <DeliveryForm delivery={delivery} onDone={() => setEditing(false)} />}
      {showLog && <Runs deliveryId={delivery.id} />}
    </li>
  );
}

/** A delivery's last 30 days sent. */
function Runs({ deliveryId }: Readonly<{ deliveryId: number }>) {
  const station = useStation();
  const runs = useQuery({
    queryKey: ["certification", "runs", station, deliveryId],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/certification/deliveries/{id}/runs", {
          params: { path: here({ id: deliveryId }) },
        }),
      ),
    enabled: station !== null,
    refetchInterval: 15_000,
  });
  if (runs.isPending) {
    return <Loading />;
  }
  if (runs.isError) {
    return <ErrorView error={runs.error} />;
  }
  if (runs.data.length === 0) {
    return <Empty>{t("certification.notSentYet")}</Empty>;
  }
  return (
    <table className="table" aria-label={t("certification.log")}>
      <thead>
        <tr>
          <th>{t("certification.day")}</th>
          <th>{t("certification.state")}</th>
          <th>{t("certification.sentFile")}</th>
          <th className="num">{t("certification.size")}</th>
          <th>{t("certification.gaps")}</th>
          <th>{t("certification.attempts")}</th>
        </tr>
      </thead>
      <tbody>
        {runs.data.map((run) => (
          <tr key={run.id}>
            <td>
              {run.day}
              {run.manual && <span className="muted small"> · {t("certification.manual")}</span>}
            </td>
            <td>
              <StateBadge run={run} />
              {run.state === "pending" && run.next_attempt_unix_ms != null && (
                <div className="muted small">{t("certification.nextAttempt", { at: dateTime(run.next_attempt_unix_ms) })}</div>
              )}
              {run.error && run.state !== "delivered" && (
                <div className="small" style={{ color: "var(--bad)" }}>
                  {run.error}
                </div>
              )}
            </td>
            <td>{run.file_name ?? "–"}</td>
            <td className="num">{bytes(run.bytes)}</td>
            <td>
              {run.gaps.length === 0 ? (
                <span className="muted">—</span>
              ) : (
                <span className="badge warn" title={run.gaps.join("\n")}>
                  {t("certification.gapCount", { count: run.gaps.length })}
                </span>
              )}
            </td>
            <td>
              <span
                title={run.attempts
                  .map((attempt) => `${dateTime(attempt.at_unix_ms)}: ${attempt.ok ? t("certification.state.delivered") : (attempt.error ?? "")}`)
                  .join("\n")}
              >
                {run.attempts.length}
              </span>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function DeliveryForm({ delivery, onDone }: Readonly<{ delivery: CertificationDelivery | null; onDone: () => void }>) {
  const servers = useServers().data ?? [];
  const today = useToday();
  const stationId = useStation();
  const stations = useQuery({ queryKey: ["stations"], queryFn: () => unwrap(api.GET("/api/v1/stations")) }).data;
  const code = stations?.find((station) => station.id === stationId)?.code ?? "station";
  const [draft, setDraft] = useState<DeliveryDraft>(deliveryDraftOf(delivery, servers[0]?.id ?? null));
  const set = (change: Partial<DeliveryDraft>) => setDraft((current) => ({ ...current, ...change }));
  const save = useAction(
    (body: DeliveryDraft) =>
      delivery
        ? unwrap(
            api.PUT("/api/v1/stations/{station_id}/certification/deliveries/{id}", {
              params: { path: here({ id: delivery.id }) },
              body: deliveryInputOf(body),
            }),
          )
        : unwrap(
            api.POST("/api/v1/stations/{station_id}/certification/deliveries", {
              params: { path: here() },
              body: deliveryInputOf(body),
            }),
          ),
    { success: t("certification.saved"), invalidate: [["certification", "deliveries"]], onSuccess: onDone },
  );
  const preview = previewFileName(draft.fileName, {
    station: code,
    server: servers.find((server) => server.id === draft.serverId)?.name ?? "",
    mount: draft.mount,
    day: addDays(today, -draft.daysBack),
    format: draft.format,
    compression: draft.compression,
  });
  return (
    <form
      className="form-grid"
      aria-label={t("certification.deliveryForm")}
      onSubmit={(e) => {
        e.preventDefault();
        save.mutate(draft);
      }}
    >
      <Field label={t("certification.deliveryName")} hint={t("certification.deliveryNameHint")}>
        <input className="input" value={draft.name} required onChange={(e) => set({ name: e.target.value })} />
      </Field>
      <Field label={t("certification.server")}>
        <select className="input" value={draft.serverId ?? ""} onChange={(e) => set({ serverId: Number(e.target.value) })}>
          {servers.map((server) => (
            <option key={server.id} value={server.id}>
              {server.name}
            </option>
          ))}
        </select>
      </Field>
      <Field label={t("certification.mount")}>
        <input className="input" value={draft.mount} placeholder={t("certification.allMounts")} onChange={(e) => set({ mount: e.target.value })} />
      </Field>
      <Field label={t("certification.host")} hint={t("certification.hostHint")}>
        <input className="input" value={draft.host} required placeholder="ftp.example.org" onChange={(e) => set({ host: e.target.value })} />
      </Field>
      <Field label={t("certification.port")}>
        <input className="input" inputMode="numeric" value={draft.port} onChange={(e) => set({ port: e.target.value })} />
      </Field>
      <Field label={t("certification.username")}>
        <input className="input" value={draft.username} required autoComplete="off" onChange={(e) => set({ username: e.target.value })} />
      </Field>
      <Field label={t("certification.password")} hint={delivery?.password_set ? t("certification.passwordKept") : undefined}>
        <input
          className="input"
          type="password"
          autoComplete="new-password"
          value={draft.password}
          onChange={(e) => set({ password: e.target.value })}
        />
      </Field>
      <Field label={t("certification.remoteDir")} hint={t("certification.remoteDirHint")}>
        <input className="input" value={draft.remoteDir} placeholder="/incoming" onChange={(e) => set({ remoteDir: e.target.value })} />
      </Field>
      <Field
        label={t("certification.fileName")}
        hint={
          preview === null
            ? t("certification.fileNameInvalid", { tokens: FILE_NAME_TOKENS.join(" ") })
            : t("certification.fileNamePreview", { name: preview, tokens: FILE_NAME_TOKENS.join(" ") })
        }
      >
        <input className="input" value={draft.fileName} onChange={(e) => set({ fileName: e.target.value })} />
      </Field>
      <Field label={t("certification.format")}>
        <Segmented
          label={t("certification.format")}
          value={draft.format}
          options={FORMATS.map((format) => ({ value: format, label: format }))}
          onChange={(format) => set({ format })}
        />
      </Field>
      <Field label={t("certification.compression")}>
        <Segmented
          label={t("certification.compression")}
          value={draft.compression}
          options={[
            { value: "none", label: t("certification.noCompression") },
            { value: "gzip", label: "gzip" },
          ]}
          onChange={(compression) => set({ compression })}
        />
      </Field>
      <Field label={t("certification.sendAt")} hint={t("certification.sendAtHint")}>
        <input className="input" type="time" value={draft.sendAt} required onChange={(e) => set({ sendAt: e.target.value })} />
      </Field>
      <Field label={t("certification.daySent")}>
        <select className="input" value={draft.daysBack} onChange={(e) => set({ daysBack: Number(e.target.value) })}>
          {DAYS_BACK.map((count) => (
            <option key={count} value={count}>
              {count === 1 ? t("certification.dayBefore") : t("certification.daysBefore", { count })}
            </option>
          ))}
        </select>
      </Field>
      <label className="row" style={{ gap: "0.5rem" }}>
        <input type="checkbox" checked={draft.enabled} onChange={(e) => set({ enabled: e.target.checked })} />
        {t("certification.deliveryEnabled")}
      </label>
      <div className="row" style={{ gap: "0.5rem" }}>
        <button type="submit" className="btn primary" disabled={save.isPending || preview === null}>
          {t("common.save")}
        </button>
        <button type="button" className="btn" onClick={onDone}>
          {t("common.cancel")}
        </button>
      </div>
    </form>
  );
}
