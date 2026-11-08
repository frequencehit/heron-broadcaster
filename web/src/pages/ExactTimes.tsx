// SPDX-License-Identifier: AGPL-3.0-or-later

// The attempts page (docs/rotation.md#the-attempts-page): the exact times of
// a day and how each was met; for one, its calculations, their attempts best
// first on a timeline up to the window, and an attempt's tracks, ranking and
// fit search. An operator can use another attempt of the latest calculation
// that lands in the window, and go back to automatic.

import { useQuery } from "@tanstack/react-query";
import { type CSSProperties, useEffect, useState } from "react";
import { type Schemas, api, done, query, unwrap } from "../api";
import { Card, Empty, ErrorView, Loading, PageHead, Segmented } from "../components/ui";
import { number, rich, t } from "../i18n";
import {
  type Attempt,
  type AttemptFilter,
  type CalculationSummary,
  type ExactTimeOfDay,
  type ExactWindow,
  type Status,
  attemptInUse,
  blocks,
  calculationResult,
  cannotCalculate,
  chosenLabel,
  criteria,
  cutMs,
  defaultExactTime,
  endingStatus,
  filterAttempts,
  joined,
  lands,
  latestBuilt,
  metStatus,
  position,
  signedSeconds,
  spanOf,
  stopLabel,
  ticks,
  triggerLabel,
  verdict,
  windowOf,
} from "../lib/exactTimes";
import { clockTime, duration, shortTime } from "../lib/format";
import { renderDay } from "../lib/medley";
import { stepLabel } from "../lib/rotation";
import { Link, setSearch, useLocation } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { here } from "../lib/station";

type Detail = Schemas["CalculationDetail"];

/** The attempts shown before *show all*. */
const SHOWN = 6;

const TONE_CLASS: Record<Status["tone"], string> = {
  good: "badge good",
  warn: "badge warn",
  bad: "badge bad",
  muted: "badge",
};

function Pill({ status }: Readonly<{ status: Status }>) {
  return <span className={`${TONE_CLASS[status.tone]} exact-pill`}>{status.label}</span>;
}

function useCalculation(id: number | null | undefined) {
  return useQuery({
    queryKey: ["exact-calculation", id],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/exact-times/calculations/{id}", {
          params: { path: here({ id: id ?? 0 }) },
        }),
      ),
    enabled: id !== null && id !== undefined,
    refetchInterval: 10_000,
  });
}

export function ExactTimes() {
  const { search } = useLocation();
  const askedCalculation = Number(search.get("calculation")) || null;
  const askedAt = Number(search.get("at")) || null;
  const askedAttempt = Number(search.get("attempt")) || null;
  const day = search.get("day") ?? "";
  // Linked from the rotation log: a calculation alone, its day and exact time follow.
  const linked = useCalculation(askedCalculation !== null && askedAt === null ? askedCalculation : null);
  useEffect(() => {
    if (linked.data && askedAt === null) {
      setSearch({ day: renderDay(new Date(linked.data.exact_at_unix_ms)), at: linked.data.exact_at_unix_ms });
    }
  }, [linked.data, askedAt]);
  const days = useQuery({
    queryKey: ["exact-times", day],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/exact-times", {
          params: { path: here(), query: query({ day }) },
        }),
      ),
    refetchInterval: 10_000,
  });
  const exactTimes = days.data?.exact_times ?? [];
  const exact = defaultExactTime(exactTimes, askedAt);
  return (
    <>
      <PageHead title={t("nav.exactTimes")} description={t("exactTimes.description")} />
      {days.isError && <ErrorView error={days.error} />}
      {days.isPending && <Loading />}
      {days.data && (
        <div className="stack exact-page">
          <section aria-label={t("exactTimes.dayLabel")} className="row exact-days">
            <span className="muted small">
              {days.data.date === days.data.today ? t("exactTimes.today") : days.data.date}
            </span>
            {exactTimes.map((item) => {
              const status = metStatus(item);
              return (
                <button
                  key={item.at_unix_ms}
                  type="button"
                  className="exact-chip"
                  aria-pressed={item.at_unix_ms === exact?.at_unix_ms}
                  onClick={() => setSearch({ at: item.at_unix_ms, calculation: null, attempt: null })}
                >
                  {shortTime(item.at_unix_ms)} · <span className={`tone-${status.tone}`}>{status.label}</span>
                </button>
              );
            })}
            <label className="row small muted exact-day-picker">
              {t("exactTimes.day")}
              <input
                className="input"
                type="date"
                value={days.data.date}
                onChange={(event) => setSearch({ day: event.target.value, at: null, calculation: null, attempt: null })}
              />
            </label>
          </section>
          {exact ? (
            <ExactTime
              key={exact.at_unix_ms}
              exact={exact}
              exactTimes={exactTimes}
              choice={days.data.choice?.exact_at_unix_ms === exact.at_unix_ms ? days.data.choice : null}
              askedCalculation={askedCalculation}
              askedAttempt={askedAttempt}
            />
          ) : (
            <Card>
              <Empty>{t("exactTimes.none")}</Empty>
              {days.data.date === days.data.today && (
                <div className="row" style={{ justifyContent: "center" }}>
                  <CalculateNow reason={null} />
                </div>
              )}
            </Card>
          )}
        </div>
      )}
    </>
  );
}

function ExactTime({
  exact,
  exactTimes,
  choice,
  askedCalculation,
  askedAttempt,
}: Readonly<{
  exact: ExactTimeOfDay;
  exactTimes: readonly ExactTimeOfDay[];
  choice: Schemas["HandChoice"] | null;
  askedCalculation: number | null;
  askedAttempt: number | null;
}>) {
  const latest = latestBuilt(exact.calculations);
  const selected =
    exact.calculations.find((calculation) => calculation.id === askedCalculation) ?? latest ?? exact.calculations.at(-1);
  // A followed pick has no attempts: those of the calculation it follows.
  const shownId = selected?.trigger === "followed" ? (selected.follows_id ?? null) : (selected?.id ?? null);
  const detail = useCalculation(shownId);
  const latestDetail = useCalculation(latest?.id ?? null);
  return (
    <>
      <Summary exact={exact} latest={latest} latestDetail={latestDetail.data} choice={choice} />
      {choice && <HandChosen choice={choice} />}
      <div className="exact-columns">
        <Card title={t("exactTimes.calculationsFor", { time: shortTime(exact.at_unix_ms) })} className="exact-runs">
          <p className="muted small exact-help">{t("exactTimes.calculationsHelp")}</p>
          <CalculateNow exactAt={exact.at_unix_ms} reason={cannotCalculate(exact, exactTimes)} />
          {exact.calculations.length === 0 && <Empty>{t("exactTimes.noCalculations")}</Empty>}
          <div className="stack exact-run-list">
            {exact.calculations.map((calculation) => (
              <Run key={calculation.id} calculation={calculation} selected={calculation.id === selected?.id} />
            ))}
          </div>
        </Card>
        {selected && (
          <AttemptsCard
            exact={exact}
            selected={selected}
            detail={detail.data}
            pending={detail.isPending && shownId !== null}
            error={detail.error}
            askedAttempt={askedAttempt}
            choice={choice}
          />
        )}
      </div>
      {detail.data && selected && (
        <AttemptDetail
          detail={detail.data}
          attempt={pickAttempt(detail.data, askedAttempt, choice)}
          choice={choice}
        />
      )}
    </>
  );
}

/**
 * *Calculate now*: the fills of the upcoming exact time built on demand
 * (`exactAt`, or the upcoming one when missing), then the new calculation
 * shown. Disabled with `reason` for any other exact time.
 */
function CalculateNow({ exactAt, reason }: Readonly<{ exactAt?: number; reason: string | null }>) {
  const canCalculate = useCan("operator");
  const calculate = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/stations/{station_id}/exact-times/calculate", {
          params: { path: here() },
          body: exactAt === undefined ? {} : { exact_at_unix_ms: exactAt },
        }),
      ),
    {
      success: t("exactTimes.calculateDone"),
      invalidate: [["exact-times"]],
      onSuccess: (now) =>
        setSearch({
          day: renderDay(new Date(now.exact_at_unix_ms)),
          at: now.exact_at_unix_ms,
          calculation: now.id,
          attempt: null,
        }),
    },
  );
  if (!canCalculate) {
    return null;
  }
  return (
    <div className="stack exact-calculate">
      <button
        type="button"
        className="btn"
        disabled={reason !== null || calculate.isPending}
        title={reason ?? t("exactTimes.calculateHelp")}
        onClick={() => calculate.mutate(undefined)}
      >
        {calculate.isPending ? (
          <>
            <span className="spinner" aria-hidden="true" /> {t("exactTimes.calculating")}
          </>
        ) : (
          t("exactTimes.calculateNow")
        )}
      </button>
      <span className="muted small">{reason ?? t("exactTimes.calculateHelp")}</span>
    </div>
  );
}

/** The attempt to show: the one asked for, else the one in use (chosen by hand, or kept). */
function pickAttempt(detail: Detail, asked: number | null, choice: Schemas["HandChoice"] | null): Attempt | undefined {
  const inUse = attemptInUse(detail, choice) ?? detail.kept;
  return (
    detail.attempts.find((attempt) => attempt.number === asked) ??
    detail.attempts.find((attempt) => attempt.number === inUse) ??
    detail.attempts[0]
  );
}

function Summary({
  exact,
  latest,
  latestDetail,
  choice,
}: Readonly<{
  exact: ExactTimeOfDay;
  latest: CalculationSummary | undefined;
  latestDetail: Detail | undefined;
  choice: Schemas["HandChoice"] | null;
}>) {
  const window = windowOf(exact);
  const slot = exact.slot === null || exact.slot === undefined ? null : exact.slot + 1;
  const combinations = (latestDetail?.attempts ?? []).reduce(
    (sum, attempt) => sum + (attempt.search?.combinations ?? 0),
    0,
  );
  return (
    <section aria-label={t("exactTimes.summary")} className="exact-summary">
      <div className="card exact-tile">
        <span className="muted small">{t("exactTimes.exactTime")}</span>
        <strong>{clockTime(exact.at_unix_ms)}</strong>
        <span className="muted small">
          {t("exactTimes.window", {
            mode: exact.mode === "soft" ? t("exactTimes.modeSoft") : t("exactTimes.modeHard"),
            from: clockTime(window.from),
            cut: clockTime(window.cut),
          })}
          {slot !== null &&
            ` · ${exact.clock_name ? t("exactTimes.slotOf", { slot, clock: exact.clock_name }) : t("exactTimes.slot", { slot })}`}
        </span>
      </div>
      <div className="card exact-tile">
        <span className="muted small">{t("exactTimes.calculationKept")}</span>
        {latest ? (
          <>
            <strong>{clockTime(latest.computed_at_unix_ms)}</strong>
            <span className="muted small">{t("exactTimes.triggeredBy", { trigger: triggerLabel(latest) })}</span>
          </>
        ) : (
          <strong className="muted">{t("exactTimes.noCalculation")}</strong>
        )}
      </div>
      <div className="card exact-tile">
        <span className="muted small">{t("exactTimes.attempts")}</span>
        {latest ? (
          <>
            <strong>{t("exactTimes.fillsCount", { count: latestDetail?.attempts.length ?? latest.fills })}</strong>
            <span className="muted small">
              {t("exactTimes.attemptsDetail", {
                batches: t("exactTimes.batches", { count: latest.batches }),
                combinations: number(combinations),
                ms: number(latest.duration_ms),
                stop: stopLabel(latest),
              })}
            </span>
          </>
        ) : (
          <strong className="muted">–</strong>
        )}
      </div>
      <Result exact={exact} latest={latest} choice={choice} window={window} />
    </section>
  );
}

function Result({
  exact,
  latest,
  choice,
  window,
}: Readonly<{
  exact: ExactTimeOfDay;
  latest: CalculationSummary | undefined;
  choice: Schemas["HandChoice"] | null;
  window: ExactWindow;
}>) {
  // On air: how it was met. Ahead: how the fill in use ends.
  const last = exact.calculations.at(-1);
  if (exact.met) {
    const status = metStatus(exact);
    return (
      <div className={`card exact-tile exact-result tone-box-${status.tone}`}>
        <span className="small">{exact.planned ? t("exactTimes.resultPlanned") : t("exactTimes.resultMet")}</span>
        <strong>{status.label}</strong>
        <span className="small">
          {t(exact.planned ? "exactTimes.plannedAt" : "exactTimes.startedAt", {
            time: clockTime(exact.met.at_unix_ms + exact.met.offset_ms),
          })}
        </span>
      </div>
    );
  }
  if (!latest || !last) {
    return (
      <div className="card exact-tile">
        <span className="muted small">{t("exactTimes.result")}</span>
        <strong className="muted">–</strong>
      </div>
    );
  }
  // Chosen by hand: the chosen attempt's ending, not the automatic best's.
  const chosen = choice && choice.exact_at_unix_ms === window.at ? choice : null;
  const ending = chosen ? chosen.ending : last.ending;
  const number = chosen ? chosen.attempt : latest.kept;
  const tone = ending.faded || !ending.in_window ? "bad" : "good";
  const end = ending.offset_ms === null || ending.offset_ms === undefined ? null : window.at + ending.offset_ms;
  return (
    <div className={`card exact-tile exact-result tone-box-${tone}`}>
      <span className="small">{chosen ? t("exactTimes.resultByHand") : t("exactTimes.resultAuto")}</span>
      <strong>
        {t("exactTimes.resultTitle", {
          number: String(number ?? "?"),
          offset: ending.offset_ms === null || ending.offset_ms === undefined ? "–" : signedSeconds(ending.offset_ms),
        })}
      </strong>
      <span className="small">
        {t("exactTimes.resultDetail", {
          time: end === null ? "–" : clockTime(Math.min(end, window.cut)),
          faded: ending.faded
            ? t("exactTimes.fadedBy", { seconds: Math.round(cutMs(ending, window) / 1000) })
            : t("exactTimes.nothingFaded"),
          filling:
            ending.filling_ms > 0
              ? t("exactTimes.filled", { seconds: Math.round(ending.filling_ms / 1000) })
              : t("exactTimes.noFilling"),
        })}
      </span>
    </div>
  );
}

function HandChosen({ choice }: Readonly<{ choice: Schemas["HandChoice"] }>) {
  const canChoose = useCan("operator");
  const back = useAction(
    () => done(api.DELETE("/api/v1/stations/{station_id}/exact-times/choice", { params: { path: here() } })),
    { success: t("exactTimes.backDone"), invalidate: [["exact-times"], ["exact-calculation"]] },
  );
  return (
    <div role="status" className="exact-hand row between">
      <span>
        {rich("exactTimes.byHandBanner", {
          attempt: <strong>{t("exactTimes.byHandAttempt", { number: String(choice.attempt) })}</strong>,
          user: choice.username,
          time: clockTime(choice.at_unix_ms),
          exact: shortTime(choice.exact_at_unix_ms),
        })}
      </span>
      {canChoose && (
        <button type="button" className="btn" disabled={back.isPending} onClick={() => back.mutate(undefined)}>
          {t("exactTimes.backToAutomatic")}
        </button>
      )}
    </div>
  );
}

function Run({ calculation, selected }: Readonly<{ calculation: CalculationSummary; selected: boolean }>) {
  const result = calculationResult(calculation);
  return (
    <button
      type="button"
      className="exact-run"
      aria-pressed={selected}
      onClick={() => setSearch({ calculation: calculation.id, attempt: null })}
    >
      <span className="row between">
        <strong>{clockTime(calculation.computed_at_unix_ms)}</strong>
        <span className={`tone-${result.tone}`}>{result.label}</span>
      </span>
      <span className="muted small">{triggerLabel(calculation)}</span>
      <span className="muted small">
        {t("exactTimes.calculationDetail", {
          count: calculation.fills,
          ms: number(calculation.duration_ms),
        })}
      </span>
      {calculation.chosen && (
        <span className="small tone-good">
          {t("exactTimes.chosenByHandBy", {
            number: String(calculation.chosen.attempt),
            user: calculation.chosen.username ?? "?",
            time: clockTime(calculation.chosen.at_unix_ms),
          })}
        </span>
      )}
    </button>
  );
}

function categoryStyle(detail: Detail, id: number | null | undefined): CSSProperties {
  const color = detail.categories.find((category) => category.id === id)?.color;
  return color ? ({ "--category": color } as CSSProperties) : {};
}

function trackTitle(detail: Detail, id: number): { title: string; artist: string } {
  const label = detail.tracks.find((track) => track.id === id);
  return {
    title: label?.title ?? t("clocks.trackNumber", { id: String(id) }),
    artist: label?.artist ?? "?",
  };
}

function AttemptsCard({
  exact,
  selected,
  detail,
  pending,
  error,
  askedAttempt,
  choice,
}: Readonly<{
  exact: ExactTimeOfDay;
  selected: CalculationSummary;
  detail: Detail | undefined;
  pending: boolean;
  error: unknown;
  askedAttempt: number | null;
  choice: Schemas["HandChoice"] | null;
}>) {
  const [filter, setFilter] = useState<AttemptFilter>("all");
  const [all, setAll] = useState(false);
  const window = windowOf(exact);
  const attempts = detail?.attempts ?? [];
  const shown = filterAttempts(attempts, filter);
  const visible = all ? shown : shown.slice(0, SHOWN);
  const span = spanOf(attempts, detail?.from_unix_ms ?? window.from, window);
  const current = detail ? pickAttempt(detail, askedAttempt, choice) : undefined;
  const inUse = detail && attemptInUse(detail, choice);
  const followed = selected.trigger === "followed";
  const title = followed
    ? t("exactTimes.followedOf", {
        time: clockTime(selected.computed_at_unix_ms),
        from: detail ? clockTime(detail.computed_at_unix_ms) : "–",
      })
    : t("exactTimes.attemptsOf", { time: clockTime(selected.computed_at_unix_ms) });
  return (
    <Card
      title={title}
      className="exact-attempts"
      actions={
        <Segmented
          label={t("exactTimes.filter")}
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: t("exactTimes.filterAll", { n: attempts.length }) },
            { value: "in_window", label: t("exactTimes.filterInWindow", { n: filterAttempts(attempts, "in_window").length }) },
            { value: "faded", label: t("exactTimes.filterFaded", { n: filterAttempts(attempts, "faded").length }) },
          ]}
        />
      }
    >
      {pending && <Loading />}
      {error !== null && error !== undefined && <ErrorView error={error} />}
      {followed && selected.follows_id === null && <Empty>{t("exactTimes.followedGone")}</Empty>}
      {detail && (
        <div className="table-wrap">
          <div className="exact-timeline">
            <div className="exact-row exact-axis muted small">
              <span>{t("exactTimes.rank")}</span>
              <span>{t("exactTimes.attempt")}</span>
              <span className="exact-ticks">
                {ticks(span, window).map((tick) => (
                  <span
                    key={tick.at}
                    className={tick.exact ? "exact-tick exact-tick-exact" : "exact-tick"}
                    style={{ left: `${tick.left}%` }}
                  >
                    {shortTime(tick.at)}
                  </span>
                ))}
              </span>
              <span>{t("exactTimes.endStatus")}</span>
            </div>
            {visible.length === 0 && <Empty>{t("exactTimes.noAttempts")}</Empty>}
            {visible.map((attempt) => (
              <button
                key={attempt.number}
                type="button"
                className="exact-row exact-attempt"
                aria-pressed={attempt.number === current?.number}
                onClick={() => setSearch({ calculation: detail.id, attempt: attempt.number })}
              >
                <span className={attempt.number === inUse ? "exact-rank in-use" : "exact-rank"}>
                  {attempt.number === inUse
                    ? choice?.calculation_id === detail.id
                      ? t("exactTimes.chosen")
                      : t("exactTimes.kept")
                    : number(attempt.rank)}
                </span>
                <span className="muted">#{attempt.number}</span>
                <span className="exact-bar">
                  <span
                    className="exact-window"
                    style={{
                      left: `${position(span, window.from)}%`,
                      width: `${position(span, window.cut) - position(span, window.from)}%`,
                    }}
                  />
                  <span className="exact-line" style={{ left: `${position(span, window.at)}%` }} />
                  {blocks(attempt, span, window).map((block) => {
                    const track = attempt.tracks[block.index];
                    if (!track) {
                      return null;
                    }
                    const { title, artist } = trackTitle(detail, track.track_id);
                    const cut = block.keptShare < 1;
                    return (
                      <span
                        key={block.index}
                        className={cut ? "exact-block cut" : "exact-block"}
                        title={`${title} · ${artist} · ${duration(track.next_ms)}`}
                        style={{
                          ...categoryStyle(detail, track.category_id),
                          left: `${block.left}%`,
                          width: `calc(${block.width}% - 2px)`,
                          ...(cut ? ({ "--kept": `${block.keptShare * 100}%` } as CSSProperties) : {}),
                        }}
                      >
                        {title}
                      </span>
                    );
                  })}
                </span>
                <Pill status={endingStatus(attempt.ending, window)} />
              </button>
            ))}
          </div>
          <p className="muted small exact-help">
            {t("exactTimes.shown", { shown: visible.length, total: shown.length })}
            {!all && shown.length > visible.length && (
              <>
                {" · "}
                <button type="button" className="link-button exact-link" onClick={() => setAll(true)}>
                  {t("exactTimes.showAll")}
                </button>
              </>
            )}
            {" · "}
            {t("exactTimes.legend")}
          </p>
        </div>
      )}
    </Card>
  );
}

function AttemptDetail({
  detail,
  attempt,
  choice,
}: Readonly<{ detail: Detail; attempt: Attempt | undefined; choice: Schemas["HandChoice"] | null }>) {
  const canChoose = useCan("operator");
  const window = windowOf(detail);
  const use = useAction(
    (n: number) =>
      done(
        api.POST("/api/v1/stations/{station_id}/exact-times/calculations/{id}/attempts/{n}/use", {
          params: { path: here({ id: detail.id, n }) },
        }),
      ),
    {
      success: t("exactTimes.useDone"),
      invalidate: [["exact-times"], ["exact-calculation"], ["queue"]],
    },
  );
  if (!attempt) {
    return null;
  }
  const kept = detail.attempts.find((other) => other.number === detail.kept) ?? detail.attempts[0] ?? attempt;
  const byHand = choice?.calculation_id === detail.id;
  const inUse = attemptInUse(detail, choice);
  const status = endingStatus(attempt.ending, window);
  const isInUse = attempt.number === inUse && detail.can_choose;
  const canUse = canChoose && detail.can_choose && lands(attempt.ending) && attempt.number !== inUse;
  const passed = window.at <= Date.now();
  const judged = verdict(attempt, kept);
  const search = attempt.search;
  return (
    <Card
      title={t("exactTimes.attemptTitle", { number: String(attempt.number), rank: attempt.rank })}
      className="exact-detail"
      actions={
        <>
          <Pill status={status} />
          {attempt.previous && <span className="badge">{t("exactTimes.previous")}</span>}
          {isInUse && (
            <span className="badge accent exact-pill">
              {byHand ? t("exactTimes.playingByHand") : t("exactTimes.playingAuto")}
            </span>
          )}
          {canUse && (
            <button
              type="button"
              className="btn primary"
              disabled={use.isPending}
              onClick={() => use.mutate(attempt.number)}
            >
              {t("exactTimes.use")}
            </button>
          )}
        </>
      }
    >
      <div className="stack">
        {detail.can_choose && !lands(attempt.ending) && (
          <p className="small bad exact-note">{t("exactTimes.cannotUse")}</p>
        )}
        {!detail.can_choose && passed && <p className="small muted exact-note">{t("exactTimes.passed")}</p>}
        {!detail.can_choose && !passed && !detail.latest && (
          <p className="small muted exact-note">{t("exactTimes.outdated")}</p>
        )}
        {!detail.can_choose && !passed && detail.latest && (
          <p className="small muted exact-note">{t("exactTimes.notFollowed")}</p>
        )}
        <div className="table-wrap">
          <table className="table exact-tracks">
            <thead>
              <tr>
                <th>{t("exactTimes.colSlot")}</th>
                <th>{t("exactTimes.colCategory")}</th>
                <th>{t("exactTimes.colTitle")}</th>
                <th className="num">{t("exactTimes.colLength")}</th>
                <th>{t("exactTimes.colStart")}</th>
                <th>{t("exactTimes.colEnd")}</th>
                <th>{t("exactTimes.colChosen")}</th>
                <th>{t("exactTimes.colRules")}</th>
              </tr>
            </thead>
            <tbody>
              {attempt.tracks.map((track, index) => {
                const { title, artist } = trackTitle(detail, track.track_id);
                const category = detail.categories.find((c) => c.id === track.category_id);
                return (
                  <tr key={`${index}-${track.track_id}`}>
                    <td>
                      {track.chosen === "filler"
                        ? t("exactTimes.fillerSlot")
                        : track.chosen === "medley"
                          ? t("exactTimes.medleySlot")
                          : track.slot === null || track.slot === undefined
                            ? "–"
                            : number(track.slot + 1)}
                    </td>
                    <td>
                      {category ? (
                        <span className="chip" style={categoryStyle(detail, category.id)}>
                          <span className="swatch exact-swatch" />
                          {category.name}
                        </span>
                      ) : (
                        "–"
                      )}
                    </td>
                    <td>
                      <Link to={`/library/${track.track_id}`}>
                        <strong>{title}</strong>
                      </Link>{" "}
                      <span className="muted">· {artist}</span>
                    </td>
                    <td className="num">{duration(track.next_ms)}</td>
                    <td className="num">{clockTime(track.start_unix_ms)}</td>
                    <td className="num">{clockTime(track.start_unix_ms + track.next_ms)}</td>
                    <td className="muted">{chosenLabel(track)}</td>
                    <td className="muted">{track.step > 0 ? stepLabel(track.step) : t("exactTimes.rulesHeld")}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
        <div className="grid two exact-boxes">
          <div className="exact-box">
            <h3>{t("exactTimes.ranking")}</h3>
            {criteria(attempt, kept, window).map((criterion) => (
              <div key={criterion.n} className="exact-criterion small">
                <span className="muted">{criterion.n}</span>
                <span>{criterion.label}</span>
                <strong>{criterion.mine}</strong>
                <span className="muted">
                  {t("exactTimes.keptValue", { number: String(kept.number), value: criterion.kept })}
                </span>
              </div>
            ))}
            <p className={`small exact-verdict tone-${judged.tone}`}>{judged.label}</p>
          </div>
          <div className="exact-box">
            <h3>{t("exactTimes.search", { count: search?.slots.length || 3 })}</h3>
            <dl className="facts small">
              {search ? (
                <>
                  <dt>
                    {t("exactTimes.searchOptions", {
                      slots: search.slots.map((slot) => number(slot + 1)).join(" · "),
                    })}
                  </dt>
                  <dd>{joined(search.options)}</dd>
                  <dt>{t("exactTimes.searchCombinations")}</dt>
                  <dd>
                    {search.capped
                      ? t("exactTimes.searchCapped", { n: number(search.combinations) })
                      : number(search.combinations)}
                  </dd>
                  <dt>{t("exactTimes.searchLanding")}</dt>
                  <dd>{number(search.landing)}</dd>
                  <dt>{t("exactTimes.searchPlayed")}</dt>
                  <dd>{number(search.played_again)}</dd>
                  <dt>{t("exactTimes.searchFound")}</dt>
                  <dd>{search.found ? t("exactTimes.yes") : t("exactTimes.no")}</dd>
                </>
              ) : (
                <>
                  <dt>{t("exactTimes.searchFound")}</dt>
                  <dd>{t("exactTimes.notSearched")}</dd>
                </>
              )}
              <dt>{t("exactTimes.seed")}</dt>
              <dd className="mono">{attempt.seed}</dd>
            </dl>
          </div>
        </div>
      </div>
    </Card>
  );
}
