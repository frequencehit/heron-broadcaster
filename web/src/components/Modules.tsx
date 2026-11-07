// SPDX-License-Identifier: AGPL-3.0-or-later

// Modules (docs/modules.md): their switches, the message a page of a
// module turned off shows instead, and the gate that shows it.

import type { ReactNode } from "react";
import { rich, t } from "../i18n";
import { MODULES, type ModuleKey, type Modules, useStationModules } from "../lib/modules";
import { Link } from "../lib/router";
import { useCan } from "../lib/session";
import { Banner, Loading } from "./ui";

/** A switch per module, with what it does and what turning it off stops. */
export function ModuleSwitches({
  value,
  installation,
  canEdit,
  saving,
  onChange,
}: Readonly<{
  value: Modules;
  /** For a station: the installation's, whose modules turned off cannot be turned on here. */
  installation?: Modules;
  canEdit: boolean;
  saving: boolean;
  onChange: (modules: Modules) => void;
}>) {
  return (
    <ul className="module-list">
      {/* A station's page leaves out the library's modules: the installation's only. */}
      {MODULES.filter((module) => installation === undefined || !module.installationOnly).map((module) => {
        const locked = installation !== undefined && !installation[module.key];
        const on = value[module.key] && !locked;
        return (
          <li key={module.key}>
            <label className="check">
              <input
                type="checkbox"
                checked={on}
                disabled={!canEdit || saving || locked}
                onChange={(e) => onChange({ ...value, [module.key]: e.target.checked })}
              />
              <strong>{t(module.label)}</strong>
              <span className={on ? "badge good" : "badge"}>{on ? t("modules.on") : t("modules.off")}</span>
            </label>
            <p className="small" style={{ margin: 0 }}>
              {t(module.help)}
            </p>
            <p className="muted small" style={{ margin: 0 }}>
              {locked ? t("modules.offForInstallation") : t(module.off)}
            </p>
          </li>
        );
      })}
    </ul>
  );
}

/** What a page of a module turned off shows. */
export function ModuleOff({ module }: Readonly<{ module: ModuleKey }>) {
  const isAdmin = useCan("admin");
  const modules = useStationModules();
  const label = MODULES.find((m) => m.key === module)?.label ?? "nav.settings";
  const forInstallation = modules.data ? !modules.data.installation[module] : false;
  const where = forInstallation ? (
    <Link to="/modules">{t("modules.whereInstallation")}</Link>
  ) : (
    <Link to="/settings">{t("modules.whereStation")}</Link>
  );
  return (
    <Banner kind="warn">
      <strong>{t("modules.turnedOff", { module: t(label) })}</strong>{" "}
      {isAdmin ? rich("modules.turnOnAt", { where }) : t("modules.askAdmin")}
    </Banner>
  );
}

/** The page of a module, or what says it is turned off. */
export function ModuleGate({ module, children }: Readonly<{ module: ModuleKey; children: ReactNode }>) {
  const modules = useStationModules();
  if (modules.isPending && modules.fetchStatus !== "idle") {
    return <Loading />;
  }
  if (modules.data && !modules.data.on[module]) {
    return <ModuleOff module={module} />;
  }
  return <>{children}</>;
}
