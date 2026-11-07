// SPDX-License-Identifier: AGPL-3.0-or-later

// Dedications (docs/listener-interaction.md#dedications): the requests of
// the range as they come, what the rules say about each, accepted into the
// queue or refused; the show's settings; the dedication to read on air.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { api, unwrap } from "../api";
import { number, t } from "../i18n";
import {
  type Dedication,
  type DedicationSettings,
  type DedicationStatus,
  type Place,
  type Reason,
  REASONS,
  acceptable,
  checkBadge,
  reasonLabel,
} from "../lib/dedications";
import { ago, shortTime } from "../lib/format";
import { useModule } from "../lib/modules";
import { Link } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { here, useStation } from "../lib/station";
import { ShowPage } from "../pages/Show";
import { Card, Empty, ErrorView, Field, Loading, NumberInput, Segmented } from "./ui";

type Tab = "requests" | "settings";

/** Shows › Dedications: the requests, then the settings every show has. */
export function DedicationsPage() {
  const [tab, setTab] = useState<Tab>("requests");
  const tabs = (
    <>
      <Segmented
        value={tab}
        label={t("dedications.tabs")}
        onChange={setTab}
        options={[
          { value: "requests", label: t("dedications.tab.requests") },
          { value: "settings", label: t("dedications.tab.settings") },
        ]}
      />
      {tab === "requests" && <Requests />}
      {tab === "settings" && <SettingsCard />}
    </>
  );
  return <ShowPage module="dedications" tabs={tabs} common={tab === "settings"} />;
}

function useDedications(enabled = true) {
  const station = useStation();
  return useQuery({
    queryKey: ["dedications", "list", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/dedications", { params: { path: here() } })),
    enabled: enabled && station !== null,
    refetchInterval: 5_000,
  });
}

type Filter = "pending" | "accepted" | "refused";

function inFilter(status: DedicationStatus, filter: Filter): boolean {
  return filter === "accepted" ? status === "accepted" || status === "played" : status === filter;
}

/** The range's requests, live. */
export function Requests() {
  const list = useDedications();
  const [filter, setFilter] = useState<Filter>("pending");
  if (list.isPending) {
    return <Loading />;
  }
  if (list.isError) {
    return <ErrorView error={list.error} />;
  }
  const data = list.data;
  const counts = data.counts;
  const now = Date.now();
  const open =
    data.opens_at_unix_ms !== null &&
    data.opens_at_unix_ms !== undefined &&
    data.closes_at_unix_ms !== null &&
    data.closes_at_unix_ms !== undefined &&
    data.opens_at_unix_ms <= now &&
    now < data.closes_at_unix_ms;
  const shown = data.dedications.filter((dedication) => inFilter(dedication.status, filter));
  const accepted = counts.accepted + counts.played;
  return (
    <>
      <p className={open ? "small dedication-open" : "small muted"}>
        {open && data.closes_at_unix_ms
          ? t("dedications.openUntil", { time: shortTime(data.closes_at_unix_ms) })
          : t("dedications.closedNow")}
      </p>
      <Card>
        <div className="row" style={{ gap: "2rem", flexWrap: "wrap" }}>
          <Tile value={number(counts.pending)} label={t("dedications.count.pending", { count: counts.pending })} />
          <Tile value={number(accepted)} label={t("dedications.count.accepted", { count: accepted })} />
          <Tile value={number(counts.refused)} label={t("dedications.count.refused", { count: counts.refused })} />
          <Tile
            value={`${number(counts.received)} / ${number(data.max_per_range)}`}
            label={t("dedications.count.received", { count: counts.received })}
          />
        </div>
      </Card>
      <Card
        title={t(`dedications.filter.${filter}`)}
        actions={
          <Segmented
            value={filter}
            label={t("dedications.tabs")}
            onChange={setFilter}
            options={[
              { value: "pending", label: `${t("dedications.filter.pending")} (${number(counts.pending)})` },
              { value: "accepted", label: `${t("dedications.filter.accepted")} (${number(accepted)})` },
              { value: "refused", label: `${t("dedications.filter.refused")} (${number(counts.refused)})` },
            ]}
          />
        }
        flush
      >
        {shown.length === 0 ? (
          <Empty>{t(`dedications.empty.${filter}`)}</Empty>
        ) : (
          <ul className="dedication-list">
            {shown.map((dedication) => (
              <Request key={dedication.id} dedication={dedication} now={now} />
            ))}
          </ul>
        )}
      </Card>
    </>
  );
}

function Tile({ value, label }: Readonly<{ value: string; label: string }>) {
  return (
    <div className="stat">
      <span className="value">{value}</span>
      <span className="label">{label}</span>
    </div>
  );
}

function trackLabel(dedication: Dedication): string {
  return [dedication.artist, dedication.title].filter(Boolean).join(" – ") || t("dedications.trackGone");
}

function Request({ dedication, now }: Readonly<{ dedication: Dedication; now: number }>) {
  const canEdit = useCan("operator");
  const [reason, setReason] = useState<Reason | null>(null);
  const accept = useAction(
    (place: Place) =>
      unwrap(
        api.POST("/api/v1/stations/{station_id}/dedications/{id}/accept", {
          params: { path: here({ id: dedication.id }) },
          body: { place },
        }),
      ),
    { success: t("dedications.accepted"), invalidate: [["dedications"], ["queue"]] },
  );
  const refuse = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/stations/{station_id}/dedications/{id}/refuse", {
          params: { path: here({ id: dedication.id }) },
          body: { reason },
        }),
      ),
    { success: t("dedications.refused"), invalidate: [["dedications"]] },
  );
  const busy = accept.isPending || refuse.isPending;
  const canAccept = acceptable(dedication);
  return (
    <li className="dedication">
      <div className="stack" style={{ gap: 6, minWidth: 0 }}>
        <div>
          {dedication.track_id ? (
            <Link to={`/library/${dedication.track_id}`}>
              <strong>{trackLabel(dedication)}</strong>
            </Link>
          ) : (
            <strong>{trackLabel(dedication)}</strong>
          )}{" "}
          <span className="muted small">{ago(dedication.submitted_at_unix_ms, now)}</span>
        </div>
        <div className="small">
          {t("dedications.forFrom", { to: dedication.to, from: dedication.from })}
        </div>
        {dedication.message && <blockquote className="dedication-message">{dedication.message}</blockquote>}
        {dedication.status === "pending" && (
          <div className="row" style={{ flexWrap: "wrap", gap: 6 }}>
            {dedication.checks.map((check, index) => {
              const badge = checkBadge(check, shortTime);
              return (
                <span key={index} className={`badge ${badge.tone}`}>
                  {badge.text}
                </span>
              );
            })}
          </div>
        )}
        {dedication.status !== "pending" && <Decided dedication={dedication} />}
      </div>
      {dedication.status === "pending" && canEdit && (
        <div className="stack dedication-actions">
          <div className="row" style={{ flexWrap: "wrap", gap: 6, justifyContent: "flex-end" }}>
            <button
              type="button"
              className="btn primary small"
              disabled={!canAccept || busy}
              onClick={() => accept.mutate("next")}
            >
              {t("dedications.acceptNext")}
            </button>
            <button type="button" className="btn small" disabled={!canAccept || busy} onClick={() => accept.mutate("hour")}>
              {t("dedications.acceptHour")}
            </button>
            <button type="button" className="btn small" disabled={!canAccept || busy} onClick={() => accept.mutate("best")}>
              {t("dedications.acceptBest")}
            </button>
          </div>
          <div className="row" style={{ gap: 6, justifyContent: "flex-end" }}>
            <select
              className="input"
              aria-label={t("dedications.reason")}
              value={reason ?? ""}
              onChange={(e) => setReason(e.target.value === "" ? null : (e.target.value as Reason))}
            >
              {REASONS.map((option) => (
                <option key={option.value ?? ""} value={option.value ?? ""}>
                  {t(option.label)}
                </option>
              ))}
            </select>
            <button type="button" className="btn small danger" disabled={busy} onClick={() => refuse.mutate(undefined)}>
              {t("dedications.refuse")}
            </button>
          </div>
        </div>
      )}
    </li>
  );
}

/** What became of a request. */
function Decided({ dedication }: Readonly<{ dedication: Dedication }>) {
  const by = dedication.decided_by ? t("dedications.by", { user: dedication.decided_by }) : "";
  if (dedication.status === "played" && dedication.played_at_unix_ms) {
    return (
      <span className="small">
        <span className="badge good">{t("dedications.status.played")}</span>{" "}
        {t("dedications.playedAt", { time: shortTime(dedication.played_at_unix_ms) })} {by}
      </span>
    );
  }
  if (dedication.status === "accepted") {
    return (
      <span className="small">
        <span className="badge accent">{t("dedications.status.accepted")}</span>{" "}
        {dedication.planned_at_unix_ms
          ? t("dedications.plannedAt", { time: shortTime(dedication.planned_at_unix_ms) })
          : ""}{" "}
        {by}
      </span>
    );
  }
  return (
    <span className="small">
      <span className="badge">{t("dedications.status.refused")}</span> {t(reasonLabel(dedication.reason))} {by}
    </span>
  );
}

function SettingsCard() {
  const station = useStation();
  const settings = useQuery({
    queryKey: ["dedications", "settings", station],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/dedications/settings", { params: { path: here() } })),
    enabled: station !== null,
  });
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return <SettingsForm initial={settings.data} />;
}

function SettingsForm({ initial }: Readonly<{ initial: DedicationSettings }>) {
  const canEdit = useCan("operator");
  const station = useStation();
  const [draft, setDraft] = useState<DedicationSettings>(initial);
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const save = useAction(
    (body: DedicationSettings) =>
      unwrap(api.PUT("/api/v1/stations/{station_id}/dedications/settings", { params: { path: here() }, body })),
    { success: t("categories.saved"), invalidate: [["dedications"]] },
  );
  const ours = (categories.data ?? []).filter((category) => station !== null && category.station_ids.includes(station));
  const set = (change: Partial<DedicationSettings>) => setDraft((current) => ({ ...current, ...change }));
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial);
  const numberField = (
    label: string,
    value: number,
    min: number,
    max: number,
    onChange: (value: number) => void,
    suffix?: string,
    hint?: string,
  ) => (
    <Field label={label} {...(hint ? { hint } : {})}>
      <NumberInput
        value={value}
        min={min}
        max={max}
        step={1}
        disabled={!canEdit}
        label={label}
        {...(suffix ? { suffix } : {})}
        onChange={onChange}
      />
    </Field>
  );
  return (
    <Card
      title={t("dedications.settings")}
      actions={
        canEdit && (
          <button type="button" className="btn primary" disabled={!dirty || save.isPending} onClick={() => save.mutate(draft)}>
            {t("common.save")}
          </button>
        )
      }
    >
      <div className="form-grid">
        {numberField(
          t("dedications.maxPerRange"),
          draft.max_per_range,
          1,
          1000,
          (max_per_range) => set({ max_per_range }),
          undefined,
          t("dedications.maxPerRangeHint"),
        )}
        {numberField(
          t("dedications.perDevice"),
          draft.per_device,
          1,
          10,
          (per_device) => set({ per_device }),
          t("dedications.perRange"),
        )}
        {numberField(
          t("dedications.messageMax"),
          draft.message_max,
          20,
          500,
          (message_max) => set({ message_max }),
          t("dedications.characters"),
        )}
        {numberField(
          t("dedications.retention"),
          draft.retention_days,
          1,
          365,
          (retention_days) => set({ retention_days }),
          t("dedications.days"),
        )}
      </div>
      <div className="field">
        <span>{t("dedications.requestable")}</span>
        <span className="row" style={{ flexWrap: "wrap" }}>
          {ours.map((category) => (
            <label key={category.id} className="check">
              <input
                type="checkbox"
                disabled={!canEdit}
                checked={draft.category_ids.includes(category.id)}
                onChange={(e) =>
                  set({
                    category_ids: e.target.checked
                      ? [...draft.category_ids, category.id]
                      : draft.category_ids.filter((id) => id !== category.id),
                  })
                }
              />
              {category.name}
            </label>
          ))}
        </span>
        <span className="muted small">{t("dedications.requestableHint")}</span>
      </div>
    </Card>
  );
}

/** On air: the dedication of the track playing, for a host to read. */
export function DedicationOnAir() {
  const on = useModule("dedications");
  const list = useDedications(on);
  const dedication = list.data?.on_air;
  if (!on || !dedication) {
    return null;
  }
  return (
    <Card title={t("dedications.onAir")}>
      <p className="small">{t("dedications.forFrom", { to: dedication.to, from: dedication.from })}</p>
      {dedication.message && <blockquote className="dedication-message">{dedication.message}</blockquote>}
    </Card>
  );
}
