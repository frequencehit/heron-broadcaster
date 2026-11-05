// SPDX-License-Identifier: AGPL-3.0-or-later

// Actions on a selection of tracks (some IDs, or every track matching the
// library's filters): labels, categories, mood, sound code and tempo, and
// the files' tags. Labels, categories, mood and sound code also prepare
// files of the buffer zone, for the tracks they become.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useState } from "react";
import { type Schemas, api, unwrap } from "../api";
import { type PlainKey, rich, t } from "../i18n";
import { Link } from "../lib/router";
import { useAction } from "../lib/session";
import { Field, Modal, Segmented } from "./ui";
import { NewCategory } from "./NewCategory";

export type Selection = Schemas["TrackSelection"];
export type TagField = Schemas["TagField"];
export type Label = Schemas["Label"];

export function useLabels() {
  return useQuery({
    queryKey: ["labels"],
    queryFn: () => unwrap(api.GET("/api/v1/labels")),
  });
}

export function useProgrammingSettings() {
  return useQuery({
    queryKey: ["settings", "programming"],
    queryFn: () => unwrap(api.GET("/api/v1/settings/programming")),
  });
}

/** Queries every action on tracks can change. */
const TRACKS: string[][] = [["tracks"], ["track"], ["labels"], ["categories"]];

/** Queries preparing buffer files changes. */
const BUFFER: string[][] = [["buffer"], ["labels"]];

interface DialogProps {
  open: boolean;
  onClose: () => void;
  selection: Selection;
  /** How many tracks are selected. */
  tracks: number;
}

/** Library tracks, or files of the buffer zone (`files`). */
type TargetProps = Omit<DialogProps, "selection"> &
  ({ selection: Selection; files?: never } | { files: number[]; selection?: never });

type Preparation = Omit<Schemas["Preparation"], "file_ids">;
/** What an action did: on tracks, or on buffer files. */
type Applied = { tracks: number } | { files: number };

function prepare(files: number[], body: Preparation) {
  return unwrap(api.POST("/api/v1/staging/files/preparation", { body: { file_ids: files, ...body } }));
}

/** A label, as a colored chip. */
export function LabelChip({ label }: { label: { name: string; color?: string | null | undefined } }) {
  return (
    <span className="chip" style={label.color ? { borderColor: label.color } : undefined}>
      <span className="swatch" style={{ background: label.color ?? "var(--muted)" }} />
      {label.name}
    </span>
  );
}

type LabelChange = "keep" | "add" | "remove";

export function LabelsDialog({ open, onClose, selection, files, tracks }: TargetProps) {
  const labels = useLabels();
  const [changes, setChanges] = useState<Record<number, LabelChange>>({});
  const [name, setName] = useState("");
  useEffect(() => {
    if (open) {
      setChanges({});
      setName("");
    }
  }, [open]);
  const create = useAction(
    (name: string) => unwrap(api.POST("/api/v1/labels", { body: { name } })),
    {
      invalidate: [["labels"]],
      onSuccess: (label) => {
        setName("");
        setChanges((current) => ({ ...current, [label.id]: "add" }));
      },
    },
  );
  const apply = useAction(
    (): Promise<Applied> => {
      const ids = (change: LabelChange) =>
        Object.entries(changes)
          .filter(([, value]) => value === change)
          .map(([id]) => Number(id));
      if (files) {
        return prepare(files, { add_labels: ids("add"), remove_labels: ids("remove") });
      }
      return unwrap(
        api.POST("/api/v1/tracks/labels", {
          body: { selection, add: ids("add"), remove: ids("remove") },
        }),
      );
    },
    {
      success: (result) =>
        "files" in result
          ? t("buffer.prepared", { count: result.files })
          : t("selection.labelsChanged", { count: result.tracks }),
      invalidate: files ? BUFFER : TRACKS,
      onSuccess: onClose,
    },
  );
  const changed = Object.values(changes).some((value) => value !== "keep");
  return (
    <Modal
      title={files ? t("buffer.labelsTitle", { count: tracks }) : t("selection.labelsTitle", { count: tracks })}
      open={open}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={!changed || apply.isPending}
            onClick={() => apply.mutate(undefined)}
          >
            {t("selection.apply")}
          </button>
        </>
      }
    >
      <div className="stack">
        {labels.data?.length === 0 && <p className="muted">{t("selection.noLabel")}</p>}
        <ul className="list">
          {labels.data?.map((label) => (
            <li key={label.id} className="row between" style={{ padding: "0.3rem 0" }}>
              <LabelChip label={label} />
              <Segmented
                label={t("selection.changeLabel", { name: label.name })}
                value={changes[label.id] ?? "keep"}
                onChange={(value) => setChanges((current) => ({ ...current, [label.id]: value }))}
                options={[
                  { value: "keep", label: t("selection.keep") },
                  { value: "add", label: t("selection.add") },
                  { value: "remove", label: t("selection.remove") },
                ]}
              />
            </li>
          ))}
        </ul>
        <form
          className="row"
          onSubmit={(event) => {
            event.preventDefault();
            if (name.trim()) {
              create.mutate(name.trim());
            }
          }}
        >
          <input
            className="input"
            style={{ flex: 1 }}
            placeholder={t("selection.newLabel")}
            aria-label={t("selection.newLabel")}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
          <button type="submit" className="btn" disabled={!name.trim() || create.isPending}>
            {t("selection.create")}
          </button>
        </form>
        <p className="muted small" style={{ margin: 0 }}>
          {rich("selection.labelsHelp", { link: <Link to="/labels">{t("selection.manageLabels")}</Link> })}
        </p>
      </div>
    </Modal>
  );
}

export function CategoryDialog({ open, onClose, selection, files, tracks }: TargetProps) {
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const [category, setCategory] = useState("");
  const [mode, setMode] = useState<"add" | "remove">("add");
  const [weight, setWeight] = useState("1");
  useEffect(() => {
    if (open) {
      setCategory("");
      setMode("add");
      setWeight("1");
    }
  }, [open]);
  const apply = useAction(
    (): Promise<Applied> => {
      const id = Number(category);
      if (files) {
        return prepare(
          files,
          mode === "add"
            ? { add_categories: [{ category_id: id, weight: Number(weight) }] }
            : { remove_categories: [id] },
        );
      }
      return unwrap(
        api.POST("/api/v1/tracks/categories", {
          body:
            mode === "add"
              ? { selection, add: [{ category_id: id, weight: Number(weight) }] }
              : { selection, remove: [id] },
        }),
      );
    },
    {
      success: (result) =>
        "files" in result
          ? t("buffer.prepared", { count: result.files })
          : mode === "add"
            ? t("selection.inCategory", { count: result.tracks })
            : t("selection.outOfCategory", { count: result.tracks }),
      invalidate: files ? BUFFER : TRACKS,
      onSuccess: onClose,
    },
  );
  const validWeight = Number(weight) > 0;
  return (
    <Modal
      title={files ? t("buffer.categoryTitle", { count: tracks }) : t("selection.categoryTitle", { count: tracks })}
      open={open}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={!category || !validWeight || apply.isPending}
            onClick={() => apply.mutate(undefined)}
          >
            {mode === "add" ? t("selection.add") : t("selection.remove")}
          </button>
        </>
      }
    >
      <div className="stack">
        <Segmented
          label={t("selection.addOrRemove")}
          value={mode}
          onChange={setMode}
          options={[
            { value: "add", label: t("selection.putIn") },
            { value: "remove", label: t("selection.takeOut") },
          ]}
        />
        <Field label={t("library.category")}>
          <select className="input" value={category} onChange={(event) => setCategory(event.target.value)}>
            <option value="">{t("selection.choose")}</option>
            {categories.data?.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </Field>
        {mode === "add" && <NewCategory onCreated={(created) => setCategory(String(created.id))} />}
        {mode === "add" && (
          <Field label={t("track.weight")} hint={t("selection.weightHint")}>
            <input
              className={validWeight ? "input num" : "input num invalid"}
              type="number"
              min={0.01}
              step={0.1}
              value={weight}
              onChange={(event) => setWeight(event.target.value)}
            />
          </Field>
        )}
      </div>
    </Modal>
  );
}

/** Keep the tracks' values, set one, or clear it. */
export type Change<T> = { kind: "keep" } | { kind: "set"; value: T } | { kind: "clear" };

export function ChangeField({
  label,
  change,
  onChange,
  input,
  clearLabel,
}: {
  label: string;
  change: Change<string>;
  onChange: (change: Change<string>) => void;
  input: (value: string, set: (value: string) => void) => ReactNode;
  clearLabel?: string;
}) {
  return (
    <div className="field">
      <span>{label}</span>
      <div className="stack" style={{ gap: "0.35rem" }}>
        <Segmented
          label={label}
          value={change.kind}
          onChange={(kind) =>
            onChange(kind === "set" ? { kind, value: change.kind === "set" ? change.value : "" } : { kind })
          }
          options={[
            { value: "keep", label: t("selection.keep") },
            { value: "set", label: t("selection.set") },
            { value: "clear", label: clearLabel ?? t("selection.clear") },
          ]}
        />
        {change.kind === "set" && input(change.value, (value) => onChange({ kind: "set", value }))}
      </div>
    </div>
  );
}

export function changeValue<T>(change: Change<T>): T | null | undefined {
  switch (change.kind) {
    case "keep":
      return undefined;
    case "clear":
      return null;
    case "set":
      return change.value;
  }
}

export function ProgrammingDialog({ open, onClose, selection, files, tracks }: TargetProps) {
  const settings = useProgrammingSettings();
  const [mood, setMood] = useState<Change<string>>({ kind: "keep" });
  const [soundCode, setSoundCode] = useState<Change<string>>({ kind: "keep" });
  const [bpm, setBpm] = useState<Change<string>>({ kind: "keep" });
  useEffect(() => {
    if (open) {
      setMood({ kind: "keep" });
      setSoundCode({ kind: "keep" });
      setBpm({ kind: "keep" });
    }
  }, [open]);
  const apply = useAction(
    (): Promise<Applied> => {
      const moodValue = changeValue(mood);
      const codeValue = changeValue(soundCode);
      if (files) {
        // The tempo is measured once the track is analyzed.
        const preparation: Preparation = {};
        if (moodValue !== undefined) {
          preparation.mood = moodValue;
        }
        if (codeValue !== undefined) {
          preparation.sound_code = codeValue;
        }
        return prepare(files, preparation);
      }
      const body: Schemas["ProgrammingChange"] = { selection };
      const bpmValue = changeValue(bpm);
      if (moodValue !== undefined) {
        body.mood = moodValue;
      }
      if (codeValue !== undefined) {
        body.sound_code = codeValue;
      }
      if (bpmValue !== undefined) {
        body.bpm = bpmValue === null ? null : Number(bpmValue);
      }
      return unwrap(api.POST("/api/v1/tracks/programming", { body }));
    },
    {
      success: (result) =>
        "files" in result
          ? t("buffer.prepared", { count: result.files })
          : t("selection.programmingSet", { count: result.tracks }),
      invalidate: files ? BUFFER : TRACKS,
      onSuccess: onClose,
    },
  );
  const scale = (values: string[] | undefined) => (value: string, set: (value: string) => void) => (
    <select className="input" value={value} onChange={(event) => set(event.target.value)}>
      <option value="">{t("selection.choose")}</option>
      {values?.map((v) => (
        <option key={v} value={v}>
          {v}
        </option>
      ))}
    </select>
  );
  const bpmNumber = bpm.kind === "set" ? Number(bpm.value) : 0;
  const invalid =
    (mood.kind === "set" && !mood.value) ||
    (soundCode.kind === "set" && !soundCode.value) ||
    (bpm.kind === "set" && !(bpmNumber >= 20 && bpmNumber <= 400));
  const nothing = mood.kind === "keep" && soundCode.kind === "keep" && bpm.kind === "keep";
  return (
    <Modal
      title={
        files ? t("buffer.programmingTitle", { count: tracks }) : t("selection.programmingTitle", { count: tracks })
      }
      open={open}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={nothing || invalid || apply.isPending}
            onClick={() => apply.mutate(undefined)}
          >
            {t("selection.apply")}
          </button>
        </>
      }
    >
      <div className="stack">
        <ChangeField
          label={t("library.column.mood")}
          change={mood}
          onChange={setMood}
          input={scale(settings.data?.moods)}
        />
        <ChangeField
          label={t("library.column.soundCode")}
          change={soundCode}
          onChange={setSoundCode}
          input={scale(settings.data?.sound_codes)}
        />
        {!files && (
          <ChangeField
            label={t("library.column.bpm")}
            change={bpm}
            onChange={setBpm}
            clearLabel={t("selection.measured")}
            input={(value, set) => (
              <input
                className="input num"
                type="number"
                min={20}
                max={400}
                step={0.1}
                value={value}
                onChange={(event) => set(event.target.value)}
                aria-label={t("library.column.bpm")}
              />
            )}
          />
        )}
        <p className="muted small" style={{ margin: 0 }}>
          {rich("selection.programmingHelp", {
            link: <Link to="/settings">{t("selection.settingsLink")}</Link>,
            measured: <em>{t("selection.measured")}</em>,
          })}
        </p>
      </div>
    </Modal>
  );
}

export const TAG_FIELDS: { field: TagField; label: PlainKey; number?: boolean }[] = [
  { field: "title", label: "library.sort.title" },
  { field: "artist", label: "library.sort.artist" },
  { field: "album", label: "library.sort.album" },
  { field: "album_artist", label: "selection.albumArtist" },
  { field: "year", label: "library.column.year", number: true },
  { field: "genre", label: "library.column.genre" },
  { field: "track", label: "selection.trackNumber", number: true },
  { field: "disc", label: "selection.discNumber", number: true },
];

const DETAIL_FIELDS: { field: TagField; label: PlainKey }[] = [
  { field: "isrc", label: "selection.isrc" },
  { field: "record_label", label: "selection.recordLabel" },
  { field: "bpm", label: "library.column.bpm" },
  { field: "mood", label: "library.column.mood" },
];

/** The current values of one track, to start the edit from. */
export type TagValues = Partial<Record<TagField, string | number | null | undefined>>;

export function TagsDialog({
  open,
  onClose,
  selection,
  tracks,
  current,
  genres,
}: DialogProps & { current?: TagValues; genres?: string[] }) {
  const [changes, setChanges] = useState<Partial<Record<TagField, Change<string>>>>({});
  const [details, setDetails] = useState<TagField[]>([]);
  const [task, setTask] = useState<number | null>(null);
  useEffect(() => {
    if (open) {
      setChanges({});
      setDetails([]);
      setTask(null);
    }
  }, [open]);
  const status = useQuery({
    queryKey: ["library-task", task],
    queryFn: () => unwrap(api.GET("/api/v1/library/tasks/{id}", { params: { path: { id: task ?? 0 } } })),
    enabled: task !== null,
    refetchInterval: (query) =>
      query.state.data?.status === "done" || query.state.data?.status === "failed" ? false : 1000,
  });
  const library = useQuery({
    queryKey: ["settings", "library"],
    queryFn: () => unwrap(api.GET("/api/v1/settings/library")),
    enabled: open,
  });
  // With write_tags off, the tags are kept in Heron: the files are left alone.
  const keep = library.data?.write_tags === false;
  const fieldValues = () => {
    const set: Record<string, string | null> = {};
    for (const [field, change] of Object.entries(changes)) {
      const value = change ? changeValue(change) : undefined;
      if (value !== undefined) {
        set[field] = value;
      }
    }
    return set;
  };
  const apply = useAction(
    () =>
      unwrap(api.POST("/api/v1/tracks/tags", { body: { selection, set: fieldValues(), from_details: details } })),
    { onSuccess: (result) => setTask(result.id), invalidate: [["library-tasks"]] },
  );
  const save = useAction(
    () => unwrap(api.POST("/api/v1/tracks/tag-overrides", { body: { selection, set: fieldValues() } })),
    {
      success: (result) => t("selection.kept", { count: result.tracks }),
      invalidate: TRACKS,
      onSuccess: () => onClose(),
    },
  );
  const finished = status.data?.status === "done" || status.data?.status === "failed";
  const client = useQueryClient();
  useEffect(() => {
    if (finished) {
      for (const key of [...TRACKS, ["library-batches"]]) {
        void client.invalidateQueries({ queryKey: key });
      }
    }
  }, [finished, client]);

  const edited = Object.values(changes).filter((change) => change && change.kind !== "keep");
  const invalid = TAG_FIELDS.some(({ field, number }) => {
    const change = changes[field];
    return change?.kind === "set" && (!change.value.trim() || (number && !(Number(change.value) >= 1)));
  });
  const report = status.data?.report;
  return (
    <Modal
      title={t("selection.tagsTitle", { count: tracks })}
      open={open}
      onClose={onClose}
      wide
      footer={
        task === null ? (
          <>
            <button type="button" className="btn" onClick={onClose}>
              {t("common.cancel")}
            </button>
            {keep ? (
              <button
                type="button"
                className="btn primary"
                disabled={edited.length === 0 || invalid || save.isPending}
                onClick={() => save.mutate(undefined)}
              >
                {t("selection.keepTags")}
              </button>
            ) : (
              <button
                type="button"
                className="btn primary"
                disabled={
                  (edited.length === 0 && details.length === 0) || invalid || apply.isPending || library.isPending
                }
                onClick={() => apply.mutate(undefined)}
              >
                {t("selection.writeTags")}
              </button>
            )}
          </>
        ) : (
          <button type="button" className="btn primary" onClick={onClose}>
            {t("ui.close")}
          </button>
        )
      }
    >
      {task === null ? (
        <div className="stack">
          <p className="muted small" style={{ margin: 0 }}>
            {tracks > 1 ? t("selection.tagsHelpMany") : t("selection.tagsHelpOne")}{" "}
            {keep
              ? t("selection.keptHelp")
              : rich("selection.tagsUndo", { link: <Link to="/to-organize">{t("nav.review")}</Link> })}
          </p>
          <div className="form-grid">
            {TAG_FIELDS.map(({ field, label, number: numeric }) => {
              const value = current?.[field];
              const name = t(label);
              return (
                <ChangeField
                  key={field}
                  label={
                    value !== undefined && value !== null
                      ? t("selection.currentValue", { field: name, value: String(value) })
                      : name
                  }
                  change={changes[field] ?? { kind: "keep" }}
                  onChange={(change) => setChanges((all) => ({ ...all, [field]: change }))}
                  input={(text, set) => (
                    <input
                      className="input"
                      type={numeric ? "number" : "text"}
                      min={numeric ? 1 : undefined}
                      value={text}
                      list={field === "genre" ? "genre-suggestions" : undefined}
                      aria-label={name}
                      onChange={(event) => set(event.target.value)}
                    />
                  )}
                />
              );
            })}
          </div>
          {genres && genres.length > 0 && (
            <datalist id="genre-suggestions">
              {genres.map((genre) => (
                <option key={genre} value={genre} />
              ))}
            </datalist>
          )}
          {!keep && (
          <fieldset className="stack" style={{ border: 0, padding: 0, margin: 0 }}>
            <legend className="small">{t("selection.alsoDetails")}</legend>
            <div className="row">
              {DETAIL_FIELDS.map(({ field, label }) => (
                <label key={field} className="check">
                  <input
                    type="checkbox"
                    checked={details.includes(field)}
                    onChange={(event) =>
                      setDetails((all) => (event.target.checked ? [...all, field] : all.filter((f) => f !== field)))
                    }
                  />
                  {t(label)}
                </label>
              ))}
            </div>
            <span className="muted small">{t("selection.detailsHelp")}</span>
          </fieldset>
          )}
        </div>
      ) : (
        <div className="stack">
          {!finished && <p>{t("selection.writing")}</p>}
          {status.data?.status === "failed" && (
            <p className="bad">{t("selection.failed", { error: status.data.error ?? "" })}</p>
          )}
          {report && (
            <>
              <p>
                {t("selection.written", { count: report.done })}
                {(report.unchanged ?? 0) > 0 && t("selection.alreadyRight", { count: report.unchanged ?? 0 })}
                {report.skipped.length > 0 && t("selection.skipped", { count: report.skipped.length })}
                {t("library.end")}
              </p>
              {report.skipped.length > 0 && (
                <ul className="small">
                  {report.skipped.slice(0, 20).map((skipped) => (
                    <li key={skipped.path}>
                      <code>{skipped.path}</code>
                      {t("selection.skippedReason", { reason: skipped.reason })}
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>
      )}
    </Modal>
  );
}
