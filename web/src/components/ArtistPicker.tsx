// SPDX-License-Identifier: AGPL-3.0-or-later

// Searching the station's artists, by any of their names.

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { type Artist, api, query, unwrap } from "../api";
import { t } from "../i18n";
import { useDebounced } from "./TrackSearch";
import { ErrorView, Icon } from "./ui";

export function ArtistPicker({
  actions,
  exclude = [],
  placeholder = t("artistPicker.search"),
}: Readonly<{
  /** Buttons for each result. */
  actions: (artist: Artist) => ReactNode;
  exclude?: number[];
  placeholder?: string;
}>) {
  const [text, setText] = useState("");
  const q = useDebounced(text.trim());
  const results = useQuery({
    queryKey: ["artists", "search", q],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/api/v1/artists", { params: { query: query({ q, limit: 8 }) }, signal })),
    placeholderData: keepPreviousData,
    enabled: q.length > 0,
  });
  const found = (results.data?.artists ?? []).filter((artist) => !exclude.includes(artist.id));
  return (
    <div className="stack">
      <label className="row" style={{ flexWrap: "nowrap" }}>
        <span className="visually-hidden">{placeholder}</span>
        <Icon name="search" />
        <input
          className="input"
          type="search"
          placeholder={placeholder}
          value={text}
          onChange={(event) => setText(event.target.value)}
        />
      </label>
      {results.isError && <ErrorView error={results.error} />}
      {q && results.data && (
        <ul className="list">
          {found.length === 0 && <li className="empty">{t("artistPicker.none")}</li>}
          {found.map((artist) => (
            <li key={artist.id} className="queue-item">
              <div className="meta">
                <div className="ellipsis">{artist.name}</div>
                <div className="muted small ellipsis">
                  {t("common.tracks", { count: artist.tracks })}
                  {artist.names.length > 1 &&
                    ` · ${t("artistPicker.also", { names: artist.names.filter((n) => n.name !== artist.name).map((n) => n.name).join(", ") })}`}
                </div>
              </div>
              <div className="row" style={{ flexWrap: "nowrap" }}>
                {actions(artist)}
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
