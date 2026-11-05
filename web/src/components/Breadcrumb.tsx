// SPDX-License-Identifier: AGPL-3.0-or-later

// Where a page sits (docs/web.md): its section of the navigation, the page,
// and the page inside it. The link back to a list brings it back as it was
// left (its search, filters, sort and page).

import { t } from "../i18n";
import { placeOf } from "../lib/pages";
import { Link, lastHref, useLocation } from "../lib/router";

/** The breadcrumb of the current page, `title` its last level; none outside a section. */
export function Breadcrumb({ title }: Readonly<{ title: string }>) {
  const { pathname } = useLocation();
  const place = placeOf(pathname);
  if (!place?.section) {
    return null;
  }
  const page = t(place.page.label);
  return (
    <nav className={place.nested ? "breadcrumb nested" : "breadcrumb"} aria-label={t("app.breadcrumb")}>
      <ol>
        <li className="section">{t(place.section)}</li>
        {place.nested ? (
          <>
            <li className="parent">
              <Link to={lastHref(place.page.path)}>{page}</Link>
            </li>
            <li aria-current="page">{title}</li>
          </>
        ) : (
          <li aria-current="page">{page}</li>
        )}
      </ol>
    </nav>
  );
}
