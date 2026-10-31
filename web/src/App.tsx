// SPDX-License-Identifier: AGPL-3.0-or-later

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useState } from "react";
import { api, can, done, errorMessage, fetchStatus, unwrap } from "./api";
import { Banner, Icon, Loading, Logo } from "./components/ui";
import { type PlainKey, rich, setLanguage, t, toSetting, useLanguage } from "./i18n";
import { setTimeZone } from "./lib/format";
import { PAGES, isCurrent, placeOf } from "./lib/pages";
import { startLive, stopLive, useLive } from "./lib/live";
import { Link, match, navigate, useLocation } from "./lib/router";
import { ME_KEY, StationScope, UserContext, useCan, useMeQuery, useUser } from "./lib/session";
import { chooseStation, here, initialStation, stationRole, useStation } from "./lib/station";
import { applyFullWidth, applyTheme, storedFullWidth, storedTheme } from "./lib/theme";
import { Account } from "./pages/Account";
import { Artists } from "./pages/Artists";
import { Buffer } from "./pages/Buffer";
import { Categories } from "./pages/Categories";
import { ClockEditor, Clocks } from "./pages/Clocks";
import { Dashboard } from "./pages/Dashboard";
import { Eq } from "./pages/Eq";
import { History } from "./pages/History";
import { Labels } from "./pages/Labels";
import { MedleyEditor, Medleys } from "./pages/Medleys";
import { Library } from "./pages/Library";
import { Login } from "./pages/Login";
import { Outputs } from "./pages/Outputs";
import { Playout } from "./pages/Playout";
import { QueuePage } from "./pages/Queue";
import { Restrictions } from "./pages/Restrictions";
import { Import } from "./pages/Import";
import { Review } from "./pages/Review";
import { Rotation } from "./pages/Rotation";
import { Schedule } from "./pages/Schedule";
import { Settings } from "./pages/Settings";
import { Stations } from "./pages/Stations";
import { Statistics } from "./pages/Statistics";
import { Track } from "./pages/Track";
import { Trash } from "./pages/Trash";
import { Users } from "./pages/Users";
import { Webhooks } from "./pages/Webhooks";

applyTheme(storedTheme());
applyFullWidth(storedFullWidth());

export function App() {
  const me = useMeQuery();
  const user = me.data;
  // Every page re-renders in the new language at once.
  useLanguage();

  useEffect(() => {
    if (user) {
      applyTheme(user.theme);
      applyFullWidth(user.full_width);
      void setLanguage(toSetting(user.language));
      // The live WebSocket opens once the station shown is known (Shell).
    } else if (user === null) {
      stopLive();
    }
  }, [user]);

  if (me.isPending) {
    return <Loading label={t("app.contacting")} />;
  }
  if (me.isError) {
    return (
      <main className="login">
        <Banner kind="bad">{t("app.apiUnreachable", { error: errorMessage(me.error) })}</Banner>
      </main>
    );
  }
  if (!user) {
    return <Login />;
  }
  return (
    <UserContext.Provider value={user}>
      <Shell />
    </UserContext.Provider>
  );
}

function route(pathname: string): ReactNode {
  const track = match("/library/:id", pathname);
  if (track) {
    return <Track id={Number(track.id)} />;
  }
  const medley = match("/medleys/:id", pathname);
  if (medley) {
    return <MedleyEditor id={medley.id === "new" ? null : Number(medley.id)} />;
  }
  const clock = match("/clocks/:id", pathname);
  if (clock) {
    return <ClockEditor id={clock.id === "new" ? null : Number(clock.id)} />;
  }
  switch (pathname) {
    case "/":
      return <Dashboard />;
    case "/queue":
      return <QueuePage />;
    case "/history":
      return <History />;
    case "/statistics":
      return <Statistics />;
    case "/library":
      return <Library />;
    case "/buffer":
      return <Buffer />;
    case "/trash":
      return <Trash />;
    case "/categories":
      return <Categories />;
    case "/labels":
      return <Labels />;
    case "/medleys":
      return <Medleys />;
    case "/artists":
      return <Artists />;
    case "/clocks":
      return <Clocks />;
    case "/schedule":
      return <Schedule />;
    case "/restrictions":
      return <Restrictions />;
    case "/rotation":
      return <Rotation />;
    case "/review":
      return <Review />;
    case "/import":
      return <Import />;
    case "/playout":
      return <Playout />;
    case "/eq":
      return <Eq />;
    case "/outputs":
      return <Outputs />;
    case "/settings":
      return <Settings />;
    case "/webhooks":
      return <Webhooks />;
    case "/users":
      return <Users />;
    case "/stations":
      return <Stations />;
    case "/account":
      return <Account />;
    default:
      return (
        <Banner kind="warn">
          {rich("app.noPage", { back: <Link to="/">{t("app.backOnAir")}</Link> })}
        </Banner>
      );
  }
}

function Shell() {
  const { pathname } = useLocation();
  const [navOpen, setNavOpen] = useState(false);
  const isAdmin = useCan("admin");
  const me = useUser();
  const client = useQueryClient();
  const stations = useQuery({
    queryKey: ["stations"],
    queryFn: () => unwrap(api.GET("/api/v1/stations")),
  });
  const station = useStation();
  // The station the pages show: the one chosen last, or the first.
  useEffect(() => {
    const list = stations.data;
    if (list && (station === null || !list.some((s) => s.id === station))) {
      chooseStation(initialStation(me.id, list));
    }
  }, [stations.data, station, me.id]);
  const isStationAdmin = can(stationRole(me, station), "admin");
  const user = client.getQueryData<{ username: string }>(ME_KEY);
  const review = useQuery({
    queryKey: ["review-count"],
    queryFn: () => unwrap(api.GET("/api/v1/library/review", { params: { query: { limit: 1 } } })),
    refetchInterval: 60_000,
  });
  const reviewCount = review.data?.total ?? 0;
  const buffer = useQuery({
    queryKey: ["buffer"],
    queryFn: () => unwrap(api.GET("/api/v1/staging")),
    refetchInterval: 60_000,
  });
  const readyCount =
    buffer.data?.groups.reduce((sum, group) => sum + group.files.filter((f) => f.status === "ready").length, 0) ?? 0;
  const zone = useQuery({
    queryKey: ["settings", "station", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/station", { params: { path: here() } })),
    enabled: station !== null,
  });
  // Before the pages render, so they write times in station time.
  if (zone.data) {
    setTimeZone(zone.data.timezone);
  }
  // The live WebSocket is the station's: opened again for each station.
  useEffect(() => {
    if (station !== null) {
      stopLive();
      startLive();
    }
  }, [station]);
  const switchTo = (id: number) => {
    chooseStation(id);
    // Every query of the old station is another station's now.
    client.removeQueries({
      predicate: (query) => query.queryKey[0] !== ME_KEY[0] && query.queryKey[0] !== "stations",
    });
  };
  const visible = (page: (typeof PAGES)[number]) =>
    !page.admin || (page.station ? isStationAdmin : isAdmin);
  const scope = placeOf(pathname)?.page.station ?? false;

  useEffect(() => {
    setNavOpen(false);
  }, [pathname]);

  const signOut = async () => {
    await done(api.POST("/api/v1/auth/logout")).catch(() => {});
    stopLive();
    client.setQueryData(ME_KEY, null);
    client.removeQueries({ predicate: (query) => query.queryKey[0] !== ME_KEY[0] });
    navigate("/");
  };

  let section: PlainKey | undefined;
  return (
    <div className={navOpen ? "shell nav-open" : "shell"}>
      {navOpen && (
        <div className="scrim" onClick={() => setNavOpen(false)} aria-hidden="true" />
      )}
      <aside className="sidebar" aria-label={t("nav.label")}>
        <Link to="/" className="brand">
          <Logo /> Heron
        </Link>
        {(stations.data?.length ?? 0) > 1 && (
          <label className="station-switcher">
            <span className="visually-hidden">{t("stations.switch")}</span>
            <select
              className="input"
              value={station ?? ""}
              onChange={(event) => switchTo(Number(event.target.value))}
            >
              {stations.data?.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name} ({s.code}){s.enabled ? "" : ` · ${t("stations.disabled")}`}
                </option>
              ))}
            </select>
          </label>
        )}
        <nav className="nav">
          {PAGES.filter(visible).map((page) => {
            const heading = page.section !== undefined && page.section !== section;
            if (page.section) {
              section = page.section;
            }
            return (
              <div key={page.path} style={{ display: "contents" }}>
                {heading && page.section && <div className="nav-section">{t(page.section)}</div>}
                <Link
                  to={page.path}
                  aria-current={isCurrent(page.path, pathname) ? "page" : undefined}
                >
                  <Icon name={page.icon} /> {t(page.label)}
                  {page.path === "/review" && reviewCount > 0 && (
                    <span className="count" title={t("nav.reviewCount")}>
                      {reviewCount}
                    </span>
                  )}
                  {page.path === "/buffer" && readyCount > 0 && (
                    <span className="count" title={t("nav.bufferCount")}>
                      {readyCount}
                    </span>
                  )}
                </Link>
              </div>
            );
          })}
        </nav>
        <div className="sidebar-footer">
          <nav className="nav">
            <Link to="/account" aria-current={pathname === "/account" ? "page" : undefined}>
              <Icon name="user" /> <span className="ellipsis">{user?.username ?? t("nav.account")}</span>
            </Link>
            <a
              href="/"
              onClick={(event) => {
                event.preventDefault();
                void signOut();
              }}
            >
              <Icon name="logout" /> {t("nav.signOut")}
            </a>
          </nav>
        </div>
      </aside>
      <div className="main">
        <div className="topbar">
          <button
            type="button"
            className="btn ghost icon"
            aria-label={t("nav.menu")}
            aria-expanded={navOpen}
            onClick={() => setNavOpen(true)}
          >
            <Icon name="menu" />
          </button>
          <Link to="/" className="brand">
            <Logo size={20} /> Heron
          </Link>
          <span className="spacer" />
          <OnAirBadge />
        </div>
        <main className="content">
          {station === null ? (
            <Loading label={t("app.contacting")} />
          ) : (
            <StationScope.Provider value={scope} key={station}>
              <Warnings />
              {route(pathname)}
            </StationScope.Provider>
          )}
        </main>
      </div>
    </div>
  );
}

function OnAirBadge() {
  const player = useLive((live) => live.player);
  const onAir = player?.decks.some((deck) => deck.on_air) && !player.paused;
  return onAir ? (
    <span className="on-air-pill">
      <span className="dot" style={{ background: "#fff" }} /> {t("app.onAirBadge")}
    </span>
  ) : null;
}

/** Problems worth knowing on every page. */
function Warnings() {
  const connected = useLive((live) => live.connected);
  const player = useLive((live) => live.player);
  const status = useQuery({ queryKey: ["status"], queryFn: () => fetchStatus(), refetchInterval: 15_000 });
  const engine = useQuery({
    queryKey: ["engine"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/engine", { params: { path: here() } })),
    refetchInterval: 15_000,
  });
  const [disconnectedSince, setDisconnectedSince] = useState<number | null>(null);
  useEffect(() => {
    setDisconnectedSince(connected ? null : Date.now());
  }, [connected]);
  // Brief reconnections are not worth a banner.
  const [, tick] = useState(0);
  useEffect(() => {
    if (connected) {
      return;
    }
    const timer = setTimeout(() => tick((n) => n + 1), 3_000);
    return () => clearTimeout(timer);
  }, [connected]);

  const warnings: ReactNode[] = [];
  if (status.isError) {
    warnings.push(t("app.apiUnreachable", { error: errorMessage(status.error) }));
  } else if (!connected && disconnectedSince !== null && Date.now() - disconnectedSince > 2_500) {
    warnings.push(t("app.liveDisconnected"));
  }
  if (status.data?.database === "unavailable") {
    warnings.push(t("app.databaseUnavailable"));
  }
  if (player === null || status.data?.engine === "unavailable") {
    warnings.push(t("app.engineUnreachable"));
  }
  if (engine.data?.config.state === "failed") {
    warnings.push(
      rich("app.configRefused", {
        error: engine.data.config.error,
        link: <Link to="/outputs">{t("app.checkOutputs")}</Link>,
      }),
    );
  }
  const clock = status.data?.clock.warning;
  if (clock) {
    warnings.push(
      rich("app.clockWarning", { label: <strong>{t("app.clockLabel")}</strong>, warning: clock }),
    );
  }
  return (
    <>
      {warnings.map((warning, index) => (
        <Banner key={index} kind={index === 0 && status.isError ? "bad" : "warn"}>
          {warning}
        </Banner>
      ))}
    </>
  );
}
