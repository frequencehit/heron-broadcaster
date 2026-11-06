// SPDX-License-Identifier: AGPL-3.0-or-later

// What was played, newest first.

import { CoverThumb } from "../components/Cover";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { Fragment, useState } from "react";
import { type Play, api, query, unwrap } from "../api";
import { Card, Empty, ErrorView, Icon, Loading, PageHead } from "../components/ui";
import { t } from "../i18n";
import { clockChange, clockTime, duration, fullDate } from "../lib/format";
import { endingLabel, sourceLabel } from "../lib/labels";
import { useLive } from "../lib/live";
import { historyFilter } from "../lib/playGrid";
import { Link, useLocation } from "../lib/router";
import { here } from "../lib/station";

const PAGE = 100;

export function History() {
  const [withFx, setWithFx] = useState(false);
  // One track's plays over a span of time, from the track page's grids.
  const filter = historyFilter(useLocation().search);
  // New plays appear as they start.
  const latest = useLive((live) => live.events[0]?.at_unix_ms);
  const plays = useInfiniteQuery({
    queryKey: ["history", withFx, latest, filter?.trackId, filter?.after, filter?.before],
    initialPageParam: undefined as number | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/history", {
          params: {
            path: here(),
            query: query({
              before: pageParam ?? filter?.before,
              after: filter?.after,
              track_id: filter?.trackId,
              exclude_fx: !withFx,
              limit: PAGE,
            }),
          },
        }),
      ),
    getNextPageParam: (last) => (last.length < PAGE ? undefined : last[last.length - 1]?.started_at_unix_ms),
  });
  const rows = plays.data?.pages.flat() ?? [];
  const station = useQuery({
    queryKey: ["settings", "station"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/station", { params: { path: here() } })),
  });

  return (
    <>
      <PageHead
        title={t("nav.history")}
        description={t("history.description", { zone: station.data?.timezone ?? "…" })}
        actions={
          <label className="check">
            <input type="checkbox" checked={withFx} onChange={(e) => setWithFx(e.target.checked)} />
            {t("history.showFx")}
          </label>
        }
      />
      {filter && (
        <p className="row">
          <span className="badge accent">
            {t("history.oneTrack", {
              track: rows[0] ? [rows[0].artist, rows[0].title].filter(Boolean).join(" – ") : `#${filter.trackId}`,
            })}
          </span>
          <Link to={`/library/${filter.trackId}`}>{t("history.backToTrack")}</Link>
          <Link to="/history">{t("history.everyPlay")}</Link>
        </p>
      )}
      <Card flush>
        {plays.isPending && <Loading />}
        {plays.isError && <ErrorView error={plays.error} />}
        {plays.data && rows.length === 0 && <Empty>{t("history.empty")}</Empty>}
        {rows.length > 0 && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("history.started")}</th>
                  <th>{t("history.track")}</th>
                  <th className="hide-narrow">{t("history.deck")}</th>
                  <th className="hide-narrow">{t("history.source")}</th>
                  <th>{t("history.played")}</th>
                </tr>
              </thead>
              <tbody>
                {groupByDay(rows).map(([day, items], index, days) => (
                  <DayRows key={day} day={day} plays={items} above={days[index - 1]?.[1].at(-1)} />
                ))}
              </tbody>
            </table>
          </div>
        )}
        {plays.hasNextPage && (
          <div className="pager">
            <span />
            <button
              type="button"
              className="btn small"
              disabled={plays.isFetchingNextPage}
              onClick={() => void plays.fetchNextPage()}
            >
              {t("history.earlier")}
            </button>
          </div>
        )}
      </Card>
    </>
  );
}

function groupByDay(plays: Play[]): [string, Play[]][] {
  const groups = new Map<string, Play[]>();
  for (const play of plays) {
    const day = fullDate(play.started_at_unix_ms);
    groups.set(day, [...(groups.get(day) ?? []), play]);
  }
  return [...groups.entries()];
}

/** A day's plays; `above` is the play shown just before them, the next day's first. */
function DayRows({ day, plays, above }: { day: string; plays: Play[]; above?: Play | undefined }) {
  return (
    <>
      <tr>
        <th colSpan={5} style={{ background: "var(--surface-2)" }}>
          {day}
        </th>
      </tr>
      {plays.map((play, index) => {
        const name = [play.artist, play.title].filter(Boolean).join(" – ") || t("history.unknownTrack");
        // Newest first: the play above started later.
        const later = index === 0 ? above : plays[index - 1];
        const change = later && clockChange(play.started_at_unix_ms, later.started_at_unix_ms);
        return (
          <Fragment key={play.id}>
            {change && (
              <tr className={`clock-change ${change.kind}`}>
                <td colSpan={5}>
                  <span>
                    <Icon name={change.kind === "summer" ? "sun" : "snow"} size={16} />
                    {t(change.kind === "summer" ? "history.toSummerTime" : "history.toWinterTime", {
                      from: change.from,
                      to: change.to,
                    })}
                  </span>
                </td>
              </tr>
            )}
            <tr>
              <td className="num">{clockTime(play.started_at_unix_ms)}</td>
              <td className="title-cell ellipsis">
                <span className="row" style={{ flexWrap: "nowrap", gap: "0.5rem" }}>
                  <CoverThumb url={play.cover_url} px={28} />
                  {play.track_id ? <Link to={`/library/${play.track_id}`}>{name}</Link> : name}
                </span>
              </td>
              <td className="hide-narrow">{play.deck.toUpperCase()}</td>
              <td className="hide-narrow">{sourceLabel(play.source)}</td>
              <td>
                {play.ended_at_unix_ms === null || play.ended_at_unix_ms === undefined ? (
                  <span className="badge accent">{t("nav.onAir")}</span>
                ) : (
                  <span className={play.end_reason === "skipped" ? "warn" : ""}>
                    {endingLabel(play.end_reason)}
                    {play.played_ms !== null && play.played_ms !== undefined && (
                      <span className="muted num"> · {duration(play.played_ms)}</span>
                    )}
                  </span>
                )}
              </td>
            </tr>
          </Fragment>
        );
      })}
    </>
  );
}
