// SPDX-License-Identifier: AGPL-3.0-or-later

// Modules turned on and off (docs/modules.md): what is on for the station
// shown, so that the pages, settings and widgets of a module turned off
// are not shown.

import { useQuery } from "@tanstack/react-query";
import { type Schemas, api, unwrap } from "../api";
import type { PlainKey } from "../i18n";
import { useStation } from "./station";

export type Modules = Schemas["Modules"];
export type ModuleKey = keyof Modules;

/** A module of the list, with its texts. */
export interface ModuleEntry {
  key: ModuleKey;
  label: PlainKey;
  help: PlainKey;
  off: PlainKey;
  /** Of the library, which the stations share: turned off for the installation only. */
  installationOnly?: boolean;
}

/** The modules, in the order the pages list them. */
export const MODULES: ModuleEntry[] = [
  { key: "medleys", label: "nav.medleys", help: "modules.medleysHelp", off: "modules.medleysOff" },
  { key: "cover_art", label: "modules.coverArt", help: "modules.coverArtHelp", off: "modules.coverArtOff" },
  {
    key: "listener_statistics",
    label: "modules.listenerStatistics",
    help: "modules.listenerStatisticsHelp",
    off: "modules.listenerStatisticsOff",
  },
  { key: "webhooks", label: "nav.webhooks", help: "modules.webhooksHelp", off: "modules.webhooksOff" },
  {
    key: "restrictions",
    label: "nav.restrictions",
    help: "modules.restrictionsHelp",
    off: "modules.restrictionsOff",
  },
  { key: "eq", label: "modules.eq", help: "modules.eqHelp", off: "modules.eqOff" },
  { key: "processing", label: "nav.agc", help: "modules.processingHelp", off: "modules.processingOff" },
  { key: "trash", label: "nav.trash", help: "modules.trashHelp", off: "modules.trashOff", installationOnly: true },
  { key: "review", label: "nav.review", help: "modules.reviewHelp", off: "modules.reviewOff", installationOnly: true },
  { key: "artists", label: "nav.artists", help: "modules.artistsHelp", off: "modules.artistsOff", installationOnly: true },
  { key: "labels", label: "nav.labels", help: "modules.labelsHelp", off: "modules.labelsOff", installationOnly: true },
  { key: "dedications", label: "nav.dedications", help: "modules.dedicationsHelp", off: "modules.showOff" },
  { key: "match", label: "nav.match", help: "modules.matchHelp", off: "modules.showOff" },
  { key: "top30", label: "nav.top30", help: "modules.top30Help", off: "modules.showOff" },
  { key: "likes", label: "nav.likes", help: "modules.likesHelp", off: "modules.likesOff" },
  {
    key: "certification",
    label: "modules.certification",
    help: "modules.certificationHelp",
    off: "modules.certificationOff",
  },
  { key: "status_bar", label: "modules.statusBar", help: "modules.statusBarHelp", off: "modules.statusBarOff" },
];

/**
 * What is shown until the station's modules are known: their defaults,
 * everything on but the shows, Likes aside, and certification (docs/modules.md).
 */
export const ALL_ON: Modules = {
  medleys: true,
  cover_art: true,
  listener_statistics: true,
  webhooks: true,
  restrictions: true,
  eq: true,
  processing: true,
  trash: true,
  review: true,
  artists: true,
  labels: true,
  dedications: false,
  match: false,
  top30: false,
  likes: true,
  certification: false,
  status_bar: true,
};

export function moduleKey(station: number | null): readonly unknown[] {
  return ["modules", station];
}

/** The station's modules; `undefined` until they are known. */
export function useStationModules() {
  const station = useStation();
  return useQuery({
    queryKey: moduleKey(station),
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{id}/modules", { params: { path: { id: station ?? 0 } } })),
    enabled: station !== null,
    staleTime: 60_000,
  });
}

/**
 * Whether a module is on for the station shown. On until it is known (or
 * when it cannot be read): a module turned off then disappears.
 */
export function useModule(key: ModuleKey): boolean {
  const modules = useStationModules();
  return (modules.data?.on ?? ALL_ON)[key];
}
