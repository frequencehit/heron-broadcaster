// SPDX-License-Identifier: AGPL-3.0-or-later

// Station badges and pickers, shown only when Heron runs several stations
// (docs/stations.md#web-ui).

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { api, unwrap } from "../api";
import { t } from "../i18n";
import { ago } from "../lib/format";
import {
  type FilterPage,
  type StationFilter as Filter,
  type StationInfo,
  keepFilter,
  stationFilter,
  storedFilter,
  useStation,
} from "../lib/station";

/** The stations the user sees, by position. */
export function useStations(): StationInfo[] {
  const stations = useQuery({
    queryKey: ["stations"],
    queryFn: () => unwrap(api.GET("/api/v1/stations")),
  });
  return stations.data ?? [];
}

/** Whether there are several stations: badges and pickers only show then. */
export function useSeveralStations(): boolean {
  return useStations().length > 1;
}

/** The codes of `ids` as badges; `all` when given for no station at all. */
export function StationBadges({ ids, emptyMeansAll }: Readonly<{ ids: number[]; emptyMeansAll?: boolean }>) {
  const stations = useStations();
  if (stations.length < 2) {
    return null;
  }
  if (ids.length === 0 && emptyMeansAll) {
    return <span className="badge">{t("stations.all")}</span>;
  }
  // In the order given: a medley's first station first.
  const shown = ids
    .map((id) => stations.find((station) => station.id === id))
    .filter((station): station is StationInfo => station !== undefined);
  return (
    <span className="chips">
      {shown.map((station) => (
        <span key={station.id} className="badge accent" title={station.name}>
          {station.code}
        </span>
      ))}
    </span>
  );
}

/** Checkboxes to tag something with stations; nothing shown with one station. */
export function StationChecks({
  value,
  onChange,
  disabled,
}: Readonly<{
  value: number[];
  onChange: (ids: number[]) => void;
  disabled?: boolean;
}>) {
  const stations = useStations();
  if (stations.length < 2) {
    return null;
  }
  return (
    <span className="station-checks">
      {stations.map((station) => (
        <label key={station.id} className="check">
          <input
            type="checkbox"
            disabled={disabled}
            checked={value.includes(station.id)}
            onChange={(event) =>
              onChange(
                event.target.checked
                  ? [...value, station.id]
                  : value.filter((id) => id !== station.id),
              )
            }
          />{" "}
          {station.name} <span className="badge">{station.code}</span>
        </label>
      ))}
    </span>
  );
}

type Plays = import("../api").Schemas["Plays"];

/** Plays and last play on each station; nothing with one station. */
export function PlaysByStation({ plays }: Readonly<{ plays: Plays | null | undefined }>) {
  const stations = useStations();
  if (stations.length < 2 || !plays) {
    return null;
  }
  return (
    <span className="chips">
      {stations.map((station) => {
        const on = plays.stations.find((s) => s.station_id === station.id);
        return (
          <span key={station.id} className="chip" title={station.name}>
            <strong>{station.code}</strong>{" "}
            {on?.last_played_at_unix_ms
              ? t("track.playCountLast", { count: on.play_count, ago: ago(on.last_played_at_unix_ms) })
              : t("track.playCount", { count: on?.play_count ?? 0 })}
          </span>
        );
      })}
    </span>
  );
}

/** A page's station filter (docs/web.md#station-filter). */
export interface StationFilterState {
  stations: StationInfo[];
  value: Filter;
  choose: (value: Filter) => void;
  /** The station to filter on; none with all stations, or with one station. */
  stationId: number | undefined;
}

/** The station filter of `page`: the one kept, else the switcher's station. */
export function useStationFilter(page: FilterPage): StationFilterState {
  const stations = useStations();
  const switcher = useStation();
  const [stored, setStored] = useState(() => storedFilter(page));
  const value = stationFilter(
    stored,
    stations.map((station) => station.id),
    switcher,
  );
  const choose = (next: Filter) => {
    keepFilter(page, next);
    setStored(String(next));
  };
  return {
    stations,
    value,
    choose,
    stationId: stations.length > 1 && value !== "all" ? value : undefined,
  };
}

/** Picks a page's station filter; nothing shown with one station. */
export function StationFilter({ filter, disabled }: Readonly<{ filter: StationFilterState; disabled?: boolean }>) {
  if (filter.stations.length < 2) {
    return null;
  }
  return (
    <label className="row small">
      <span className="muted">{t("stations.filter")}</span>
      <select
        className="input"
        disabled={disabled}
        value={disabled ? "all" : String(filter.value)}
        onChange={(event) => filter.choose(event.target.value === "all" ? "all" : Number(event.target.value))}
      >
        {filter.stations.map((station) => (
          <option key={station.id} value={station.id}>
            {station.name}
          </option>
        ))}
        <option value="all">{t("stations.filterAll")}</option>
      </select>
    </label>
  );
}
