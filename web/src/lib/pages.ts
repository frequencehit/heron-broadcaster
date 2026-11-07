// SPDX-License-Identifier: AGPL-3.0-or-later

// The pages of the navigation, by section: the sidebar lists them and the
// breadcrumb (docs/web.md) says where a page sits among them.

import type { IconName } from "../components/ui";
import type { PlainKey } from "../i18n";
import type { ModuleKey } from "./modules";

export interface Page {
  path: string;
  label: PlainKey;
  icon: IconName;
  section?: PlainKey;
  /** Admins only: of the station shown for a station's page, shared admins otherwise. */
  admin?: boolean;
  /** One of a station's pages: it shows the chosen station, and its actions follow the role there. */
  station?: boolean;
  /** The page of a module: hidden while the module is off for the station shown (docs/modules.md). */
  module?: ModuleKey;
}

export const PAGES: Page[] = [
  { path: "/", label: "nav.onAir", icon: "air", station: true },
  { path: "/queue", label: "nav.queue", icon: "queue", station: true },
  { path: "/history", label: "nav.history", icon: "history", station: true },
  { path: "/statistics", label: "nav.statistics", icon: "chart", station: true, module: "listener_statistics" },
  { path: "/library", label: "nav.tracks", icon: "library", section: "nav.library" },
  { path: "/import-music", label: "nav.buffer", icon: "upload" },
  { path: "/labels", label: "nav.labels", icon: "label", module: "labels" },
  { path: "/medleys", label: "nav.medleys", icon: "queue", module: "medleys" },
  { path: "/artists", label: "nav.artists", icon: "mic", module: "artists" },
  { path: "/to-organize", label: "nav.review", icon: "review", module: "review" },
  { path: "/trash", label: "nav.trash", icon: "trash", module: "trash" },
  { path: "/migrate", label: "nav.import", icon: "upload", admin: true },
  { path: "/clocks", label: "nav.clocks", icon: "clock", section: "nav.programming", station: true },
  { path: "/schedule", label: "nav.schedule", icon: "calendar", station: true },
  { path: "/categories", label: "nav.categories", icon: "tag" },
  { path: "/restrictions", label: "nav.restrictions", icon: "ban", module: "restrictions" },
  { path: "/rotation-log", label: "nav.rotation", icon: "log", station: true },
  // The shows (docs/listener-interaction.md), each a module: each names its
  // section, so that it is listed under it while the others are off.
  { path: "/shows/dedications", label: "nav.dedications", icon: "mic", section: "nav.shows", station: true, module: "dedications" },
  { path: "/shows/match", label: "nav.match", icon: "bolt", section: "nav.shows", station: true, module: "match" },
  { path: "/shows/top30", label: "nav.top30", icon: "chart", section: "nav.shows", station: true, module: "top30" },
  { path: "/shows/likes", label: "nav.likes", icon: "like", section: "nav.shows", station: true, module: "likes" },
  { path: "/playout", label: "nav.playout", icon: "fade", section: "nav.station", station: true },
  { path: "/eq", label: "nav.eq", icon: "eq", station: true, module: "eq" },
  { path: "/processing", label: "nav.agc", icon: "agc", station: true, module: "processing" },
  { path: "/outputs", label: "nav.outputs", icon: "output", station: true },
  { path: "/settings", label: "nav.settings", icon: "settings", station: true },
  { path: "/webhooks", label: "nav.webhooks", icon: "bolt", admin: true, station: true, module: "webhooks" },
  { path: "/stations", label: "nav.stations", icon: "air", admin: true },
  { path: "/users", label: "nav.users", icon: "users", admin: true },
  { path: "/backups", label: "nav.backups", icon: "download", admin: true },
  { path: "/modules", label: "nav.modules", icon: "settings", section: "nav.administration", admin: true },
];

/** Whether `pathname` is the page at `page`, or a page inside it. */
export function isCurrent(page: string, pathname: string): boolean {
  return page === "/" ? pathname === "/" : pathname === page || pathname.startsWith(`${page}/`);
}

/** Where a path sits: its page of the navigation and that page's section. */
export interface Place {
  page: Page;
  /** The section the page is listed under; none for the pages above the sections. */
  section?: PlainKey | undefined;
  /** A page inside the page (`/library/12`), not the page itself. */
  nested: boolean;
}

export function placeOf(pathname: string): Place | null {
  let section: PlainKey | undefined;
  for (const page of PAGES) {
    section = page.section ?? section;
    if (isCurrent(page.path, pathname)) {
      return { page, section, nested: pathname !== page.path };
    }
  }
  return null;
}
