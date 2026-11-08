// SPDX-License-Identifier: AGPL-3.0-or-later

// The match (docs/listener-interaction.md#match): the match in progress,
// to close its vote or cancel it, here and on On air; the matches played;
// the show's settings.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { api, unwrap } from "../api";
import { number, t } from "../i18n";
import { dateTime, shortTime } from "../lib/format";
import { type MatchSettings, type MatchView, along, clock, left, outcome, sides, useCurrentMatch as useCurrent } from "../lib/match";
import { useModule } from "../lib/modules";
import { useAction, useCan } from "../lib/session";
import { here, useStation } from "../lib/station";
import { ShowPage } from "../pages/Show";
import { Card, ConfirmButton, Empty, ErrorView, Field, Loading, NumberInput, Segmented } from "./ui";

type Tab = "current" | "history" | "settings";

/** Shows › Match: the match in progress, the history, then the settings every show has. */
export function MatchPage() {
  const [tab, setTab] = useState<Tab>("current");
  const tabs = (
    <>
      <Segmented
        value={tab}
        label={t("match.tabs")}
        onChange={setTab}
        options={[
          { value: "current", label: t("match.tab.current") },
          { value: "history", label: t("match.tab.history") },
          { value: "settings", label: t("match.tab.settings") },
        ]}
      />
      {tab === "current" && <CurrentMatch empty />}
      {tab === "history" && <History />}
      {tab === "settings" && <SettingsCard />}
    </>
  );
  return <ShowPage module="match" tabs={tabs} common={tab === "settings"} />;
}

function useSettings(enabled = true) {
  const station = useStation();
  return useQuery({
    queryKey: ["match", "settings", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/match/settings", { params: { path: here() } })),
    enabled: enabled && station !== null,
  });
}

/** The time now, every half second. */
function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(timer);
  }, []);
  return now;
}

function trackName(track: { artist?: string | null; title?: string | null }): string {
  return [track.artist, track.title].filter(Boolean).join(" – ");
}

/** The match in progress; with `empty`, says so when there is none. */
export function CurrentMatch({ empty = false, compact = false }: Readonly<{ empty?: boolean; compact?: boolean }>) {
  const current = useCurrent();
  const settings = useSettings();
  if (current.isPending) {
    return empty ? <Loading /> : null;
  }
  if (current.isError) {
    return empty ? <ErrorView error={current.error} /> : null;
  }
  const match = current.data;
  if (!match) {
    if (!empty) {
      return null;
    }
    return (
      <Card>
        <Empty>
          {t("match.none")}{" "}
          {settings.data && settings.data.tracks_from !== "clock" && !settings.data.category_id
            ? t("match.noCategory")
            : t("match.noneNext")}
        </Empty>
      </Card>
    );
  }
  return <MatchCard match={match} every={settings.data?.every_tracks ?? null} compact={compact} />;
}

function MatchCard({ match, every, compact }: Readonly<{ match: MatchView; every: number | null; compact: boolean }>) {
  const canEdit = useCan("operator");
  const now = useNow();
  const close = useAction(
    () => unwrap(api.POST("/api/v1/stations/{station_id}/match/current/close", { params: { path: here() } })),
    { invalidate: [["match"]] },
  );
  const cancel = useAction(
    () => unwrap(api.POST("/api/v1/stations/{station_id}/match/current/cancel", { params: { path: here() } })),
    { invalidate: [["match"]] },
  );
  const open = match.status === "open";
  const remaining = left(match.closes_at_unix_ms, now);
  const leader = Math.max(...match.choices.map((choice) => choice.votes));
  return (
    <Card
      title={
        <span className="row" style={{ gap: 8 }}>
          {t("match.current")}
          {every !== null && <span className="badge good">{t("match.every", { count: every })}</span>}
        </span>
      }
      actions={
        <span className="row" style={{ gap: 8, flexWrap: "wrap" }}>
          {open && remaining !== null ? (
            <span className={remaining > 0 && remaining <= 5_000 ? "match-clock hurry" : "match-clock"}>
              {remaining > 0 ? (
                <>
                  {t("match.closesIn")} <strong>{clock(remaining)}</strong>
                </>
              ) : (
                t("match.closed")
              )}
            </span>
          ) : (
            <span className="muted small">{t("match.queued")}</span>
          )}
          {canEdit && open && (
            <button type="button" className="btn small" disabled={close.isPending} onClick={() => close.mutate(undefined)}>
              {t("match.closeNow")}
            </button>
          )}
          {canEdit && (
            <ConfirmButton
              className="btn small danger"
              question={t("match.cancelConfirm")}
              confirm={t("match.cancel")}
              disabled={cancel.isPending}
              onConfirm={() => cancel.mutate(undefined)}
            >
              {t("match.cancel")}
            </ConfirmButton>
          )}
        </span>
      }
    >
      <div className={compact ? "match-duel compact" : "match-duel"}>
        {match.choices.map((choice, index) => (
          <div key={choice.choice} className="match-duel-cell">
            {index === 1 && <span className="match-versus">{t("match.versus")}</span>}
            <div className={choice.votes === leader && leader > 0 ? "match-choice leading" : "match-choice"}>
              {match.category_name && <span className="muted small">{t("match.categoryOf", { name: match.category_name })}</span>}
              <strong className="match-choice-title">{trackName(choice)}</strong>
              <div className="row" style={{ justifyContent: "space-between", alignItems: "baseline" }}>
                <span className="match-percent">{choice.percent} %</span>
                <span className="muted small">{t("match.votes", { count: choice.votes })}</span>
              </div>
              <div className={`match-bar side-${choice.choice}`}>
                <span style={{ width: `${choice.percent}%` }} />
              </div>
            </div>
          </div>
        ))}
      </div>
      <p className="small">
        <strong>{t("match.during", { track: trackName(match.vote_track) })}</strong>
      </p>
      {open && match.opened_at_unix_ms && match.closes_at_unix_ms && match.transition_at_unix_ms && (
        <Timeline
          start={match.opened_at_unix_ms}
          closes={match.closes_at_unix_ms}
          transition={match.transition_at_unix_ms}
          now={now}
          jingle={match.jingle !== null && match.jingle !== undefined}
        />
      )}
      <p className="muted small">{t("match.rules", { category: match.category_name ?? "…" })}</p>
    </Card>
  );
}

/** The vote's track from its start to the transition: now, the close. */
function Timeline({
  start,
  closes,
  transition,
  now,
  jingle,
}: Readonly<{ start: number; closes: number; transition: number; now: number; jingle: boolean }>) {
  const played = along(now, start, transition);
  const close = along(closes, start, transition);
  return (
    <div className="match-timeline">
      <div className="match-timeline-bar" aria-hidden="true">
        <span className="played" style={{ width: `${played}%` }} />
        <span className="close" style={{ left: `${close}%` }} />
        <span className="after" style={{ left: `${close}%` }} />
      </div>
      <div className="match-timeline-labels muted small">
        <span>{jingle ? t("match.timeline.jingle") : shortTime(start)}</span>
        <span>{t("match.timeline.now", { time: shortTime(now) })}</span>
        <span>{t("match.timeline.close", { time: shortTime(closes) })}</span>
        <span>{t("match.timeline.transition", { time: shortTime(transition) })}</span>
      </div>
    </div>
  );
}

/** On air: the match in progress, when there is one. */
export function MatchOnAir() {
  const on = useModule("match");
  if (!on) {
    return null;
  }
  return <CurrentMatch compact />;
}

export function History() {
  const station = useStation();
  const history = useQuery({
    queryKey: ["match", "history", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/match/history", { params: { path: here() } })),
    enabled: station !== null,
    refetchInterval: 15_000,
  });
  if (history.isPending) {
    return <Loading />;
  }
  if (history.isError) {
    return <ErrorView error={history.error} />;
  }
  const { matches, week } = history.data;
  return (
    <Card
      title={t("match.history")}
      actions={<span className="muted small">{t("match.week", { matches: number(week.matches), votes: number(week.votes) })}</span>}
      flush
    >
      {matches.length === 0 ? (
        <Empty>{t("match.historyEmpty")}</Empty>
      ) : (
        <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>{t("match.col.when")}</th>
              <th>{t("match.col.winner")}</th>
              <th>{t("match.col.loser")}</th>
              <th className="num">{t("match.col.votes")}</th>
            </tr>
          </thead>
          <tbody>
            {matches.map((match) => {
              const { winner, loser } = sides(match);
              const badge = outcome(match);
              const when = match.opened_at_unix_ms ?? match.closed_at_unix_ms;
              return (
                <tr key={match.id}>
                  <td>{when ? dateTime(when) : "…"}</td>
                  <td>
                    <strong>{winner ? trackName(winner) : match.choices.map(trackName).join(" / ")}</strong>{" "}
                    <span className={badge.tone === "muted" ? "badge" : `badge ${badge.tone}`}>{badge.text}</span>
                  </td>
                  <td className="muted">{loser ? trackName(loser) : ""}</td>
                  <td className="num">{number(match.votes)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
        </div>
      )}
    </Card>
  );
}

/** The match's own settings (the settings every show has aside). */
export function SettingsCard() {
  const settings = useSettings();
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return <SettingsForm initial={settings.data} />;
}

function SettingsForm({ initial }: Readonly<{ initial: MatchSettings }>) {
  const canEdit = useCan("operator");
  const station = useStation();
  const [draft, setDraft] = useState<MatchSettings>(initial);
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const save = useAction(
    (body: MatchSettings) =>
      unwrap(api.PUT("/api/v1/stations/{station_id}/match/settings", { params: { path: here() }, body })),
    { success: t("categories.saved"), invalidate: [["match"]] },
  );
  const ours = (categories.data ?? []).filter((category) => station !== null && category.station_ids.includes(station));
  const set = (change: Partial<MatchSettings>) => setDraft((current) => ({ ...current, ...change }));
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial);
  const category = (value: number | null | undefined, onChange: (id: number | null) => void, none: string, label: string) => (
    <select
      className="input"
      aria-label={label}
      value={value ?? ""}
      disabled={!canEdit}
      onChange={(e) => onChange(e.target.value === "" ? null : Number(e.target.value))}
    >
      <option value="">{none}</option>
      {ours.map((c) => (
        <option key={c.id} value={c.id}>
          {c.name}
        </option>
      ))}
    </select>
  );
  return (
    <Card
      title={t("match.settings")}
      actions={
        canEdit && (
          <button type="button" className="btn primary" disabled={!dirty || save.isPending} onClick={() => save.mutate(draft)}>
            {t("common.save")}
          </button>
        )
      }
    >
      <div className="form-grid">
        {/* Not a <label>: its buttons keep their own names. */}
        <div className="field">
          <span>{t("match.tracksFrom")}</span>
          <Segmented
            label={t("match.tracksFrom")}
            value={draft.tracks_from ?? "category"}
            disabled={!canEdit}
            options={[
              { value: "clock", label: t("match.fromClock") },
              { value: "category", label: t("match.fromCategory") },
            ]}
            onChange={(tracks_from) => set({ tracks_from })}
          />
          {draft.tracks_from === "clock" && <span className="hint">{t("match.fromClockHint")}</span>}
        </div>
        <Field label={t("match.everyTracks")} hint={t("match.everyTracksHint")}>
          <NumberInput
            value={draft.every_tracks}
            min={1}
            max={20}
            step={1}
            disabled={!canEdit}
            label={t("match.everyTracks")}
            suffix={t("match.tracksUnit")}
            onChange={(every_tracks) => set({ every_tracks })}
          />
        </Field>
        {draft.tracks_from === "clock" ? (
          <Field label={t("match.categoryClockLabel")} hint={t("match.categoryHint")}>
            {category(
              draft.category_id,
              (category_id) => set({ category_id }),
              t("match.noCategoryClockOption"),
              t("match.categoryClockLabel"),
            )}
          </Field>
        ) : (
          <Field label={t("match.categoryLabel")} hint={t("match.categoryHint")}>
            {category(draft.category_id, (category_id) => set({ category_id }), t("match.noCategoryOption"), t("match.categoryLabel"))}
          </Field>
        )}
        <Field label={t("match.jingle")} hint={t("match.jingleHint")}>
          {category(
            draft.jingle_category_id,
            (jingle_category_id) => set({ jingle_category_id }),
            t("match.noJingle"),
            t("match.jingle"),
          )}
        </Field>
        <Field label={t("match.announceEvery")}>
          <NumberInput
            value={draft.announce_every ?? 1}
            min={1}
            max={20}
            step={1}
            disabled={!canEdit}
            label={t("match.announceEvery")}
            suffix={t("match.matchesUnit")}
            onChange={(announce_every) => set({ announce_every })}
          />
        </Field>
        <Field label={t("match.closeBefore")}>
          <NumberInput
            value={draft.close_before_s}
            min={5}
            max={60}
            step={1}
            disabled={!canEdit}
            label={t("match.closeBefore")}
            suffix={t("match.secondsBefore")}
            onChange={(close_before_s) => set({ close_before_s })}
          />
        </Field>
      </div>
      <div className="row" style={{ gap: "2rem", flexWrap: "wrap" }}>
        <label className="check">
          <input
            type="checkbox"
            checked={draft.live_results}
            disabled={!canEdit}
            onChange={(e) => set({ live_results: e.target.checked })}
          />
          {t("match.liveResults")}
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={draft.loser_same_day}
            disabled={!canEdit}
            onChange={(e) => set({ loser_same_day: e.target.checked })}
          />
          {t("match.loserSameDay")}
        </label>
      </div>
    </Card>
  );
}
