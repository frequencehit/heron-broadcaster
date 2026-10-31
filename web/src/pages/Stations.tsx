// SPDX-License-Identifier: AGPL-3.0-or-later

// The stations (admins): each with its own engine (docs/stations.md).

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { api, done, unwrap } from "../api";
import { Banner, Card, ConfirmButton, ErrorView, Field, Icon, Loading, Modal, PageHead } from "../components/ui";
import { t } from "../i18n";
import { useAction, useCan } from "../lib/session";
import { type StationInfo, firstStation } from "../lib/station";

interface Draft {
  name: string;
  code: string;
  enabled: boolean;
  position: number;
}

export function Stations() {
  const isAdmin = useCan("admin");
  const stations = useQuery({
    queryKey: ["stations"],
    queryFn: () => unwrap(api.GET("/api/v1/stations")),
  });
  const [editing, setEditing] = useState<StationInfo | "new" | null>(null);
  const remove = useAction(
    (id: number) => done(api.DELETE("/api/v1/stations/{id}", { params: { path: { id } } })),
    { success: t("stations.deleted"), invalidate: [["stations"]] },
  );

  if (!isAdmin) {
    return <Banner kind="warn">{t("users.adminsOnly")}</Banner>;
  }
  const first = stations.data ? firstStation(stations.data)?.id : undefined;
  return (
    <>
      <PageHead
        title={t("nav.stations")}
        actions={
          <button type="button" className="btn primary" onClick={() => setEditing("new")}>
            <Icon name="plus" /> {t("stations.new")}
          </button>
        }
      />
      <p className="muted">{t("stations.intro")}</p>
      <Card flush>
        {stations.isPending && <Loading />}
        {stations.isError && <ErrorView error={stations.error} />}
        {stations.data && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("stations.name")}</th>
                  <th>{t("stations.code")}</th>
                  <th className="hide-narrow">{t("stations.engine")}</th>
                  <th>
                    <span className="visually-hidden">{t("library.actions")}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {stations.data.map((station) => (
                  <tr key={station.id}>
                    <td>
                      {station.name}{" "}
                      {station.id === first && <span className="badge accent">{t("stations.first")}</span>}{" "}
                      {!station.enabled && <span className="badge">{t("stations.disabled")}</span>}
                    </td>
                    <td>
                      <span className="badge accent">{station.code}</span>
                    </td>
                    <td className="hide-narrow">
                      <EngineVariable station={station} />
                    </td>
                    <td className="actions">
                      <button type="button" className="btn small" onClick={() => setEditing(station)}>
                        {t("stations.edit")}
                      </button>
                      {station.id !== first && (
                        <ConfirmButton
                          className="btn ghost small icon danger"
                          title={t("categories.delete", { name: station.name })}
                          confirm={t("categories.deleteButton")}
                          question={<p>{t("stations.deleteQuestion", { name: station.name })}</p>}
                          onConfirm={() => remove.mutate(station.id)}
                        >
                          <Icon name="trash" />
                        </ConfirmButton>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
      {editing && (
        <StationDialog
          station={editing === "new" ? null : editing}
          first={editing !== "new" && editing.id === first}
          onClose={() => setEditing(null)}
        />
      )}
    </>
  );
}

function StationDialog({
  station,
  first,
  onClose,
}: {
  station: StationInfo | null;
  first: boolean;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState<Draft>({
    name: station?.name ?? "",
    code: station?.code ?? "",
    enabled: station?.enabled ?? true,
    position: station?.position ?? 0,
  });
  const body = draft;
  // A station's code names its engine's variable: only the first's can change.
  const codeLocked = station !== null && !first;
  const save = useAction(
    () =>
      station
        ? unwrap(api.PUT("/api/v1/stations/{id}", { params: { path: { id: station.id } }, body }))
        : unwrap(api.POST("/api/v1/stations", { body })),
    {
      success: station ? t("categories.saved") : t("stations.created"),
      invalidate: [["stations"]],
      onSuccess: onClose,
    },
  );
  const set = (patch: Partial<Draft>) => setDraft((current) => ({ ...current, ...patch }));
  return (
    <Modal
      title={station ? t("stations.editTitle", { name: station.name }) : t("stations.new")}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="submit" form="station-form" className="btn primary" disabled={save.isPending}>
            {t("stations.save")}
          </button>
        </>
      }
    >
      <form
        id="station-form"
        className="stack"
        onSubmit={(event) => {
          event.preventDefault();
          save.mutate(undefined);
        }}
      >
        <Field label={t("stations.name")}>
          <input className="input" value={draft.name} onChange={(e) => set({ name: e.target.value })} required />
        </Field>
        <Field
          label={t("stations.code")}
          hint={codeLocked ? t("stations.codeLocked") : first ? t("stations.codeFirstHint") : t("stations.codeHint")}
        >
          <input
            className="input"
            value={draft.code}
            pattern="[a-z0-9]{1,32}"
            onChange={(e) => set({ code: e.target.value })}
            disabled={codeLocked}
            required
          />
        </Field>
        <Field label={t("stations.engine")} hint={t("stations.engineVariableHint")}>
          {station ? (
            <EngineVariable station={station} />
          ) : (
            <code className="mono">HERON_ENGINE_URL_{draft.code.toUpperCase() || "…"}</code>
          )}
        </Field>
        <Field label={t("stations.position")}>
          <input
            className="input"
            type="number"
            min={0}
            value={draft.position}
            onChange={(e) => set({ position: Number(e.target.value) || 0 })}
          />
        </Field>
        {!first && (
          <label className="check">
            <input type="checkbox" checked={draft.enabled} onChange={(e) => set({ enabled: e.target.checked })} />{" "}
            {t("stations.enabled")}
            <span className="hint"> — {t("stations.enabledHint")}</span>
          </label>
        )}
      </form>
    </Modal>
  );
}

/** A station's engine variable, and whether heron-api has it. */
function EngineVariable({ station }: { station: StationInfo }) {
  return (
    <span>
      <code className="mono">{station.engine_variable}</code>{" "}
      {station.engine_taken_by ? (
        <span className="badge warn" title={t("stations.engineTakenHint")}>
          {t("stations.engineTaken", { code: station.engine_taken_by })}
        </span>
      ) : station.engine_configured ? (
        <span className="badge good">{t("stations.engineConfigured")}</span>
      ) : (
        <span className="badge warn">{t("stations.engineMissing")}</span>
      )}
    </span>
  );
}
