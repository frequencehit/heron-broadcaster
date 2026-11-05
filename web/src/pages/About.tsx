// SPDX-License-Identifier: AGPL-3.0-or-later

// Which build runs, on what machine and how it is set up. Everyone sees the
// build; the machine, the dependencies and the configuration are for admins
// (the API sends them to nobody else). See docs/web.md.

import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { type About as AboutData, type ApiStatus, api, fetchStatus, unwrap } from "../api";
import { Banner, Card, ErrorView, Loading, PageHead, useToast } from "../components/ui";
import { t } from "../i18n";
import { clockText, configText, diagnostics, serviceName, uptime } from "../lib/about";
import { buildLabel, differentBuilds, webBuild } from "../lib/version";

const REPOSITORY = "https://github.com/dehy/heron-broadcaster";

export function About() {
  const toast = useToast();
  const about = useQuery({ queryKey: ["about"], queryFn: () => unwrap(api.GET("/api/v1/about")) });
  const status = useQuery({ queryKey: ["status"], queryFn: ({ signal }) => fetchStatus(signal) });
  const web = webBuild();
  const apiBuild = status.data ? { version: status.data.version, revision: status.data.revision } : null;

  const copy = async () => {
    if (!about.data || !apiBuild) {
      return;
    }
    try {
      await navigator.clipboard.writeText(diagnostics(about.data, web, apiBuild));
      toast(t("about.copied"));
    } catch {
      toast(t("about.copyFailed"), true);
    }
  };

  return (
    <>
      <PageHead
        title={t("about.title")}
        description={t("about.description")}
        actions={
          <button type="button" className="btn" onClick={copy} disabled={!about.data || !apiBuild}>
            {t("about.copy")}
          </button>
        }
      />
      {about.isPending && <Loading />}
      {about.error && <ErrorView error={about.error} />}
      {about.data && (
        <div className="stack">
          <BuildCard about={about.data} status={status.data} />
          {about.data.runtime && <RuntimeCard about={about.data} />}
          {about.data.dependencies && <DependenciesCard about={about.data} />}
          {about.data.configuration?.map((config) => (
            <Card
              key={`${config.service}/${config.instance ?? ""}`}
              title={`${t("about.configuration")}: ${serviceName(config.service, config.instance)}`}
            >
              <p className="muted">{t("about.configurationHint")}</p>
              <table className="table">
                <tbody>
                  {config.items.map((item) => (
                    <tr key={item.name}>
                      <td className="mono">{item.name}</td>
                      <td>{item.kind === "flag" ? <Flag on={item.value} /> : configText(item)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </Card>
          ))}
          {!about.data.runtime && <p className="muted">{t("about.adminsOnly")}</p>}
          <LinksCard />
        </div>
      )}
      {differentBuilds(web, apiBuild ?? web) && apiBuild && (
        <Banner kind="warn">
          {t("about.differentBuilds", { web: buildLabel(web), api: buildLabel(apiBuild) })}
        </Banner>
      )}
    </>
  );
}

function Flag({ on }: Readonly<{ on: boolean }>) {
  return <span className={on ? "badge good" : "badge"}>{on ? t("about.on") : t("about.off")}</span>;
}

function BuildCard({ about, status }: Readonly<{ about: AboutData; status: ApiStatus | undefined }>) {
  const web = webBuild();
  return (
    <Card title={t("about.build")} flush>
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>{t("about.service")}</th>
              <th>{t("about.version")}</th>
              <th>{t("about.commit")}</th>
              <th>{t("about.built")}</th>
              <th>{t("about.status")}</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td>{t("about.web")}</td>
              <td>{web.version}</td>
              <td className="mono">{web.revision || "–"}</td>
              <td>–</td>
              <td>
                {status && !differentBuilds(web, status) ? (
                  <span className="badge good">{t("about.sameBuild")}</span>
                ) : null}
              </td>
            </tr>
            {about.services.map((service) => (
              <tr key={`${service.service}/${service.instance ?? ""}`}>
                <td>{serviceName(service.service, service.instance)}</td>
                <td>{service.version ?? "–"}</td>
                <td className="mono">{service.revision ?? "–"}</td>
                <td>{service.build_date ?? "–"}</td>
                <td>
                  {service.running ? (
                    <span className="badge good">{t("about.running")}</span>
                  ) : (
                    <span className="badge bad">{service.version ? t("about.stopped") : t("about.neverSeen")}</span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Card>
  );
}

function RuntimeCard({ about }: Readonly<{ about: AboutData }>) {
  const runtime = about.runtime;
  if (!runtime) {
    return null;
  }
  return (
    <Card title={t("about.runtime")} flush>
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>{t("about.service")}</th>
              <th>{t("about.uptime")}</th>
              <th>{t("about.system")}</th>
            </tr>
          </thead>
          <tbody>
            {runtime.hosts.map((host) => (
              <tr key={`${host.service}/${host.instance ?? ""}`}>
                <td>{serviceName(host.service, host.instance)}</td>
                <td>{uptime(host.uptime_seconds)}</td>
                <td>
                  {[host.os, host.kernel, host.arch].filter(Boolean).join(", ") || "–"}
                  {host.container && <span className="badge"> {t("about.container")}</span>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="card-body">
        <dl className="facts">
          <Fact label={t("about.timezone")}>{runtime.timezone ?? "–"}</Fact>
          <Fact label={t("about.clock")}>{clockText(runtime.clock)}</Fact>
          <Fact label={t("about.cores")}>{runtime.cpu_cores}</Fact>
          <Fact label={t("about.memory")}>{runtime.memory_mib} MiB</Fact>
        </dl>
      </div>
    </Card>
  );
}

function DependenciesCard({ about }: Readonly<{ about: AboutData }>) {
  const dependencies = about.dependencies;
  if (!dependencies) {
    return null;
  }
  const { database, ffmpeg } = dependencies;
  return (
    <Card title={t("about.dependencies")}>
      <dl className="facts">
        <Fact label={t("about.database")}>
          {database.version ?? "–"}
          {database.migrations !== null && database.migrations !== undefined && (
            <span className="muted"> · {t("about.migrations", { count: database.migrations })}</span>
          )}
        </Fact>
        <Fact label={t("about.ffmpeg")}>{ffmpeg ?? t("about.builtIn")}</Fact>
      </dl>
    </Card>
  );
}

function Fact({ label, children }: Readonly<{ label: string; children: ReactNode }>) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </>
  );
}

function LinksCard() {
  return (
    <Card title={t("about.links")}>
      <ul>
        <li>
          <a href={`${REPOSITORY}/tree/main/docs`}>{t("about.docs")}</a>
        </li>
        <li>
          <a href={`${REPOSITORY}/blob/main/CHANGELOG.md`}>{t("about.changelog")}</a>
        </li>
        <li>
          <a href={REPOSITORY}>{t("about.repository")}</a>
        </li>
      </ul>
      <p className="muted">{t("about.license")}</p>
    </Card>
  );
}
