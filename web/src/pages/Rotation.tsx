// SPDX-License-Identifier: AGPL-3.0-or-later

// The selection log: every track rotation queued, and why. And simulations
// of the programming to come, checked rule by rule.

import { useStations } from "../components/Stations";
import { codes, here } from "../lib/station";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { Fragment, useState } from "react";
import { type Explanation, type RotationPick, type SimulationReport, api, query, unwrap } from "../api";
import { Banner, Card, Empty, ErrorView, Field, Loading, PageHead, Segmented } from "../components/ui";
import { number, rich, t } from "../i18n";
import { ago, clockTime, dateTime } from "../lib/format";
import { describeWindows, methodLabel, stepLabel } from "../lib/rotation";
import { Link } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { useCategories } from "./Clocks";

export function Rotation() {
  const canSimulate = useCan("operator");
  return (
    <>
      <PageHead
        title={t("nav.rotation")}
        description={t("rotationPage.description")}
      />
      {canSimulate && <SimulationCard />}
      <PicksCard />
    </>
  );
}

function StepBadge({ explanation }: { explanation: Explanation }) {
  if (!explanation.enforce_rules) {
    return <span className="badge">{t("rotationPage.rulesOff")}</span>;
  }
  if (explanation.step === 0) {
    return <span className="badge good">{t("rotationPage.rulesHeld")}</span>;
  }
  return (
    <span className="badge warn" title={stepLabel(explanation.step)}>
      {t("rotationPage.relaxed", { step: stepLabel(explanation.step) })}
    </span>
  );
}

function Where({ explanation }: { explanation: Explanation }) {
  const categories = useCategories();
  const name = (id: number | null | undefined) =>
    id === null || id === undefined ? undefined : categories.data?.find((c) => c.id === id)?.name;
  const slotCategory = name(explanation.slot_category_id) ?? explanation.category_name ?? t("rotationPage.wholeLibrary");
  // The default schedule has no name: only the others are named.
  const schedule = explanation.schedule_name ? (
    <span className="muted"> · {t("rotationPage.schedule", { name: explanation.schedule_name })}</span>
  ) : null;
  if (explanation.clock_id === null || explanation.clock_id === undefined) {
    return (
      <span>
        <span className="muted">{t("rotationPage.noClock")}</span>
        {schedule}
      </span>
    );
  }
  return (
    <span>
      {rich("rotationPage.where", {
        clock: <Link to={`/clocks/${explanation.clock_id}`}>{explanation.clock_name ?? t("rotationPage.clock")}</Link>,
        slot: (explanation.slot ?? 0) + 1,
        category: slotCategory,
      })}
      {explanation.step >= 9 && (
        <span className="muted"> → {explanation.category_name ?? t("rotationPage.wholeLibrary")}</span>
      )}
      {schedule}
    </span>
  );
}

function Details({ explanation }: { explanation: Explanation }) {
  const stations = useStations();
  const blocked = explanation.blocked;
  const held = [
    blocked.track && t("rotationPage.blockedTrack", { count: blocked.track }),
    blocked.title && t("rotationPage.blockedTitle", { count: blocked.title }),
    blocked.artist && t("rotationPage.blockedArtist", { count: blocked.artist }),
    blocked.album && t("rotationPage.blockedAlbum", { count: blocked.album }),
    blocked.across && t("rotationPage.blockedAcross", { count: blocked.across }),
  ].filter(Boolean);
  const elsewhere = explanation.held_elsewhere;
  const across = explanation.across ?? { artist_min: 0, title_min: 0, track_min: 0 };
  const acrossWindows = [
    across.artist_min && `${t("separation.artist")} ${across.artist_min} ${t("common.min")}`,
    across.title_min && `${t("separation.title")} ${across.title_min} ${t("common.min")}`,
    across.track_min && `${t("separation.track")} ${across.track_min} ${t("common.min")}`,
  ].filter(Boolean);
  return (
    <dl className="facts small">
      <dt>{t("rotationPage.method")}</dt>
      <dd>{methodLabel(explanation.method)}</dd>
      <dt>{t("rotation.target.category")}</dt>
      <dd>
        {t("common.tracks", { count: explanation.pool })}
        {explanation.restricted > 0 && `, ${t("rotationPage.restricted", { count: explanation.restricted })}`}
      </dd>
      {held.length > 0 && (
        <>
          <dt>{t("rotationPage.heldBack")}</dt>
          <dd>{held.join(", ")}</dd>
        </>
      )}
      {elsewhere && (
        <>
          <dt>{t("rotationPage.heldElsewhere")}</dt>
          <dd>
            {t(elsewhere.planned ? "rotationPage.heldByQueue" : "rotationPage.heldByPlay", {
              station: codes([elsewhere.station_id], stations).join("") || `#${elsewhere.station_id}`,
              time: clockTime(elsewhere.at_unix_ms),
            })}
          </dd>
        </>
      )}
      {acrossWindows.length > 0 && (
        <>
          <dt>{t("rotationPage.acrossWindows")}</dt>
          <dd>{acrossWindows.join(" · ")}</dd>
        </>
      )}
      <dt>{t("rotationPage.chosenAmong")}</dt>
      <dd>{t("common.tracks", { count: explanation.candidates })}</dd>
      <dt>{t("rotationPage.windows")}</dt>
      <dd>{describeWindows(explanation.windows)}</dd>
      <dt>{t("library.lastPlayed")}</dt>
      <dd>
        {ago(explanation.last_played_unix_ms || null)}
        {explanation.method === "weighted" &&
          ` · ${t("rotationPage.weight", { weight: number(explanation.weight, { minimumFractionDigits: 2, maximumFractionDigits: 2 }) })}`}
      </dd>
      {explanation.skipped_slots.length > 0 && (
        <>
          <dt>{t("history.skipped")}</dt>
          <dd>
            {t("rotationPage.skippedSlots", {
              slots: explanation.skipped_slots
                .map((slot) =>
                  slot.reason === "same_type_in_a_row"
                    ? t("rotationPage.slotInARow", { slot: slot.slot + 1 })
                    : t("rotationPage.slot", { slot: slot.slot + 1 }),
                )
                .join(", "),
            })}
          </dd>
        </>
      )}
    </dl>
  );
}

function PicksCard() {
  const [before, setBefore] = useState<number[]>([]);
  const [open, setOpen] = useState<number | null>(null);
  const cursor = before[before.length - 1];
  const picks = useQuery({
    queryKey: ["rotation-picks", cursor],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/rotation/picks", { params: { path: here(), query: query({ before: cursor, limit: 50 }) } })),
    placeholderData: keepPreviousData,
    refetchInterval: cursor === undefined ? 15_000 : false,
  });
  const rows: RotationPick[] = picks.data ?? [];
  const last = rows[rows.length - 1];
  return (
    <Card title={t("rotationPage.picks")} flush>
      {picks.isPending && <Loading />}
      {picks.isError && <ErrorView error={picks.error} />}
      {picks.data?.length === 0 && <Empty>{t("rotationPage.nothingPicked")}</Empty>}
      {rows.length > 0 && (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>{t("rotationPage.planned")}</th>
                <th>{t("history.track")}</th>
                <th>{t("review.from")}</th>
                <th>{t("rotationPage.rules")}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((pick) => (
                <Fragment key={pick.id}>
                  <tr onClick={() => setOpen(open === pick.id ? null : pick.id)} style={{ cursor: "pointer" }}>
                    <td className="num small">
                      {dateTime(pick.planned_at_unix_ms)}
                      {!pick.played_at_unix_ms && <div className="muted">{t("rotationPage.notPlayed")}</div>}
                    </td>
                    <td className="title-cell">
                      {pick.track_id ? (
                        <Link to={`/library/${pick.track_id}`} onClick={(event) => event.stopPropagation()}>
                          {pick.artist ?? "?"} – {pick.title ?? "?"}
                        </Link>
                      ) : (
                        <span className="muted">{t("rotationPage.deletedTrack")}</span>
                      )}
                    </td>
                    <td className="small">
                      <Where explanation={pick.explanation} />
                    </td>
                    <td>
                      <StepBadge explanation={pick.explanation} />
                    </td>
                  </tr>
                  {open === pick.id && (
                    <tr>
                      <td colSpan={4}>
                        <Details explanation={pick.explanation} />
                      </td>
                    </tr>
                  )}
                </Fragment>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {(before.length > 0 || rows.length === 50) && (
        <div className="pager">
          <button
            type="button"
            className="btn small"
            disabled={before.length === 0}
            onClick={() => setBefore(before.slice(0, -1))}
          >
            {t("rotationPage.newer")}
          </button>
          <button
            type="button"
            className="btn small"
            disabled={rows.length < 50 || !last}
            onClick={() => last && setBefore([...before, last.id])}
          >
            {t("rotationPage.older")}
          </button>
        </div>
      )}
    </Card>
  );
}

type Span = "24" | "168";

function SimulationCard() {
  const [hours, setHours] = useState<Span>("168");
  const [report, setReport] = useState<SimulationReport | null>(null);
  const simulate = useAction(
    () => unwrap(api.POST("/api/v1/stations/{station_id}/rotation/simulate", { params: { path: here() }, body: { hours: Number(hours) } })),
    { onSuccess: setReport },
  );
  return (
    <Card
      title={t("clocks.simulation")}
      actions={
        <>
          <Segmented
            label={t("library.length")}
            value={hours}
            options={[
              { value: "24", label: t("rotationPage.aDay") },
              { value: "168", label: t("rotationPage.aWeek") },
            ]}
            onChange={setHours}
          />
          <button
            type="button"
            className="btn small primary"
            disabled={simulate.isPending}
            onClick={() => simulate.mutate(undefined)}
          >
            {simulate.isPending ? t("clocks.simulating") : t("rotationPage.simulate")}
          </button>
        </>
      }
    >
      {!report ? (
        <p className="muted small" style={{ margin: 0 }}>
          {t("rotationPage.simulateHelp")}
        </p>
      ) : (
        <SimulationSummary report={report} />
      )}
    </Card>
  );
}

function SimulationSummary({ report }: { report: SimulationReport }) {
  const simulation = report.simulation;
  const [shown, setShown] = useState(100);
  const names = new Map(report.tracks.map((track) => [track.id, track]));
  const picks = simulation.picks.length;
  const relaxed = simulation.steps.slice(1).reduce((sum, n) => sum + n, 0);
  const off = simulation.picks.filter((pick) => !pick.explanation.enforce_rules).length;
  return (
    <div className="stack">
      <div className="grid three">
        <div className="stat">
          <span className="value">{number(picks)}</span>
          <span className="label">{t("rotationPage.tracksPicked", { count: picks })}</span>
        </div>
        <div className="stat">
          <span className="value">
            {simulation.violations.length === 0 ? t("rotationPage.noneBroken") : number(simulation.violations.length)}
          </span>
          <span className="label">{t("rotationPage.rulesBroken", { count: simulation.violations.length })}</span>
        </div>
        <div className="stat">
          <span className="value">
            {picks ? number(relaxed / picks, { style: "percent", minimumFractionDigits: 1, maximumFractionDigits: 1 }) : "–"}
          </span>
          <span className="label">{t("rotationPage.relaxations")}</span>
        </div>
      </div>
      {simulation.violations.length > 0 && (
        <Banner kind="bad">
          {t("rotationPage.violations", { count: simulation.violations.length, seed: String(report.seed) })}
        </Banner>
      )}
      {simulation.gap_minutes > 0 && (
        <Banner kind="warn">
          {t("rotationPage.gap", { count: simulation.gap_minutes })}
        </Banner>
      )}
      <Field label={t("rotationPage.steps")}>
        <ul className="steps">
          {simulation.steps.map((n, step) =>
            n === 0 ? null : (
              <li key={step} title={stepLabel(step)}>
                <span className="num">{step}</span>
                <span className="bar" style={{ width: `${Math.max(1, (n / picks) * 100)}%` }} />
                <span className="small muted">
                  {step === 0 ? t("rotationPage.withEveryRule", { count: n }) : number(n)}
                </span>
              </li>
            ),
          )}
        </ul>
      </Field>
      {off > 0 && <p className="muted small">{t("rotationPage.off", { count: off })}</p>}
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>{t("rotationPage.time")}</th>
              <th>{t("history.track")}</th>
              <th>{t("review.from")}</th>
              <th>{t("rotationPage.rules")}</th>
            </tr>
          </thead>
          <tbody>
            {simulation.picks.slice(0, shown).map((pick, index) => {
              const track = names.get(pick.track_id);
              return (
                <tr key={index}>
                  <td className="num small">{dateTime(pick.start_unix_ms)}</td>
                  <td className="title-cell">
                    <Link to={`/library/${pick.track_id}`}>
                      {track ? `${track.artist ?? "?"} – ${track.title ?? "?"}` : t("clocks.trackNumber", { id: String(pick.track_id) })}
                    </Link>
                  </td>
                  <td className="small">
                    <Where explanation={pick.explanation} />
                  </td>
                  <td>
                    <StepBadge explanation={pick.explanation} />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {shown < picks && (
        <div className="pager">
          <span className="muted small">
            {t("rotationPage.shown", { shown, total: picks })}
          </span>
          <button type="button" className="btn small" onClick={() => setShown(shown + 200)}>
            {t("review.showMore")}
          </button>
        </div>
      )}
    </div>
  );
}
