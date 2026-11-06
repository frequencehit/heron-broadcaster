// SPDX-License-Identifier: AGPL-3.0-or-later

// Administration › Modules (admins): modules turned on and off for the
// whole installation (docs/modules.md).

import { useQuery } from "@tanstack/react-query";
import { type Schemas, api, unwrap } from "../api";
import { ModuleSwitches } from "../components/Modules";
import { Banner, Card, ErrorView, Loading, PageHead } from "../components/ui";
import { t } from "../i18n";
import { useAction, useCan } from "../lib/session";

export function ModulesPage() {
  const isAdmin = useCan("admin");
  const modules = useQuery({
    queryKey: ["modules"],
    queryFn: () => unwrap(api.GET("/api/v1/modules")),
    enabled: isAdmin,
  });
  const save = useAction((body: Schemas["Modules"]) => unwrap(api.PUT("/api/v1/modules", { body })), {
    success: t("categories.saved"),
    // Every station's modules follow, and what they show.
    invalidate: [["modules"]],
  });
  if (!isAdmin) {
    return <Banner kind="warn">{t("users.adminsOnly")}</Banner>;
  }
  return (
    <>
      <PageHead title={t("nav.modules")} description={t("modules.installationHelp")} />
      <Card>
        {modules.isPending && <Loading />}
        {modules.isError && <ErrorView error={modules.error} />}
        {modules.data && (
          <ModuleSwitches
            value={modules.data}
            canEdit={isAdmin}
            saving={save.isPending}
            onChange={(value) => save.mutate(value)}
          />
        )}
      </Card>
    </>
  );
}
