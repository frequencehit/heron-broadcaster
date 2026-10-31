// SPDX-License-Identifier: AGPL-3.0-or-later

// The schedules: which clock runs at each hour of the week, in the
// station's time zone. Pick a schedule and a clock, then paint hours,
// rectangles, days, an hour of every day or the week (docs/web.md ›
// Painting the schedule); a schedule other than the default one plays
// during its periods (docs/web.md › Schedules and periods).

import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type Period, type ScheduleSummary, type ScheduledHour, api, done, unwrap } from "../api";
import { Banner, Card, ConfirmButton, Empty, ErrorView, Field, Icon, Loading, Modal, PageHead } from "../components/ui";
import { locale, rich, t } from "../i18n";
import { DAY_NUMBERS, dayName, hourLabel } from "../lib/rotation";
import {
  type Grid,
  type Hour,
  HOURS,
  civilLabel,
  joinCivil,
  periodProblem,
  splitCivil,
  WEEKDAYS,
  WEEKEND,
  cell,
  copyDay,
  everyDay,
  paint as paintHours,
  rectangle,
  wholeDays,
} from "../lib/schedule";
import { Link } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { here } from "../lib/station";

const PALETTE = ["#2f6fd6", "#b4549a", "#1d7a3e", "#b07a00", "#0b6e66", "#6b4fbb", "#c62828", "#5f6268"];

function toGrid(hours: ScheduledHour[]): Grid {
  return new Map(hours.map((h) => [cell(h.day_of_week, h.hour), h.clock_id]));
}

function sameGrid(a: Grid, b: Grid): boolean {
  return a.size === b.size && [...a].every(([key, clock]) => b.get(key) === clock);
}

function fromGrid(grid: Grid): ScheduledHour[] {
  return [...grid.entries()].map(([key, clock_id]) => {
    const [day, hour] = key.split("-").map(Number);
    return { day_of_week: day ?? 1, hour: hour ?? 0, clock_id };
  });
}

/** The name shown for a schedule: the default one has none of its own. */
function scheduleName(schedule: Pick<ScheduleSummary, "name" | "is_default"> | undefined): string {
  return !schedule ? "?" : schedule.is_default ? t("schedules.default") : schedule.name;
}

function samePeriods(a: Period[], b: Period[]): boolean {
  return JSON.stringify(a.map(normal)) === JSON.stringify(b.map(normal));
}

function normal(period: Period): Period {
  return { start: period.start, end: period.end, every_year: period.every_year ?? false };
}

export function Schedule() {
  const canEdit = useCan("operator");
  const clocks = useQuery({ queryKey: ["clocks"], queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/clocks", { params: { path: here() } })) });
  const list = useQuery({ queryKey: ["schedules"], queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/schedules", { params: { path: here() } })) });
  // The schedule shown; the one playing now until another is picked.
  const [chosen, setChosen] = useState<number | undefined>(undefined);
  const selectedId = chosen ?? list.data?.active_id;
  const schedule = useQuery({
    queryKey: ["schedules", selectedId],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/schedules/{id}", { params: { path: here({ id: selectedId ?? 0 }) } })),
    enabled: selectedId !== undefined,
  });
  const [periods, setPeriods] = useState<Period[]>([]);
  const station = useQuery({
    queryKey: ["settings", "station"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/station", { params: { path: here() } })),
  });
  const [grid, setGrid] = useState<Grid>(new Map());
  // The clock painted; null paints "no clock", undefined until one is picked.
  const [picked, setBrush] = useState<number | null | undefined>(undefined);
  const brush = picked === undefined ? (clocks.data?.[0]?.id ?? null) : picked;
  const [onlyEmpty, setOnlyEmpty] = useState(false);
  // The weeks before each stroke, to undo them.
  const [undo, setUndo] = useState<Grid[]>([]);
  // A drag: where it started, and the week before it.
  const stroke = useRef<{ start: Hour; base: Grid } | null>(null);
  // The hour clicked last, for Shift + click.
  const last = useRef<Hour | null>(null);
  const [copyFrom, setCopyFrom] = useState<number>(1);
  const [copyTo, setCopyTo] = useState<ReadonlySet<number>>(new Set());

  useEffect(() => {
    if (schedule.data) {
      setGrid(toGrid(schedule.data.hours));
      setPeriods(schedule.data.periods.map(normal));
      setUndo([]);
    }
  }, [schedule.data]);
  useEffect(() => {
    const stop = () => {
      stroke.current = null;
    };
    window.addEventListener("pointerup", stop);
    return () => window.removeEventListener("pointerup", stop);
  }, []);
  const undoLast = useCallback(() => {
    setUndo((stack) => {
      const previous = stack[stack.length - 1];
      if (previous) {
        setGrid(previous);
      }
      return stack.slice(0, -1);
    });
  }, []);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && !event.shiftKey && event.key.toLowerCase() === "z") {
        const target = event.target as HTMLElement | null;
        if (target?.closest("input, textarea, select")) {
          return;
        }
        event.preventDefault();
        undoLast();
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [undoLast]);

  const colors = useMemo(
    () => new Map((clocks.data ?? []).map((clock, index) => [clock.id, PALETTE[index % PALETTE.length]])),
    [clocks.data],
  );
  const names = new Map((clocks.data ?? []).map((clock) => [clock.id, clock.name]));
  const save = useAction(
    () =>
      unwrap(
        api.PUT("/api/v1/stations/{station_id}/schedules/{id}", {
          params: { path: here({ id: selectedId ?? 0 }) },
          body: { hours: fromGrid(grid), periods },
        }),
      ),
    {
      success: t("schedule.saved"),
      invalidate: [["schedules"], ["schedule"], ["clocks"]],
    },
  );

  if (clocks.isPending || list.isPending || schedule.isPending) {
    return <Loading />;
  }
  if (clocks.isError || list.isError || schedule.isError) {
    return <ErrorView error={clocks.error ?? list.error ?? schedule.error} />;
  }
  const dirty = !sameGrid(toGrid(schedule.data.hours), grid) || !samePeriods(schedule.data.periods, periods);
  const invalid = periods.some((period) => periodProblem(period) !== null);
  /** One stroke: these hours painted with the clock picked, undoable. */
  const apply = (hours: Hour[]) => {
    setUndo((stack) => [...stack, grid]);
    setGrid(paintHours(grid, hours, brush, onlyEmpty));
  };
  const brushName = brush === null ? t("schedule.noClockBrush") : (names.get(brush) ?? "?");
  const revert = () => {
    setGrid(toGrid(schedule.data.hours));
    setPeriods(schedule.data.periods.map(normal));
    setUndo([]);
  };
  const hoursOf = (clock: number) => [...grid.values()].filter((id) => id === clock).length;

  return (
    <>
      <PageHead
        title={t("nav.schedule")}
        description={rich("schedule.description", {
          link: <Link to="/clocks">{t("schedule.clock")}</Link>,
          zone: station.data?.timezone ?? "…",
        })}
        actions={
          canEdit && (
            <>
              <button
                type="button"
                className="btn"
                disabled={undo.length === 0}
                title={t("schedule.undoHelp")}
                onClick={undoLast}
              >
                {t("schedule.undo")}
              </button>
              {dirty && (
                <button type="button" className="btn" onClick={revert}>
                  {t("track.revert")}
                </button>
              )}
              <button
                type="button"
                className="btn primary"
                disabled={!dirty || invalid || save.isPending}
                onClick={() => save.mutate(undefined)}
              >
                {t("common.save")}
              </button>
            </>
          )
        }
      />
      <SchedulesCard
        list={list.data.schedules}
        activeId={list.data.active_id}
        nextChange={list.data.next_change ?? null}
        selected={schedule.data}
        periods={periods}
        canEdit={canEdit}
        onSelect={setChosen}
        onPeriods={setPeriods}
      />
      {clocks.data.length === 0 ? (
        <Card>
          <Empty>
            {rich("schedule.noClock", { link: <Link to="/clocks/new">{t("schedule.createOne")}</Link> })}
          </Empty>
        </Card>
      ) : (
        <Card>
          <div className="stack">
            {canEdit && (
              <div className="row" role="radiogroup" aria-label={t("schedule.clockToSchedule")}>
                <span className="muted small">{t("schedule.paintWith")}</span>
                {clocks.data.map((clock) => (
                  <button
                    key={clock.id}
                    type="button"
                    role="radio"
                    aria-checked={brush === clock.id}
                    className={brush === clock.id ? "btn small brush active" : "btn small brush"}
                    onClick={() => setBrush(clock.id)}
                  >
                    <span className="swatch" style={{ background: colors.get(clock.id) }} />
                    {clock.name}
                    <span className="muted">{t("rotation.hours", { hours: hoursOf(clock.id) })}</span>
                  </button>
                ))}
                <button
                  type="button"
                  role="radio"
                  aria-checked={brush === null}
                  className={brush === null ? "btn small brush active" : "btn small brush"}
                  onClick={() => setBrush(null)}
                >
                  <span className="swatch empty-swatch" /> {t("schedule.noClockBrush")}
                </button>
              </div>
            )}
            {canEdit && (
              <div className="row small">
                <button type="button" className="btn small" onClick={() => apply(wholeDays(WEEKDAYS))}>
                  {t("schedule.weekdays")}
                </button>
                <button type="button" className="btn small" onClick={() => apply(wholeDays(WEEKEND))}>
                  {t("schedule.weekend")}
                </button>
                <button type="button" className="btn small" onClick={() => apply(wholeDays(DAY_NUMBERS))}>
                  {t("schedule.wholeWeek")}
                </button>
                <label className="check">
                  <input type="checkbox" checked={onlyEmpty} onChange={(e) => setOnlyEmpty(e.target.checked)} />
                  {t("schedule.onlyEmpty")}
                </label>
                <span className="spacer" />
                <span className="row" role="group" aria-label={t("schedule.copyDay")}>
                  <span>{t("schedule.copy")}</span>
                  <select
                    className="input"
                    style={{ width: "auto" }}
                    aria-label={t("schedule.copyFrom")}
                    value={copyFrom}
                    onChange={(e) => setCopyFrom(Number(e.target.value))}
                  >
                      {DAY_NUMBERS.map((day) => (
                        <option key={day} value={day}>
                          {dayName(day)}
                        </option>
                      ))}
                  </select>
                  {t("schedule.onto")}
                  {DAY_NUMBERS.filter((day) => day !== copyFrom).map((day) => (
                    <label key={day} className="check">
                      <input
                        type="checkbox"
                        checked={copyTo.has(day)}
                        onChange={(e) =>
                          setCopyTo((current) => {
                            const next = new Set(current);
                            if (e.target.checked) {
                              next.add(day);
                            } else {
                              next.delete(day);
                            }
                            return next;
                          })
                        }
                      />
                      {dayName(day)}
                    </label>
                  ))}
                  <button
                    type="button"
                    className="btn small"
                    disabled={[...copyTo].every((day) => day === copyFrom)}
                    onClick={() => {
                      setUndo((stack) => [...stack, grid]);
                      setGrid(copyDay(grid, copyFrom, [...copyTo]));
                    }}
                  >
                    {t("schedule.copyButton")}
                  </button>
                </span>
              </div>
            )}
            <div className="table-wrap">
              <table className="schedule">
                <thead>
                  <tr>
                    <th />
                    {HOURS.map((hour) => {
                      const label = t("schedule.paintHour", { hour: hourLabel(hour), clock: brushName });
                      return (
                        <th key={hour} scope="col">
                          {canEdit ? (
                            <button
                              type="button"
                              className="head"
                              aria-label={label}
                              title={label}
                              onClick={() => apply(everyDay(hour))}
                            >
                              {String(hour).padStart(2, "0")}
                            </button>
                          ) : (
                            String(hour).padStart(2, "0")
                          )}
                        </th>
                      );
                    })}
                  </tr>
                </thead>
                <tbody>
                  {DAY_NUMBERS.map((day) => {
                    const name = dayName(day);
                    return (
                      <tr key={day}>
                        <th scope="row">
                          {canEdit ? (
                            <button
                              type="button"
                              className="head"
                              aria-label={t("schedule.paintDay", { day: name, clock: brushName })}
                              title={t("schedule.paintDay", { day: name, clock: brushName })}
                              onClick={() => apply(wholeDays([day]))}
                            >
                              {name}
                            </button>
                          ) : (
                            name
                          )}
                        </th>
                        {HOURS.map((hour) => {
                          const clock = grid.get(cell(day, hour));
                          const label = t("schedule.cell", {
                            day: name,
                            hour: hourLabel(hour),
                            clock: clock !== undefined ? (names.get(clock) ?? "?") : t("schedule.noClockCell"),
                          });
                          return (
                            <td key={hour}>
                              <button
                                type="button"
                                className="hour"
                                aria-label={label}
                                title={label}
                                disabled={!canEdit}
                                style={{ background: clock !== undefined ? colors.get(clock) : undefined }}
                                onPointerDown={(event) => {
                                  event.preventDefault();
                                  // The pointer may cross other cells: let them get its events.
                                  event.currentTarget.releasePointerCapture(event.pointerId);
                                  const here = { day, hour };
                                  const from = last.current;
                                  last.current = here;
                                  if (event.shiftKey && from) {
                                    apply(rectangle(from, here));
                                    return;
                                  }
                                  stroke.current = { start: here, base: grid };
                                  apply([here]);
                                }}
                                onPointerEnter={() => {
                                  const current = stroke.current;
                                  if (current) {
                                    setGrid(paintHours(current.base, rectangle(current.start, { day, hour }), brush, onlyEmpty));
                                  }
                                }}
                                onKeyDown={(event) => {
                                  if (event.key === "Enter" || event.key === " ") {
                                    event.preventDefault();
                                    const here = { day, hour };
                                    const from = last.current;
                                    last.current = here;
                                    apply(event.shiftKey && from ? rectangle(from, here) : [here]);
                                  }
                                }}
                              />
                            </td>
                          );
                        })}
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
            <div className="row small muted">
              {clocks.data.map((clock) => (
                <span key={clock.id} className="row" style={{ gap: "0.3rem" }}>
                  <span className="swatch" style={{ background: colors.get(clock.id) }} />
                  {rich("schedule.clockHours", {
                    clock: <Link to={`/clocks/${clock.id}`}>{clock.name}</Link>,
                    count: hoursOf(clock.id),
                  })}
                </span>
              ))}
              <span>{t("schedule.unscheduled", { count: 168 - grid.size })}</span>
            </div>
          </div>
        </Card>
      )}
    </>
  );
}

/** The schedules: which plays now, the next change, the one shown and its periods. */
function SchedulesCard({
  list,
  activeId,
  nextChange,
  selected,
  periods,
  canEdit,
  onSelect,
  onPeriods,
}: {
  list: ScheduleSummary[];
  activeId: number;
  nextChange: { at: string; schedule_id: number } | null;
  selected: { id: number; name: string; is_default: boolean; hours: ScheduledHour[]; periods: Period[] };
  periods: Period[];
  canEdit: boolean;
  onSelect: (id: number) => void;
  onPeriods: (periods: Period[]) => void;
}) {
  const byId = new Map(list.map((schedule) => [schedule.id, schedule]));
  const [creating, setCreating] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState("");
  const [copyFrom, setCopyFrom] = useState<number | null>(selected.id);
  const create = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/stations/{station_id}/schedules", { params: { path: here() }, body: { name: name.trim(), ...(copyFrom !== null ? { copy_from: copyFrom } : {}) } }),
      ),
    {
      success: t("schedules.created"),
      invalidate: [["schedules"]],
      onSuccess: (created) => {
        setCreating(false);
        onSelect(created.id);
      },
    },
  );
  const rename = useAction(
    () =>
      unwrap(
        api.PUT("/api/v1/stations/{station_id}/schedules/{id}", {
          params: { path: here({ id: selected.id }) },
          body: { name: name.trim(), hours: selected.hours, periods: selected.periods },
        }),
      ),
    { success: t("schedules.renamed"), invalidate: [["schedules"]], onSuccess: () => setRenaming(false) },
  );
  const remove = useAction(
    () => done(api.DELETE("/api/v1/stations/{station_id}/schedules/{id}", { params: { path: here({ id: selected.id }) } })),
    {
      success: t("schedules.deleted"),
      invalidate: [["schedules"], ["schedule"], ["clocks"]],
      onSuccess: () => onSelect(activeId === selected.id ? (list[0]?.id ?? activeId) : activeId),
    },
  );
  const next = nextChange ? byId.get(nextChange.schedule_id) : undefined;
  const update = (index: number, changes: Partial<Period>) =>
    onPeriods(periods.map((period, at) => (at === index ? { ...period, ...changes } : period)));
  const bound = (index: number, which: "start" | "end", label: string) => {
    const period = periods[index];
    if (!period) {
      return null;
    }
    const value = splitCivil(period[which]);
    return (
      <Field label={label}>
        <span className="row" style={{ gap: "0.3rem", flexWrap: "nowrap" }}>
          <input
            className="input"
            type="date"
            required
            value={value.date}
            disabled={!canEdit}
            aria-label={t("schedules.boundDate", { bound: label, period: index + 1 })}
            onChange={(event) => update(index, { [which]: joinCivil({ ...value, date: event.target.value }) })}
          />
          <select
            className="input"
            style={{ width: "auto" }}
            value={value.hour}
            disabled={!canEdit}
            aria-label={t("schedules.boundHour", { bound: label, period: index + 1 })}
            onChange={(event) => update(index, { [which]: joinCivil({ ...value, hour: Number(event.target.value) }) })}
          >
            {HOURS.map((hour) => (
              <option key={hour} value={hour}>
                {hourLabel(hour)}
              </option>
            ))}
          </select>
        </span>
      </Field>
    );
  };
  const today = new Date().toISOString().slice(0, 10);
  return (
    <Card title={t("schedules.title")}>
      <div className="stack">
        <div className="row">
          <select
            className="input"
            style={{ width: "auto" }}
            aria-label={t("schedules.shown")}
            value={selected.id}
            onChange={(event) => onSelect(Number(event.target.value))}
          >
            {list.map((schedule) => (
              <option key={schedule.id} value={schedule.id}>
                {scheduleName(schedule)}
                {schedule.id === activeId ? ` · ${t("schedules.playingNow")}` : ""}
              </option>
            ))}
          </select>
          <span className="spacer" />
          {canEdit && (
            <>
              <button
                type="button"
                className="btn small"
                onClick={() => {
                  setName("");
                  setCopyFrom(selected.id);
                  setCreating(true);
                }}
              >
                <Icon name="plus" /> {t("schedules.new")}
              </button>
              {!selected.is_default && (
                <>
                  <button
                    type="button"
                    className="btn small"
                    onClick={() => {
                      setName(selected.name);
                      setRenaming(true);
                    }}
                  >
                    {t("schedules.rename")}
                  </button>
                  <ConfirmButton
                    className="btn small danger"
                    confirm={t("categories.deleteButton")}
                    question={<p>{rich("schedules.deleteQuestion", { name: <strong>{selected.name}</strong> })}</p>}
                    onConfirm={() => remove.mutate(undefined)}
                  >
                    <Icon name="trash" /> {t("categories.deleteButton")}
                  </ConfirmButton>
                </>
              )}
            </>
          )}
        </div>
        <p className="muted small" style={{ margin: 0 }}>
          {rich("schedules.now", { name: <strong>{scheduleName(byId.get(activeId))}</strong> })}{" "}
          {nextChange && next
            ? rich("schedules.nextChange", {
                name: <strong>{scheduleName(next)}</strong>,
                at: civilLabel(nextChange.at, false, locale()),
              })
            : t("schedules.noChange")}
        </p>
        {selected.is_default ? (
          <p className="muted small" style={{ margin: 0 }}>
            {t("schedules.defaultHelp")}
          </p>
        ) : (
          <>
            {periods.length === 0 && <Banner kind="warn">{t("schedules.noPeriod")}</Banner>}
            <ol className="list" aria-label={t("schedules.periods")}>
              {periods.map((period, index) => {
                const problem = periodProblem(period);
                return (
                  <li key={index} className="row" style={{ alignItems: "flex-end" }}>
                    {bound(index, "start", t("schedules.from"))}
                    {bound(index, "end", t("schedules.until"))}
                    <label className="check">
                      <input
                        type="checkbox"
                        checked={period.every_year ?? false}
                        disabled={!canEdit}
                        onChange={(event) => update(index, { every_year: event.target.checked })}
                      />
                      {t("restrictions.everyYear")}
                    </label>
                    {canEdit && (
                      <button
                        type="button"
                        className="btn ghost icon"
                        aria-label={t("schedules.removePeriod", { period: index + 1 })}
                        title={t("schedules.removePeriod", { period: index + 1 })}
                        onClick={() => onPeriods(periods.filter((_, at) => at !== index))}
                      >
                        <Icon name="trash" />
                      </button>
                    )}
                    {problem && <span className="badge warn">{t(problem === "empty" ? "schedules.problemEmpty" : "schedules.problemBackwards")}</span>}
                  </li>
                );
              })}
            </ol>
            <p className="muted small" style={{ margin: 0 }}>
              {t("schedules.periodsHelp")}
            </p>
            {canEdit && (
              <div className="row">
                <button
                  type="button"
                  className="btn small"
                  onClick={() =>
                    onPeriods([
                      ...periods,
                      { start: joinCivil({ date: today, hour: 0 }), end: joinCivil({ date: today, hour: 23 }), every_year: false },
                    ])
                  }
                >
                  <Icon name="plus" /> {t("schedules.addPeriod")}
                </button>
              </div>
            )}
          </>
        )}
      </div>
      <Modal
        title={t("schedules.new")}
        open={creating}
        onClose={() => setCreating(false)}
        footer={
          <>
            <button type="button" className="btn" onClick={() => setCreating(false)}>
              {t("common.cancel")}
            </button>
            <button
              type="button"
              className="btn primary"
              disabled={!name.trim() || create.isPending}
              onClick={() => create.mutate(undefined)}
            >
              {t("schedules.create")}
            </button>
          </>
        }
      >
        <div className="stack">
          <Field label={t("categories.name")}>
            <input
              className="input"
              value={name}
              maxLength={255}
              placeholder={t("schedules.namePlaceholder")}
              onChange={(event) => setName(event.target.value)}
            />
          </Field>
          <Field label={t("schedules.startFrom")} hint={t("schedules.startFromHelp")}>
            <select
              className="input"
              value={copyFrom ?? ""}
              onChange={(event) => setCopyFrom(event.target.value === "" ? null : Number(event.target.value))}
            >
              <option value="">{t("schedules.empty")}</option>
              {list.map((schedule) => (
                <option key={schedule.id} value={schedule.id}>
                  {t("schedules.copyOf", { name: scheduleName(schedule) })}
                </option>
              ))}
            </select>
          </Field>
        </div>
      </Modal>
      <Modal
        title={t("schedules.rename")}
        open={renaming}
        onClose={() => setRenaming(false)}
        footer={
          <>
            <button type="button" className="btn" onClick={() => setRenaming(false)}>
              {t("common.cancel")}
            </button>
            <button
              type="button"
              className="btn primary"
              disabled={!name.trim() || rename.isPending}
              onClick={() => rename.mutate(undefined)}
            >
              {t("common.save")}
            </button>
          </>
        }
      >
        <Field label={t("categories.name")}>
          <input className="input" value={name} maxLength={255} onChange={(event) => setName(event.target.value)} />
        </Field>
      </Modal>
    </Card>
  );
}
