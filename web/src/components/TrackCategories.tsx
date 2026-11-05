// SPDX-License-Identifier: AGPL-3.0-or-later

// The categories of the tracks a page shows (decks, queue), each as a chip
// with its color and name. The live state carries no categories: they are
// asked once for every track shown (GET /tracks/categories).

import { useQuery } from "@tanstack/react-query";
import { type Schemas, api, unwrap } from "../api";
import { t } from "../i18n";
import { LabelChip } from "./SelectionActions";

export type CategoryRef = Schemas["CategoryRef"];

/** The categories of each of `trackIds` (library tracks), by track ID. */
export function useTrackCategories(trackIds: readonly (number | null | undefined)[]) {
  const ids = [...new Set(trackIds.filter((id): id is number => id != null))].sort((a, b) => a - b);
  const query = useQuery({
    queryKey: ["track-categories", ids],
    enabled: ids.length > 0,
    // Categories change rarely: the queue moving on asks for new tracks only.
    staleTime: 60_000,
    queryFn: () => unwrap(api.GET("/api/v1/tracks/categories", { params: { query: { ids: ids.join(",") } } })),
  });
  const byTrack = new Map<number, CategoryRef[]>();
  for (const track of query.data ?? []) {
    byTrack.set(track.track_id, track.categories);
  }
  return byTrack;
}

export function CategoryChips({ categories }: Readonly<{ categories: readonly CategoryRef[] | undefined }>) {
  if (!categories || categories.length === 0) {
    return null;
  }
  return (
    <span className="chips" aria-label={t("categories.ofTrack")}>
      {categories.map((category) => (
        <LabelChip key={category.id} label={category} />
      ))}
    </span>
  );
}
