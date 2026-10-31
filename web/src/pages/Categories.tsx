// SPDX-License-Identifier: AGPL-3.0-or-later

// Categories: groups of tracks the clocks call, with their type (song,
// jingle, ad…), the separation windows they override, and those the FX
// player's picker opens on. The station's category types are managed here.

import { useQuery } from "@tanstack/react-query";
import { type FormEvent, useState } from "react";
import { StationBadges, StationChecks, useSeveralStations } from "../components/Stations";
import { here, useStation } from "../lib/station";
import { type Category, type SeparationOverrides, api, done, unwrap } from "../api";
import { useLabels } from "../components/SelectionActions";
import { SeparationEditor } from "../components/Separation";
import { Card, ConfirmButton, Empty, ErrorView, Field, Icon, Loading, Modal, PageHead } from "../components/ui";
import { number, rich, t } from "../i18n";
import { COLORS } from "../lib/colors";
import { totalTime } from "../lib/format";
import { describeOverrides } from "../lib/rotation";
import { Link } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { TypesCard, useCategoryTypes } from "./CategoryTypes";

type SortKey = "name" | "type" | "tracks" | "total";

/** Categories sorted by a column; ties by name. */
export function sortCategories(categories: Category[], by: SortKey, descending: boolean): Category[] {
  const byName = (a: Category, b: Category) => a.name.localeCompare(b.name);
  const compare: Record<SortKey, (a: Category, b: Category) => number> = {
    name: byName,
    type: (a, b) => (a.type_name ?? "").localeCompare(b.type_name ?? ""),
    tracks: (a, b) => a.tracks - b.tracks,
    total: (a, b) => a.total_ms - b.total_ms,
  };
  return [...categories].sort((a, b) => {
    const order = compare[by](a, b);
    return (descending ? -order : order) || byName(a, b);
  });
}


export function Categories() {
  const canEdit = useCan("operator");
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const [editing, setEditing] = useState<Category | "new" | null>(null);
  const [filling, setFilling] = useState<Category | null>(null);
  const [type, setType] = useState<number | "all" | "none">("all");
  const [sort, setSort] = useState<{ by: SortKey; descending: boolean }>({ by: "name", descending: false });
  const types = useCategoryTypes();
  const shown = sortCategories(
    (categories.data ?? []).filter((category) =>
      type === "all" ? true : type === "none" ? category.type_id == null : category.type_id === type,
    ),
    sort.by,
    sort.descending,
  );
  const header = (by: SortKey, label: string, className?: string, title?: string) => (
    <th
      className={className}
      title={title}
      aria-sort={sort.by === by ? (sort.descending ? "descending" : "ascending") : undefined}
    >
      <button
        type="button"
        className="sort"
        onClick={() =>
          setSort((current) => ({
            by,
            // Counts and times start with the largest.
            descending: current.by === by ? !current.descending : by === "tracks" || by === "total",
          }))
        }
      >
        {label}
        {sort.by === by && <span aria-hidden="true">{sort.descending ? " ▾" : " ▴"}</span>}
      </button>
    </th>
  );
  const remove = useAction(
    (id: number) => done(api.DELETE("/api/v1/categories/{id}", { params: { path: { id } } })),
    { success: t("categories.deleted"), invalidate: [["categories"]] },
  );

  return (
    <>
      <PageHead
        title={t("nav.categories")}
        description={t("categories.description")}
        actions={
          canEdit && (
            <button type="button" className="btn primary" onClick={() => setEditing("new")}>
              <Icon name="plus" /> {t("categories.new")}
            </button>
          )
        }
      />
      <Card flush>
        {(types.data?.length ?? 0) > 0 && (categories.data?.length ?? 0) > 0 && (
          <div className="card-head">
            <label className="row small">
              {t("categories.type")}
              <select
                className="input"
                value={String(type)}
                onChange={(event) =>
                  setType(
                    event.target.value === "all" || event.target.value === "none"
                      ? event.target.value
                      : Number(event.target.value),
                  )
                }
              >
                <option value="all">{t("categories.allTypes")}</option>
                {types.data?.map((categoryType) => (
                  <option key={categoryType.id} value={categoryType.id}>
                    {categoryType.name}
                  </option>
                ))}
                <option value="none">{t("categories.noType")}</option>
              </select>
            </label>
          </div>
        )}
        {categories.isPending && <Loading />}
        {categories.isError && <ErrorView error={categories.error} />}
        {categories.data?.length === 0 && <Empty>{t("categories.none")}</Empty>}
        {categories.data && categories.data.length > 0 && (
          <table className="table">
            <thead>
              <tr>
                {header("name", t("categories.name"))}
                {header("type", t("categories.type"))}
                {header("tracks", t("nav.tracks"), "num")}
                {header("total", t("categories.totalTime"), "num", t("categories.totalTimeHint"))}
                <th>{t("categories.separation")}</th>
                <th>
                  <span className="visually-hidden">{t("library.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {shown.map((category) => (
                <tr key={category.id}>
                  <td>
                    <span className="row">
                      <span className="swatch" style={{ background: category.color ?? undefined }} />
                      {category.name}
                      <StationBadges ids={category.station_ids} />
                      {category.fx_player && (
                        <span className="chip" title={t("categories.fxPlayerHint")}>
                          {t("categories.fxPlayerChip")}
                        </span>
                      )}
                    </span>
                  </td>
                  <td>{category.type_name ?? <span className="muted">–</span>}</td>
                  <td className="num">
                    <Link to={`/library?category=${category.id}`}>{number(category.tracks)}</Link>
                  </td>
                  <td className="num">{totalTime(category.total_ms)}</td>
                  <td className="small muted">{describeOverrides(category.separation) || t("categories.stationRules")}</td>
                  <td className="actions">
                    {canEdit && (
                      <>
                        <button
                          type="button"
                          className="btn ghost small icon"
                          aria-label={t("categories.edit", { name: category.name })}
                          onClick={() => setEditing(category)}
                        >
                          <Icon name="edit" />
                        </button>
                        <button
                          type="button"
                          className="btn ghost small"
                          title={t("categories.fillHelp")}
                          onClick={() => setFilling(category)}
                        >
                          {t("categories.fill")}
                        </button>
                        <ConfirmButton
                          className="btn ghost small icon danger"
                          title={t("categories.delete", { name: category.name })}
                          confirm={t("categories.deleteButton")}
                          question={
                            <p>
                              {rich("categories.deleteQuestion", {
                                name: <strong>{category.name}</strong>,
                                count: category.tracks,
                              })}
                            </p>
                          }
                          onConfirm={() => remove.mutate(category.id)}
                        >
                          <Icon name="trash" />
                        </ConfirmButton>
                      </>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
      <TypesCard />
      {editing && <CategoryDialog category={editing === "new" ? null : editing} onClose={() => setEditing(null)} />}
      {filling && <FillDialog category={filling} onClose={() => setFilling(null)} />}
    </>
  );
}

function CategoryDialog({ category, onClose }: { category: Category | null; onClose: () => void }) {
  const [name, setName] = useState(category?.name ?? "");
  const [color, setColor] = useState(category?.color ?? COLORS[0] ?? "#2f6fd6");
  const [fxPlayer, setFxPlayer] = useState(category?.fx_player ?? false);
  const types = useCategoryTypes();
  // New categories are songs, the first type, unless chosen otherwise.
  const [typeId, setTypeId] = useState<number | null | undefined>(category ? (category.type_id ?? null) : undefined);
  const chosenType = typeId === undefined ? (types.data?.[0]?.id ?? null) : typeId;
  const [separation, setSeparation] = useState<SeparationOverrides>(category?.separation ?? {});
  const station = useStation();
  const several = useSeveralStations();
  const [stationIds, setStationIds] = useState<number[]>(
    category?.station_ids ?? (station === null ? [] : [station]),
  );
  const rotation = useQuery({
    queryKey: ["settings", "rotation"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/rotation", { params: { path: here() } })),
  });
  const save = useAction(
    () => {
      const body = {
        name,
        color,
        type_id: chosenType,
        fx_player: fxPlayer,
        separation,
        ...(several ? { station_ids: stationIds } : {}),
      };
      return category
        ? unwrap(api.PUT("/api/v1/categories/{id}", { params: { path: { id: category.id } }, body }))
        : unwrap(api.POST("/api/v1/categories", { body }));
    },
    { success: t("categories.saved"), invalidate: [["categories"]], onSuccess: onClose },
  );
  const submit = (event: FormEvent) => {
    event.preventDefault();
    save.mutate(undefined);
  };
  return (
    <Modal
      title={category ? t("categories.edit", { name: category.name }) : t("categories.new")}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="submit" form="category-form" className="btn primary" disabled={save.isPending}>
            {t("common.save")}
          </button>
        </>
      }
    >
      <form id="category-form" className="stack" onSubmit={submit}>
        <Field label={t("categories.name")}>
          <input className="input" value={name} onChange={(e) => setName(e.target.value)} required maxLength={255} />
        </Field>
        <Field label={t("categories.type")} hint={t("categories.typeHint")}>
          <select
            className="input"
            value={chosenType === null ? "" : String(chosenType)}
            onChange={(event) => setTypeId(event.target.value ? Number(event.target.value) : null)}
          >
            {types.data?.map((categoryType) => (
              <option key={categoryType.id} value={categoryType.id}>
                {categoryType.name}
              </option>
            ))}
            <option value="">{t("categories.noType")}</option>
          </select>
        </Field>
        <Field label={t("categories.color")}>
          <div className="row">
            {COLORS.map((swatch) => (
              <button
                key={swatch}
                type="button"
                className="btn icon small"
                aria-label={t("categories.colorValue", { color: swatch })}
                aria-pressed={color === swatch}
                style={{ background: swatch, borderColor: color === swatch ? "var(--fg)" : swatch }}
                onClick={() => setColor(swatch)}
              />
            ))}
            <input type="color" value={color} onChange={(e) => setColor(e.target.value)} aria-label={t("categories.otherColor")} />
          </div>
        </Field>
        {several && (
          <Field label={t("categories.stations")} hint={t("categories.stationsHint")}>
            <StationChecks value={stationIds} onChange={setStationIds} />
          </Field>
        )}
        <Field label={t("categories.fxPlayer")} hint={t("categories.fxPlayerHint")}>
          <label className="check">
            <input type="checkbox" checked={fxPlayer} onChange={(e) => setFxPlayer(e.target.checked)} />
            {t("categories.fxPlayerCheck")}
          </label>
        </Field>
        <fieldset>
          <legend>{t("categories.separation")}</legend>
          <SeparationEditor value={separation} inherited={rotation.data?.separation} onChange={setSeparation} />
        </fieldset>
      </form>
    </Modal>
  );
}

/** Puts every track with a label in the category. */
function FillDialog({ category, onClose }: { category: Category; onClose: () => void }) {
  const labels = useLabels();
  const [label, setLabel] = useState("");
  const fill = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/tracks/categories", {
          body: {
            selection: { filters: { labels_any: label } },
            add: [{ category_id: category.id, weight: 1 }],
          },
        }),
      ),
    {
      success: (result) => t("categories.filled", { count: result.tracks, name: category.name }),
      invalidate: [["categories"], ["tracks"], ["track"]],
      onSuccess: onClose,
    },
  );
  return (
    <Modal
      title={t("categories.fillTitle", { name: category.name })}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={!label || fill.isPending}
            onClick={() => fill.mutate(undefined)}
          >
            {t("categories.fillButton")}
          </button>
        </>
      }
    >
      <div className="stack">
        {labels.data?.length === 0 ? (
          <p>{rich("categories.fillNoLabel", { link: <Link to="/labels">{t("categories.createLabel")}</Link> })}</p>
        ) : (
          <Field label={t("categories.fillLabel")}>
            <select className="input" value={label} onChange={(event) => setLabel(event.target.value)}>
              <option value="">{t("selection.choose")}</option>
              {labels.data?.map((l) => (
                <option key={l.id} value={l.id}>
                  {t("categories.labelOption", { name: l.name, tracks: number(l.tracks) })}
                </option>
              ))}
            </select>
          </Field>
        )}
        <p className="muted small" style={{ margin: 0 }}>
          {rich("categories.fillNote", { link: <Link to="/library">{t("categories.libraryLink")}</Link> })}
        </p>
      </div>
    </Modal>
  );
}
