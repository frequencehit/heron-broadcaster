// SPDX-License-Identifier: AGPL-3.0-or-later

// The library: search, filters, sorting, pages, the columns shown, and
// actions on a selection (queue, labels, categories, programming, tags).

import { useStations } from "../components/Stations";
import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useRef, useState } from "react";
import {
  type Schemas,
  type TrackFilter,
  type TrackSort,
  type TrackSummary,
  api,
  fetchLibraryStatus,
  query,
  unwrap,
} from "../api";
import {
  CategoryDialog,
  LabelChip,
  LabelsDialog,
  ProgrammingDialog,
  type Selection,
  TagsDialog,
  useLabels,
  useProgrammingSettings,
} from "../components/SelectionActions";
import { useDebounced } from "../components/TrackSearch";
import { TaskState, useTask } from "../components/Task";
import { Menu } from "../components/Menu";
import { Card, ConfirmDialog, Empty, ErrorView, Icon, Loading, PageHead, useToast } from "../components/ui";
import { type PlainKey, number, rich, t } from "../i18n";
import { ago, duration, trackName } from "../lib/format";
import { Link, setSearch, useLocation } from "../lib/router";
import { type RangeSelection, useRangeSelection } from "../lib/rangeSelection";
import { useAction, useCan } from "../lib/session";
import { type Preview, usePreview } from "../lib/preview";
import { here } from "../lib/station";

const PAGE = 50;
/** Files in one tag edit, at most (the API's limit). */
const MAX_TAG_EDIT = 10_000;

const FILTERS: { value: TrackFilter | ""; label: PlainKey }[] = [
  { value: "", label: "library.filter.all" },
  { value: "playable", label: "library.filter.playable" },
  { value: "unanalyzed", label: "library.filter.unanalyzed" },
  { value: "silent", label: "library.filter.silent" },
  { value: "missing", label: "library.filter.missing" },
  { value: "disabled", label: "library.filter.disabled" },
  { value: "needs_review", label: "library.filter.needsReview" },
];

const SORTS: { value: TrackSort; label: PlainKey }[] = [
  { value: "artist", label: "library.sort.artist" },
  { value: "title", label: "library.sort.title" },
  { value: "album", label: "library.sort.album" },
  { value: "genre", label: "library.sort.genre" },
  { value: "added", label: "library.sort.added" },
  { value: "last_played", label: "library.sort.lastPlayed" },
  { value: "play_count", label: "library.sort.playCount" },
  { value: "bitrate", label: "library.sort.bitrate" },
  { value: "duration", label: "library.sort.duration" },
  { value: "year", label: "library.sort.year" },
  { value: "bpm", label: "library.sort.bpm" },
  { value: "mood", label: "library.sort.mood" },
  { value: "sound_code", label: "library.sort.soundCode" },
];

type Column =
  | "album"
  | "genre"
  | "length"
  | "added"
  | "last_played"
  | "plays"
  | "bitrate"
  | "year"
  | "era"
  | "bpm"
  | "mood"
  | "sound_code"
  | "labels";

const COLUMNS: { value: Column; label: PlainKey }[] = [
  { value: "album", label: "library.sort.album" },
  { value: "genre", label: "library.column.genre" },
  { value: "length", label: "library.length" },
  { value: "added", label: "library.column.added" },
  { value: "last_played", label: "library.lastPlayed" },
  { value: "plays", label: "library.column.plays" },
  { value: "bitrate", label: "library.column.bitrate" },
  { value: "year", label: "library.column.year" },
  { value: "era", label: "library.column.era" },
  { value: "bpm", label: "library.column.bpm" },
  { value: "mood", label: "library.column.mood" },
  { value: "sound_code", label: "library.column.soundCode" },
  { value: "labels", label: "library.column.labels" },
];

const DEFAULT_COLUMNS: Column[] = ["album", "length", "last_played", "plays", "labels"];
const COLUMNS_KEY = "heron.library.columns";

function storedColumns(): Column[] {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(COLUMNS_KEY) ?? "null");
    if (Array.isArray(stored)) {
      return COLUMNS.map((c) => c.value).filter((c) => stored.includes(c));
    }
  } catch {
    // Unreadable or blocked storage: the defaults.
  }
  return DEFAULT_COLUMNS;
}

function useColumns(): [Column[], (column: Column, shown: boolean) => void] {
  const [columns, setColumns] = useState(storedColumns);
  const toggle = (column: Column, shown: boolean) =>
    setColumns((current) => {
      const next = COLUMNS.map((c) => c.value).filter((c) => (c === column ? shown : current.includes(c)));
      try {
        localStorage.setItem(COLUMNS_KEY, JSON.stringify(next));
      } catch {
        // Not remembered.
      }
      return next;
    });
  return [columns, toggle];
}

/** The filters of the URL, as the API takes them. */
function filtersOf(search: URLSearchParams, q: string) {
  const whole = (name: string) => {
    const value = Number(search.get(name));
    return search.get(name) && Number.isFinite(value) && value >= 0 ? value : undefined;
  };
  const text = (name: string) => search.get(name) || undefined;
  return query({
    q,
    filter: text("filter") as TrackFilter | undefined,
    category_id: whole("category"),
    labels_all: text("labels_all"),
    labels_any: text("labels_any"),
    labels_none: text("labels_none"),
    station_id: whole("station"),
    never_played: search.get("never_played") === "true" ? true : undefined,
    not_played_days: whole("not_played_days"),
    added_days: whole("added_days"),
    batch_id: whole("batch"),
    bitrate_below: whole("bitrate_below"),
    era: text("era"),
    mood: text("mood"),
    sound_code: text("sound_code"),
    genre: text("genre"),
  });
}

/** Filters of the URL beyond search, show and category. */
const MORE_FILTERS = [
  "station",
  "labels_all",
  "labels_any",
  "labels_none",
  "never_played",
  "not_played_days",
  "added_days",
  "bitrate_below",
  "era",
  "mood",
  "sound_code",
  "genre",
];

type Dialog = "labels" | "category" | "programming" | "tags" | null;

export function Library() {
  const { search } = useLocation();
  const canEdit = useCan("operator");
  const preview = usePreview();
  const filter = (search.get("filter") ?? "") as TrackFilter | "";
  const sort = (search.get("sort") ?? "artist") as TrackSort;
  const desc = search.get("desc") === "true";
  const category = search.get("category") ?? "";
  const page = Math.max(0, Number(search.get("page") ?? 0) || 0);
  const batch = search.get("batch");
  const [text, setText] = useState(search.get("q") ?? "");
  const q = useDebounced(text.trim(), 300);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  // Selected tracks seen on any page, to list what a deletion takes.
  const known = useRef(new Map<number, TrackSummary>());
  const client = useQueryClient();
  const toast = useToast();
  const [deleteTask, setDeleteTask] = useState<number | null>(null);
  const deleting = useTask(deleteTask);
  // Every track matching the filters, on every page.
  const [allMatching, setAllMatching] = useState(false);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [columns, toggleColumn] = useColumns();
  const [moreOpen, setMoreOpen] = useState(() => MORE_FILTERS.some((name) => search.get(name)));

  useEffect(() => {
    if (q !== (search.get("q") ?? "")) {
      setSearch({ q, page: undefined });
    }
  }, [q, search]);

  const filters = filtersOf(search, q);
  const filterKey = JSON.stringify(filters);
  // Other filters start another selection.
  useEffect(() => {
    setSelected(new Set());
    setAllMatching(false);
  }, [filterKey]);

  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const labels = useLabels();
  const programming = useProgrammingSettings();
  const status = useQuery({
    queryKey: ["library-status"],
    queryFn: ({ signal }) => fetchLibraryStatus(signal),
    refetchInterval: 10_000,
  });
  const tracks = useQuery({
    queryKey: ["tracks", { ...filters, sort, desc, page }],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/api/v1/tracks", {
          params: {
            query: { ...filters, ...query({ sort, desc: desc || undefined }), limit: PAGE, offset: page * PAGE },
          },
          signal,
        }),
      ),
    placeholderData: keepPreviousData,
  });

  const enqueue = useAction(
    ({ ids, next }: { ids: number[]; next: boolean }) =>
      unwrap(
        api.POST("/api/v1/stations/{station_id}/queue", { params: { path: here() }, body: next ? { track_ids: ids, position: 0 } : { track_ids: ids } }),
      ),
    {
      success: (result) =>
        result.entry_ids.length === 1 ? t("queuePage.queued") : t("library.queued", { count: result.entry_ids.length }),
      onSuccess: () => setSelected(new Set()),
    },
  );

  const setEnabled = useAction(
    (enabled: boolean) => unwrap(api.POST("/api/v1/tracks/enabled", { body: { selection, enabled } })),
    {
      success: (result) => t("library.enabledChanged", { count: result.changed }),
      invalidate: [["tracks"], ["track"]],
    },
  );
  const backToFileTags = useAction(
    () => unwrap(api.POST("/api/v1/tracks/tag-overrides/clear", { body: { selection } })),
    {
      success: (result) => t("library.overridesCleared", { count: result.changed }),
      invalidate: [["tracks"], ["track"]],
    },
  );
  const [confirmDelete, setConfirmDelete] = useState(false);
  const remove = useAction(
    (ids: number[]) => unwrap(api.POST("/api/v1/tracks/delete", { body: { track_ids: ids } })),
    {
      onSuccess: (task) => {
        setDeleteTask(task.id);
        setSelected(new Set());
      },
    },
  );
  useEffect(() => {
    const task = deleting.data;
    if (task?.status === "done") {
      const report = task.report;
      toast(
        report?.skipped.length
          ? t("library.deletedSkipped", { count: report.done, skipped: report.skipped.length })
          : t("library.deleted", { count: report?.done ?? 0 }),
      );
      void client.invalidateQueries({ queryKey: ["tracks"] });
      void client.invalidateQueries({ queryKey: ["library-status"] });
      void client.invalidateQueries({ queryKey: ["trash"] });
    } else if (task?.status === "failed") {
      toast(task.error ?? t("library.deleteFailed"), true);
    }
  }, [deleting.data, client, toast]);

  const items = tracks.data?.items ?? [];
  for (const track of items) {
    known.current.set(track.id, track);
  }
  const selectedTracks = [...selected].flatMap((id) => {
    const track = known.current.get(id);
    return track ? [track] : [];
  });
  const playableIds = selectedTracks.filter((track) => !track.missing && !track.silent).map((track) => track.id);
  const deletingNow =
    deleteTask !== null && deleting.data?.status !== "done" && deleting.data?.status !== "failed";
  const total = tracks.data?.total ?? 0;
  const pages = Math.max(1, Math.ceil(total / PAGE));
  const range = useRangeSelection(
    items.map((track) => track.id),
    selected,
    (next) => {
      setAllMatching(false);
      setSelected(next);
    },
    `${filterKey} ${page}`,
  );
  const pageSelected = items.length > 0 && items.every((track) => selected.has(track.id));
  const selection: Selection = allMatching ? { filters } : { track_ids: [...selected] };
  const selectedCount = allMatching ? total : selected.size;
  const clear = () => {
    setSelected(new Set());
    setAllMatching(false);
  };
  const shown = COLUMNS.filter((c) => columns.includes(c.value));

  return (
    <>
      <PageHead title={t("nav.tracks")} description={<LibrarySummary status={status.data} />} />
      <Card flush>
        {batch && (
          <div className="card-head">
            <span className="chip">
              {t("library.importBatch", { batch })}
              <button
                type="button"
                className="btn ghost small"
                aria-label={t("library.clearImportBatch")}
                onClick={() => setSearch({ batch: undefined, page: undefined })}
              >
                <Icon name="close" size={12} />
              </button>
            </span>
          </div>
        )}
        <div className="card-head">
          <div className="row" style={{ flex: 1 }}>
            <label className="row" style={{ flex: "1 1 16rem", flexWrap: "nowrap" }}>
              <Icon name="search" />
              <span className="visually-hidden">{t("library.search")}</span>
              <input
                className="input"
                type="search"
                placeholder={t("library.searchPlaceholder")}
                value={text}
                onChange={(event) => setText(event.target.value)}
              />
            </label>
            <label>
              <span className="visually-hidden">{t("library.show")}</span>
              <select
                className="input"
                value={filter}
                onChange={(event) => setSearch({ filter: event.target.value, page: undefined })}
              >
                {FILTERS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {t(option.label)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span className="visually-hidden">{t("library.category")}</span>
              <select
                className="input"
                value={category}
                onChange={(event) => setSearch({ category: event.target.value, page: undefined })}
              >
                <option value="">{t("library.anyCategory")}</option>
                {categories.data?.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </select>
            </label>
            <span className="row" style={{ flexWrap: "nowrap" }}>
              <label>
                <span className="visually-hidden">{t("library.sortBy")}</span>
                <select
                  className="input"
                  value={sort}
                  onChange={(event) => setSearch({ sort: event.target.value, page: undefined })}
                >
                  {SORTS.map((option) => (
                    <option key={option.value} value={option.value}>
                      {t("library.sortOption", { sort: t(option.label) })}
                    </option>
                  ))}
                </select>
              </label>
              <button
                type="button"
                className="btn ghost small"
                aria-pressed={desc}
                title={t("library.reverseHelp")}
                onClick={() => setSearch({ desc: desc ? undefined : "true", page: undefined })}
              >
                {desc ? t("library.reversed") : t("library.reverse")}
              </button>
            </span>
            <button
              type="button"
              className="btn ghost small"
              aria-expanded={moreOpen}
              onClick={() => setMoreOpen((open) => !open)}
            >
              {t("library.moreFilters")}
            </button>
            <ColumnsMenu columns={columns} onToggle={toggleColumn} />
          </div>
        </div>
        {moreOpen && <MoreFilters search={search} labels={labels.data ?? []} programming={programming.data} />}
        {deletingNow && (
          <div className="card-head">
            <span>{t("library.deleting")}</span>
            <TaskState task={deleting.data} />
          </div>
        )}
        {selectedCount > 0 && (
          <div className="card-head" style={{ background: "var(--accent-soft)" }}>
            <span>
              {allMatching
                ? t("library.allMatching", { count: total })
                : t("library.selected", { count: selected.size })}
              {!allMatching && pageSelected && total > items.length && (
                <>
                  {" · "}
                  <button type="button" className="link-button" onClick={() => setAllMatching(true)}>
                    {t("library.selectAllMatching", { count: total })}
                  </button>
                </>
              )}
            </span>
            <div className="row">
              {canEdit && !allMatching && (
                <>
                  <button
                    type="button"
                    className="btn small"
                    disabled={playableIds.length === 0}
                    onClick={() => enqueue.mutate({ ids: playableIds, next: true })}
                  >
                    <Icon name="next" size={14} /> {t("queue.playNext")}
                  </button>
                  <button
                    type="button"
                    className="btn small"
                    disabled={playableIds.length === 0}
                    onClick={() => enqueue.mutate({ ids: playableIds, next: false })}
                  >
                    <Icon name="plus" size={14} /> {t("library.addToQueue")}
                  </button>
                </>
              )}
              {canEdit && (
                <button
                  type="button"
                  className="btn small"
                  disabled={selectedCount > MAX_TAG_EDIT}
                  title={selectedCount > MAX_TAG_EDIT ? t("library.tagsLimit", { max: MAX_TAG_EDIT }) : undefined}
                  onClick={() => setDialog("tags")}
                >
                  <Icon name="edit" size={14} /> {t("library.tagsAction")}
                </button>
              )}
              <button type="button" className="btn small ghost" onClick={clear}>
                {t("queuePage.clear")}
              </button>
              {canEdit && (
                <Menu
                  label={t("library.moreActions")}
                  items={[
                    { label: t("library.labelsAction"), onSelect: () => setDialog("labels") },
                    { label: t("library.categoryAction"), onSelect: () => setDialog("category") },
                    { label: t("library.programmingAction"), onSelect: () => setDialog("programming") },
                    { label: t("library.enable"), onSelect: () => setEnabled.mutate(true), disabled: setEnabled.isPending },
                    {
                      label: t("library.disable"),
                      title: t("library.disableHelp"),
                      onSelect: () => setEnabled.mutate(false),
                      disabled: setEnabled.isPending,
                    },
                    {
                      label: t("library.backToFileTags"),
                      title:
                        selectedCount > MAX_TAG_EDIT
                          ? t("library.tagsLimit", { max: MAX_TAG_EDIT })
                          : t("library.backToFileTagsHelp"),
                      onSelect: () => backToFileTags.mutate(undefined),
                      disabled: selectedCount > MAX_TAG_EDIT || backToFileTags.isPending,
                    },
                    {
                      label: t("library.delete"),
                      danger: true,
                      title: allMatching ? t("library.deletePageOnly") : undefined,
                      onSelect: () => setConfirmDelete(true),
                      disabled: allMatching || deletingNow,
                    },
                  ]}
                />
              )}
            </div>
          </div>
        )}
        {tracks.isPending && <Loading />}
        {tracks.isError && (
          <div className="card-body">
            <ErrorView error={tracks.error} />
          </div>
        )}
        {tracks.data && items.length === 0 && <Empty>{t("library.noMatch")}</Empty>}
        {items.length > 0 && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th style={{ width: "2rem" }}>
                    <input
                      type="checkbox"
                      aria-label={t("library.selectPage")}
                      checked={pageSelected || allMatching}
                      onChange={() => {
                        setAllMatching(false);
                        setSelected(pageSelected ? new Set() : new Set(items.map((track) => track.id)));
                      }}
                    />
                  </th>
                  <th>{t("library.sort.title")}</th>
                  <th className="hide-narrow">{t("library.sort.artist")}</th>
                  {shown.map((c) => (
                    <th key={c.value} className={c.value === "length" ? undefined : "hide-narrow"}>
                      {t(c.label)}
                    </th>
                  ))}
                  <th>
                    <span className="visually-hidden">{t("library.actions")}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {items.map((track) => (
                  <Row
                    key={track.id}
                    track={track}
                    columns={shown.map((c) => c.value)}
                    canQueue={canEdit}
                    selected={allMatching || selected.has(track.id)}
                    range={range}
                    onQueue={(next) => enqueue.mutate({ ids: [track.id], next })}
                    onArtist={setText}
                    preview={preview}
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
        {total > PAGE && (
          <div className="pager">
            <button
              type="button"
              className="btn small"
              disabled={page === 0}
              onClick={() => setSearch({ page: page - 1 || undefined })}
            >
              {t("library.previous")}
            </button>
            <span className="muted small num">
              {t("library.page", { page: page + 1, pages })} · {t("common.tracks", { count: total })}
            </span>
            <button
              type="button"
              className="btn small"
              disabled={page + 1 >= pages}
              onClick={() => setSearch({ page: page + 1 })}
            >
              {t("dashboard.next")}
            </button>
          </div>
        )}
      </Card>
      <ConfirmDialog
        open={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        question={<DeleteQuestion tracks={selectedTracks} />}
        confirm={t("library.deleteConfirm", { count: selected.size })}
        onConfirm={() => remove.mutate([...selected])}
      />
      <LabelsDialog
        open={dialog === "labels"}
        onClose={() => setDialog(null)}
        selection={selection}
        tracks={selectedCount}
      />
      <CategoryDialog
        open={dialog === "category"}
        onClose={() => setDialog(null)}
        selection={selection}
        tracks={selectedCount}
      />
      <ProgrammingDialog
        open={dialog === "programming"}
        onClose={() => setDialog(null)}
        selection={selection}
        tracks={selectedCount}
      />
      <TagsDialog
        open={dialog === "tags"}
        onClose={() => setDialog(null)}
        selection={selection}
        tracks={selectedCount}
      />
    </>
  );
}

function ColumnsMenu({ columns, onToggle }: { columns: Column[]; onToggle: (column: Column, shown: boolean) => void }) {
  return (
    <details className="menu">
      <summary className="btn ghost small">{t("library.columns")}</summary>
      <div className="menu-panel stack">
        {COLUMNS.map((column) => (
          <label key={column.value} className="check">
            <input
              type="checkbox"
              checked={columns.includes(column.value)}
              onChange={(event) => onToggle(column.value, event.target.checked)}
            />
            {t(column.label)}
          </label>
        ))}
      </div>
    </details>
  );
}

/** Decades offered when the station has no eras of its own. */
const DECADES = ["50s", "60s", "70s", "80s", "90s", "2000s", "2010s", "2020s"];

function MoreFilters({
  search,
  labels,
  programming,
}: {
  search: URLSearchParams;
  labels: Schemas["Label"][];
  programming: Schemas["ProgrammingSettings"] | undefined;
}) {
  const value = (name: string) => search.get(name) ?? "";
  const set = (name: string, next: string | undefined) => setSearch({ [name]: next || undefined, page: undefined });
  const labelSelect = (name: string, title: string) => (
    <label className="field">
      <span>{title}</span>
      <select
        className="input"
        multiple
        size={Math.min(4, Math.max(2, labels.length))}
        value={value(name).split(",").filter(Boolean)}
        onChange={(event) => set(name, [...event.target.selectedOptions].map((option) => option.value).join(","))}
      >
        {labels.map((label) => (
          <option key={label.id} value={label.id}>
            {label.name}
          </option>
        ))}
      </select>
    </label>
  );
  const numberField = (name: string, title: string, unit: string) => (
    <label className="field">
      <span>{title}</span>
      <span className="row" style={{ flexWrap: "nowrap" }}>
        <input
          className="input num"
          type="number"
          min={0}
          value={value(name)}
          onChange={(event) => set(name, event.target.value)}
        />
        <span className="muted small">{unit}</span>
      </span>
    </label>
  );
  const stations = useStations();
  const choice = (name: string, title: string, values: string[]) => (
    <label className="field">
      <span>{title}</span>
      <select className="input" value={value(name)} onChange={(event) => set(name, event.target.value)}>
        <option value="">{t("library.any")}</option>
        {values.map((v) => (
          <option key={v} value={v}>
            {v}
          </option>
        ))}
      </select>
    </label>
  );
  const eras = programming?.eras.length ? programming.eras.map((era) => era.name) : DECADES;
  return (
    <div className="card-body stack">
      <div className="filters-grid">
        {stations.length > 1 && (
          <label className="field">
            <span>{t("library.playsOn")}</span>
            <select className="input" value={value("station")} onChange={(event) => set("station", event.target.value)}>
              <option value="">{t("stations.all")}</option>
              {stations.map((station) => (
                <option key={station.id} value={station.id}>
                  {station.name}
                </option>
              ))}
            </select>
          </label>
        )}
        {labels.length > 0 && (
          <>
            {labelSelect("labels_any", t("library.labelsAny"))}
            {labelSelect("labels_all", t("library.labelsAll"))}
            {labelSelect("labels_none", t("library.labelsNone"))}
          </>
        )}
        {numberField("not_played_days", t("library.notPlayedFor"), t("library.days"))}
        {numberField("added_days", t("library.addedInLast"), t("library.days"))}
        {numberField("bitrate_below", t("library.bitrateBelow"), "kbps")}
        {choice("era", t("library.column.era"), eras)}
        {choice("mood", t("library.column.mood"), programming?.moods ?? [])}
        {choice("sound_code", t("library.column.soundCode"), programming?.sound_codes ?? [])}
        <label className="check">
          <input
            type="checkbox"
            checked={value("never_played") === "true"}
            onChange={(event) => set("never_played", event.target.checked ? "true" : undefined)}
          />
          {t("library.neverPlayed")}
        </label>
      </div>
      {MORE_FILTERS.some((name) => search.get(name)) && (
        <div>
          <button
            type="button"
            className="btn small ghost"
            onClick={() =>
              setSearch({ ...Object.fromEntries(MORE_FILTERS.map((name) => [name, undefined])), page: undefined })
            }
          >
            {t("library.clearFilters")}
          </button>
        </div>
      )}
    </div>
  );
}

function LibrarySummary({ status }: { status: Awaited<ReturnType<typeof fetchLibraryStatus>> | undefined }) {
  if (!status) {
    return null;
  }
  const queue = status.analysis_queue;
  const analyzing = queue.pending + queue.running;
  const parts: ReactNode[] = [
    t("common.tracks", { count: status.tracks }),
    t("library.analyzed", { count: status.analyzed }),
  ];
  if (analyzing > 0) {
    parts.push(t("library.analyzing", { count: analyzing }));
  }
  if (status.missing > 0) {
    parts.push(<Link to="/library?filter=missing">{t("library.missing", { count: status.missing })}</Link>);
  }
  if (status.silent > 0) {
    parts.push(<Link to="/library?filter=silent">{t("library.silent", { count: status.silent })}</Link>);
  }
  if (queue.failed > 0) {
    parts.push(t("library.unreadable", { count: queue.failed }));
  }
  return (
    <>
      {parts.map((part, index) => (
        <span key={index}>
          {index > 0 && t("library.separator")}
          {part}
        </span>
      ))}
      {status.last_scan && t("library.lastScan", { ago: ago(status.last_scan.finished_at_unix * 1000) })}
      {t("library.end")}
    </>
  );
}

function TrackState({ track }: { track: TrackSummary }) {
  if (track.missing) {
    return <span className="badge bad">{t("library.state.missing")}</span>;
  }
  if (!track.enabled) {
    return <span className="badge">{t("library.state.disabled")}</span>;
  }
  if (!track.analyzed) {
    return <span className="badge warn">{t("library.state.unanalyzed")}</span>;
  }
  if (track.silent) {
    return <span className="badge bad">{t("library.state.silent")}</span>;
  }
  return null;
}

/** `128 kbps VBR`, `1411 kbps FLAC`. */
export function bitrate(track: Pick<TrackSummary, "bitrate_kbps" | "bitrate_mode" | "codec">): string {
  if (!track.bitrate_kbps) {
    return track.codec ?? "–";
  }
  const mode = track.bitrate_mode === "lossless" ? (track.codec ?? "") : (track.bitrate_mode ?? "").toUpperCase();
  const kbps = t("library.kbps", { kbps: track.bitrate_kbps });
  return mode ? `${kbps} ${mode}` : kbps;
}

function Row({
  track,
  columns,
  canQueue,
  selected,
  range,
  onQueue,
  onArtist,
  preview,
}: {
  track: TrackSummary;
  columns: Column[];
  canQueue: boolean;
  selected: boolean;
  range: RangeSelection<number>;
  onQueue: (next: boolean) => void;
  onArtist: (artist: string) => void;
  preview: Preview;
}) {
  const previewing = preview.playing === track.id;
  const unplayable = preview.failed.has(track.id);
  const playable = !track.missing && !track.silent;
  const file = track.path.split("/").pop() ?? track.path;
  const none = <span className="muted">–</span>;
  const cells: Record<Column, ReactNode> = {
    album: track.album ?? none,
    genre: track.genre ?? none,
    length: <span className="num">{duration(track.duration_ms)}</span>,
    added: <span className="muted small">{ago(track.added_at_unix_ms)}</span>,
    last_played: <span className="muted small">{ago(track.last_played_at_unix_ms)}</span>,
    plays: <span className="num">{number(track.play_count)}</span>,
    bitrate: <span className="small">{bitrate(track)}</span>,
    year: track.year ?? none,
    era: track.era ?? none,
    bpm: <span className="num">{track.bpm !== null && track.bpm !== undefined ? number(track.bpm) : "–"}</span>,
    mood: track.mood ?? none,
    sound_code: track.sound_code ?? none,
    labels: (
      <span className="chips">
        {track.labels.map((label) => (
          <LabelChip key={label.id} label={label} />
        ))}
      </span>
    ),
  };
  return (
    <tr className={previewing ? "previewing" : undefined} onClickCapture={range.row(track.id)}>
      <td>
        <input
          type="checkbox"
          aria-label={t("library.select", { name: track.title ?? file })}
          checked={selected}
          {...range.checkbox(track.id)}
        />
      </td>
      <td className="title-cell">
        <div className="row" style={{ flexWrap: "nowrap" }}>
          <Link to={`/library/${track.id}`} className="ellipsis">
            {track.title ?? file}
          </Link>
          <TrackState track={track} />
        </div>
        {previewing && (
          <span className="preview-progress" aria-hidden="true" style={{ width: `${preview.progress * 100}%` }} />
        )}
      </td>
      <td className="hide-narrow ellipsis" style={{ maxWidth: "14rem" }}>
        {track.artist ? (
          <button
            type="button"
            className="link-button"
            title={t("library.searchArtist", { artist: track.artist })}
            onClick={() => onArtist(track.artist ?? "")}
          >
            {track.artist}
          </button>
        ) : (
          none
        )}
      </td>
      {columns.map((column) => (
        <td
          key={column}
          className={column === "length" ? undefined : "hide-narrow ellipsis"}
          style={column === "album" || column === "genre" ? { maxWidth: "14rem" } : undefined}
        >
          {cells[column]}
        </td>
      ))}
      <td className="actions">
        {canQueue && playable && (
          <>
            <button
              type="button"
              className="btn ghost small icon"
              title={t("queue.playNext")}
              aria-label={t("queue.playNamedNext", { name: track.title ?? file })}
              onClick={() => onQueue(true)}
            >
              <Icon name="next" />
            </button>
            <button
              type="button"
              className="btn ghost small icon"
              title={t("library.addToQueue")}
              aria-label={t("library.addNamed", { name: track.title ?? file })}
              onClick={() => onQueue(false)}
            >
              <Icon name="plus" />
            </button>
          </>
        )}
        {!track.missing && (
          <button
            type="button"
            className={unplayable ? "btn ghost small icon warn" : "btn ghost small icon"}
            title={unplayable ? t("library.previewFailed") : previewing ? t("library.stopPreview") : t("library.preview")}
            aria-label={t(previewing ? "library.stopPreviewNamed" : "library.previewNamed", { name: track.title ?? file })}
            aria-pressed={previewing}
            onClick={() => preview.toggle(track)}
          >
            <Icon name={unplayable ? "warn" : previewing ? "stop" : "play"} />
          </button>
        )}
      </td>
    </tr>
  );
}

/** What a deletion takes, before it is confirmed. */
export function DeleteQuestion({ tracks }: { tracks: TrackSummary[] }) {
  const shown = tracks.slice(0, 15);
  return (
    <>
      <p>
        {rich("library.deleteQuestion", {
          count: tracks.length,
          trash: <Link to="/trash">{t("nav.trash")}</Link>,
        })}
      </p>
      <ul className="small" style={{ margin: 0 }}>
        {shown.map((track) => (
          <li key={track.id}>
            {trackName(track)}
            {track.missing && <span className="muted"> {t("library.fileMissing")}</span>}
          </li>
        ))}
        {tracks.length > shown.length && (
          <li className="muted">{t("library.andMore", { count: tracks.length - shown.length })}</li>
        )}
      </ul>
    </>
  );
}
