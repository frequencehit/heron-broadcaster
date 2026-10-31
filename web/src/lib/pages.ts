// SPDX-License-Identifier: AGPL-3.0-or-later

// The pages of the navigation, by section: the sidebar lists them and the
// breadcrumb (docs/web.md) says where a page sits among them.

import type { IconName } from "../components/ui";
import type { PlainKey } from "../i18n";

export interface Page {
  path: string;
  label: PlainKey;
  icon: IconName;
  section?: PlainKey;
  /** Admins only: of the station shown for a station's page, shared admins otherwise. */
  admin?: boolean;
  /** One of a station's pages: it shows the chosen station, and its actions follow the role there. */
  station?: boolean;
}

export const PAGES: Page[] = [
  { path: "/", label: "nav.onAir", icon: "air", station: true },
  { path: "/queue", label: "nav.queue", icon: "queue", station: true },
  { path: "/history", label: "nav.history", icon: "history", station: true },
  { path: "/statistics", label: "nav.statistics", icon: "chart", station: true },
  { path: "/library", label: "nav.tracks", icon: "library", section: "nav.library" },
  { path: "/buffer", label: "nav.buffer", icon: "upload" },
  { path: "/labels", label: "nav.labels", icon: "label" },
  { path: "/medleys", label: "nav.medleys", icon: "queue" },
  { path: "/artists", label: "nav.artists", icon: "mic" },
  { path: "/review", label: "nav.review", icon: "review" },
  { path: "/trash", label: "nav.trash", icon: "trash" },
  { path: "/import", label: "nav.import", icon: "upload", admin: true },
  { path: "/clocks", label: "nav.clocks", icon: "clock", section: "nav.programming", station: true },
  { path: "/schedule", label: "nav.schedule", icon: "calendar", station: true },
  { path: "/categories", label: "nav.categories", icon: "tag" },
  { path: "/restrictions", label: "nav.restrictions", icon: "ban" },
  { path: "/rotation", label: "nav.rotation", icon: "log", station: true },
  { path: "/playout", label: "nav.playout", icon: "fade", section: "nav.station", station: true },
  { path: "/eq", label: "nav.eq", icon: "eq", station: true },
  { path: "/outputs", label: "nav.outputs", icon: "output", station: true },
  { path: "/settings", label: "nav.settings", icon: "settings", station: true },
  { path: "/webhooks", label: "nav.webhooks", icon: "bolt", admin: true, station: true },
  { path: "/stations", label: "nav.stations", icon: "air", admin: true },
  { path: "/users", label: "nav.users", icon: "users", admin: true },
  { path: "/backups", label: "nav.backups", icon: "download", admin: true },
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
