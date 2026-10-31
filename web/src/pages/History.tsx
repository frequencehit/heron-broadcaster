// SPDX-License-Identifier: AGPL-3.0-or-later

// What was played, newest first.

import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { type Play, api, query, unwrap } from "../api";
import { Card, Empty, ErrorView, Loading, PageHead } from "../components/ui";
import { t } from "../i18n";
import { clockTime, duration, fullDate } from "../lib/format";
import { endingLabel, sourceLabel } from "../lib/labels";
import { useLive } from "../lib/live";
import { Link } from "../lib/router";
import { here } from "../lib/station";

const PAGE = 100;

export function History() {
  const [withFx, setWithFx] = useState(false);
  // New plays appear as they start.
  const latest = useLive((live) => live.events[0]?.at_unix_ms);
  const plays = useInfiniteQuery({
    queryKey: ["history", withFx, latest],
    initialPageParam: undefined as number | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/history", {
          params: { path: here(), query: query({ before: pageParam, exclude_fx: !withFx, limit: PAGE }) },
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
                {groupByDay(rows).map(([day, items]) => (
                  <DayRows key={day} day={day} plays={items} />
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

function DayRows({ day, plays }: { day: string; plays: Play[] }) {
  return (
    <>
      <tr>
        <th colSpan={5} style={{ background: "var(--surface-2)" }}>
          {day}
        </th>
      </tr>
      {plays.map((play) => {
        const name = [play.artist, play.title].filter(Boolean).join(" – ") || t("history.unknownTrack");
        return (
          <tr key={play.id}>
            <td className="num">{clockTime(play.started_at_unix_ms)}</td>
            <td className="title-cell ellipsis">
              {play.track_id ? <Link to={`/library/${play.track_id}`}>{name}</Link> : name}
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
        );
      })}
    </>
  );
}
