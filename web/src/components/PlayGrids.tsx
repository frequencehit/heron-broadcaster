// SPDX-License-Identifier: AGPL-3.0-or-later

// When a track played (docs/web.md#plays-by-day-and-hour): its plays of the
// last 30 days, week by week, and the hours of the week it played in, for
// one station or all of them, all plays or rotation only.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { api, query, unwrap } from "../api";
import { number, t } from "../i18n";
import { sourceLabel } from "../lib/labels";
import {
  type GridDay,
  type GridHour,
  type GridPlay,
  type PlayGrid,
  cellKey,
  dateLabel,
  historyHref,
  hourCells,
  most,
  shade,
  weeks,
} from "../lib/playGrid";
import { dayName, hourLabel } from "../lib/rotation";
import { Link, navigate } from "../lib/router";
import { chooseStation, currentStation } from "../lib/station";
import { useStations } from "./Stations";
import { Card, ErrorView, Loading, Segmented } from "./ui";

type StationChoice = number | "all";
type SourceChoice = "all" | "rotation";
const PERIODS = [30, 90, 365] as const;
type Period = (typeof PERIODS)[number];
/** The plays listed under the hours grid at most. */
const LISTED = 10;

export function PlayGrids({ trackId }: Readonly<{ trackId: number }>) {
  const stations = useStations();
  const [station, setStation] = useState<StationChoice>(() => currentStation() ?? "all");
  const [source, setSource] = useState<SourceChoice>("all");
  const [period, setPeriod] = useState<Period>(30);
  const grid = useQuery({
    queryKey: ["track", trackId, "plays-grid", station, source, period],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/tracks/{id}/plays/grid", {
          params: {
            path: { id: trackId },
            query: query({
              station_id: station === "all" ? undefined : station,
              source: source === "all" ? undefined : source,
              days: period,
            }),
          },
        }),
      ),
    placeholderData: (previous) => previous,
  });
  const codes = new Map(stations.map((s) => [s.id, s.code]));
  const several = stations.length > 1;

  return (
    <section className="stack play-grids">
      <div className="row between">
        <h2>{t("plays.title")}</h2>
        <div className="row">
          {several && (
            <Segmented
              label={t("plays.station")}
              value={String(station)}
              options={[
                ...stations.map((s) => ({ value: String(s.id), label: `${s.name} (${s.code})` })),
                { value: "all", label: t("plays.allStations") },
              ]}
              onChange={(value) => setStation(value === "all" ? "all" : Number(value))}
            />
          )}
          <Segmented<SourceChoice>
            label={t("plays.counted")}
            value={source}
            options={[
              { value: "all", label: t("plays.allPlays") },
              { value: "rotation", label: t("plays.rotationOnly") },
            ]}
            onChange={setSource}
          />
        </div>
      </div>
      {grid.data && (
        <p className="muted small">
          {station === "all"
            ? t("plays.zoneAll", { zone: grid.data.time_zone })
            : t("plays.zone", { zone: grid.data.time_zone })}
        </p>
      )}
      {grid.isError && <ErrorView error={grid.error} />}
      <div className="grid two">
        <Card
          title={t("plays.days")}
          actions={grid.data && <DaysSummary grid={grid.data} />}
        >
          {grid.data ? (
            <DaysGrid
              grid={grid.data}
              trackId={trackId}
              codes={station === "all" ? codes : null}
              onOpen={() => {
                // The history shows the station chosen in the switcher.
                if (station !== "all") {
                  chooseStation(station);
                }
              }}
            />
          ) : (
            <Loading />
          )}
        </Card>
        <Card
          title={t("plays.hours")}
          actions={
            <Segmented
              label={t("plays.period")}
              value={String(period)}
              options={PERIODS.map((days) => ({ value: String(days), label: t("plays.periodDays", { count: days }) }))}
              onChange={(value) => setPeriod(Number(value) as Period)}
            />
          }
        >
          {grid.data ? (
            <HoursGrid grid={grid.data} period={period} codes={station === "all" ? codes : null} />
          ) : (
            <Loading />
          )}
        </Card>
      </div>
    </section>
  );
}

function DaysSummary({ grid }: Readonly<{ grid: PlayGrid }>) {
  const first = grid.days[0];
  const count = grid.days.reduce((sum, day) => sum + day.plays.length, 0);
  return (
    <span className="muted small">
      {t("plays.daysSummary", { count, from: first ? dateLabel(first.date) : "", to: dateLabel(grid.today) })}
    </span>
  );
}

/** How a play came, and on which station when `codes` are given (all stations). */
function playSource(play: GridPlay, codes: Map<number, string> | null): string {
  const how = sourceLabel(play.source);
  const code = codes?.get(play.station_id);
  const said = code ? `${code}, ${how}` : how;
  // Listeners' votes (docs/listener-interaction.md#likes).
  return play.likes + play.dislikes > 0
    ? `${said} · ${t("likes.counts", { likes: number(play.likes), dislikes: number(play.dislikes) })}`
    : said;
}

function dayTitle(day: GridDay): string {
  const label = dateLabel(day.date, true);
  return day.plays.length === 0
    ? t("plays.dayNone", { day: label })
    : t("plays.dayCount", { day: label, count: day.plays.length });
}

export function DaysGrid({
  grid,
  trackId,
  codes,
  onOpen,
}: Readonly<{ grid: PlayGrid; trackId: number; codes: Map<number, string> | null; onOpen?: () => void }>) {
  const [shown, setShown] = useState<string>(grid.today);
  const rows = weeks(grid.days);
  const top = most(grid.days);
  const day = grid.days.find((d) => d.date === shown) ?? grid.days.at(-1);
  const open = (target: GridDay) => {
    onOpen?.();
    navigate(historyHref(trackId, target));
  };
  return (
    <div className="stack">
      <table className="play-days">
        <thead>
          <tr>
            <th className="week">{t("plays.weekOf")}</th>
            {[1, 2, 3, 4, 5, 6, 7].map((weekday) => (
              <th key={weekday}>{dayName(weekday)}</th>
            ))}
            <th className="total">{t("plays.total")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((week) => (
            <tr key={week.monday}>
              <th className="week" scope="row">
                {dateLabel(week.monday)}
              </th>
              {week.cells.map((cell) => (
                <td key={cell.date}>
                  {cell.day ? (
                    <button
                      type="button"
                      className={[
                        "play-day",
                        `level-${shade(cell.day.plays.length, top)}`,
                        cell.date === grid.today ? "today" : "",
                        cell.date === day?.date ? "shown" : "",
                      ]
                        .filter(Boolean)
                        .join(" ")}
                      aria-label={dayTitle(cell.day)}
                      aria-current={cell.date === grid.today ? "date" : undefined}
                      onMouseEnter={() => setShown(cell.date)}
                      onFocus={() => setShown(cell.date)}
                      onClick={() => cell.day && open(cell.day)}
                    >
                      <span className="num">{cell.day.plays.length}</span>
                      <small>{dateLabel(cell.date)}</small>
                    </button>
                  ) : (
                    <span className="play-day out">
                      <small>{dateLabel(cell.date)}</small>
                    </span>
                  )}
                </td>
              ))}
              <td className="total num">{week.total}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {day && (
        <div className="play-detail" aria-live="polite">
          <strong>{dayTitle(day)}</strong>
          {day.plays.length > 0 && (
            <ul>
              {day.plays.map((play) => (
                <li key={play.started_at_unix_ms}>
                  <span className="num">{play.time}</span>{" "}
                  <span className="muted small">· {playSource(play, codes)}</span>
                </li>
              ))}
            </ul>
          )}
          <div>
            <Link to={historyHref(trackId, day)} className="small" onClick={() => onOpen?.()}>
              {t("plays.openHistory")}
            </Link>
          </div>
        </div>
      )}
      <span className="muted small">{t("plays.daysHint")}</span>
    </div>
  );
}

function hourTitle(weekday: number, hour: number, count: number, period: number): string {
  const params = {
    day: dayName(weekday),
    from: hourLabel(hour),
    to: `${String(hour).padStart(2, "0")}:59`,
    days: period,
  };
  return count === 0 ? t("plays.hourNone", params) : t("plays.hourCount", { ...params, count });
}

export function HoursGrid({
  grid,
  period,
  codes,
}: Readonly<{ grid: PlayGrid; period: number; codes: Map<number, string> | null }>) {
  const cells = hourCells(grid.hours);
  const top = most(grid.hours);
  const [shown, setShown] = useState<[number, number] | null>(() => {
    const first = grid.hours[0];
    return first ? [first.weekday, first.hour] : null;
  });
  const cell: GridHour | undefined = shown ? cells.get(cellKey(shown[0], shown[1])) : undefined;
  const plays = cell?.plays ?? [];
  return (
    <div className="stack">
      <table className="play-hours">
        <thead>
          <tr>
            <th className="hour" />
            {[1, 2, 3, 4, 5, 6, 7].map((weekday) => (
              <th key={weekday}>{dayName(weekday)}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {Array.from({ length: 24 }, (_, hour) => (
            <tr key={hour}>
              <th className="hour num" scope="row">
                {hourLabel(hour)}
              </th>
              {[1, 2, 3, 4, 5, 6, 7].map((weekday) => {
                const count = cells.get(cellKey(weekday, hour))?.plays.length ?? 0;
                const here = shown?.[0] === weekday && shown[1] === hour;
                return (
                  <td key={weekday}>
                    <button
                      type="button"
                      className={`play-hour level-${shade(count, top)}${here ? " shown" : ""}`}
                      aria-label={hourTitle(weekday, hour, count, period)}
                      onMouseEnter={() => setShown([weekday, hour])}
                      onFocus={() => setShown([weekday, hour])}
                      onClick={() => setShown([weekday, hour])}
                    >
                      {count > 0 ? "✓" : ""}
                    </button>
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
      {shown && (
        <div className="play-detail" aria-live="polite">
          <strong>{hourTitle(shown[0], shown[1], plays.length, period)}</strong>
          {plays.length > 0 && (
            <ul>
              {plays.slice(0, LISTED).map((play) => (
                <li key={play.started_at_unix_ms}>
                  {dateLabel(play.date, true)} <span className="num">{play.time}</span>{" "}
                  <span className="muted small">· {playSource(play, codes)}</span>
                </li>
              ))}
              {plays.length > LISTED && (
                <li className="muted">{t("plays.more", { count: plays.length - LISTED })}</li>
              )}
            </ul>
          )}
        </div>
      )}
      <span className="muted small">{t("plays.hoursHint")}</span>
    </div>
  );
}
