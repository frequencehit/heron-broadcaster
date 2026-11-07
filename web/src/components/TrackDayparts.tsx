// SPDX-License-Identifier: AGPL-3.0-or-later

// A track's own daypart separation (docs/rotation.md#daypart-separation):
// its dayparts kept out and its same daypart on the following days, each
// replacing its categories' value, on every station. On the track's page,
// as a column of the track list, and as a bulk edit.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { type Schemas, api, unwrap } from "../api";
import { number, t } from "../i18n";
import { maxSeparation } from "../lib/dayparts";
import { useAction } from "../lib/session";
import { Card, Field, Modal } from "./ui";
import { MAX_DAYS } from "./Dayparts";
import { type Change, ChangeField, type Selection, changeValue } from "./SelectionActions";

/** A track's own values; `null`: its categories'. */
export interface OwnDayparts {
  dayparts?: number | null;
  daypart_days?: number | null;
}

/** The longest a track's own separation can be: 10, cut to 6 on a
 * station of 4 dayparts. */
const MAX_SEPARATION = maxSeparation(6);

/** Queries a change of a track's daypart values touches. */
const TRACKS: string[][] = [["tracks"], ["track"]];

/** `2`, `off`, or `null` for the categories'. */
function separationText(value: number | null | undefined): string | null {
  if (value === null || value === undefined) {
    return null;
  }
  return value === 0 ? t("trackDayparts.offShort") : number(value);
}

/** `1 d`, `off`, or `null` for the categories'. */
function daysText(value: number | null | undefined): string | null {
  if (value === null || value === undefined) {
    return null;
  }
  return value === 0 ? t("trackDayparts.offShort") : t("format.days", { count: value });
}

/** Whether the track sets either value. */
export function hasOwnDayparts(track: OwnDayparts): boolean {
  return (track.dayparts ?? null) !== null || (track.daypart_days ?? null) !== null;
}

/** The track list's cell: the track's own values, `–` when it follows its
 * categories. */
export function DaypartsCell({ track }: Readonly<{ track: OwnDayparts }>) {
  if (!hasOwnDayparts(track)) {
    return (
      <span className="muted" title={t("trackDayparts.followsCategories")}>
        –
      </span>
    );
  }
  const separation = separationText(track.dayparts);
  const days = daysText(track.daypart_days);
  const category = t("trackDayparts.categoryShort");
  return (
    <span
      className="small"
      title={t("trackDayparts.cellTitle", { dayparts: separation ?? category, days: days ?? category })}
    >
      <span className={separation === null ? "muted" : "num"}>{separation ?? category}</span>
      {" · "}
      <span className={days === null ? "muted" : "num"}>{days ?? category}</span>
    </span>
  );
}

/** "The category's", "0: off", then each value up to `max`. */
function OwnSelect({
  label,
  value,
  max,
  option,
  disabled,
  onChange,
}: Readonly<{
  label: string;
  value: number | null;
  max: number;
  option: (n: number) => string;
  disabled?: boolean;
  onChange: (value: number | null) => void;
}>) {
  return (
    <select
      className="input"
      aria-label={label}
      value={value === null ? "" : String(value)}
      disabled={disabled}
      onChange={(event) => onChange(event.target.value === "" ? null : Number(event.target.value))}
    >
      <option value="">{t("trackDayparts.categoryOption")}</option>
      {Array.from({ length: max + 1 }, (_, n) => (
        <option key={n} value={n}>
          {n === 0 ? t("dayparts.offOption") : option(n)}
        </option>
      ))}
    </select>
  );
}

function setOwn(selection: Selection, change: { dayparts?: number | null; daypart_days?: number | null }) {
  const body: Schemas["ProgrammingChange"] = { selection, ...change };
  return unwrap(api.POST("/api/v1/tracks/programming", { body }));
}

/** On the track's page: its own values, and what applies in each of its
 * categories. */
export function TrackDaypartsCard({
  track,
  canEdit,
}: Readonly<{
  track: Schemas["TrackDetail"];
  canEdit: boolean;
}>) {
  const savedSeparation = track.dayparts ?? null;
  const savedDays = track.daypart_days ?? null;
  const [separation, setSeparation] = useState(savedSeparation);
  const [days, setDays] = useState(savedDays);
  useEffect(() => setSeparation(savedSeparation), [savedSeparation]);
  useEffect(() => setDays(savedDays), [savedDays]);
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const save = useAction(
    () => {
      const change: OwnDayparts = {};
      if (separation !== savedSeparation) {
        change.dayparts = separation;
      }
      if (days !== savedDays) {
        change.daypart_days = days;
      }
      return setOwn({ track_ids: [track.id] }, change);
    },
    { invalidate: TRACKS },
  );
  const dirty = separation !== savedSeparation || days !== savedDays;
  const own = t("trackDayparts.own");
  return (
    <Card
      title={t("trackDayparts.title")}
      actions={
        canEdit &&
        dirty && (
          <button
            type="button"
            className="btn small primary"
            disabled={save.isPending}
            onClick={() => save.mutate(undefined)}
          >
            {t("common.save")}
          </button>
        )
      }
    >
      <div className="stack">
        <p className="muted small" style={{ margin: 0 }}>
          {t("trackDayparts.intro")}
        </p>
        <Field label={t("dayparts.category")} hint={t("trackDayparts.maxHint")}>
          <OwnSelect
            label={t("dayparts.category")}
            value={separation}
            max={MAX_SEPARATION}
            option={(n) => String(n)}
            disabled={!canEdit}
            onChange={setSeparation}
          />
        </Field>
        <Field label={t("dayparts.days")} hint={t("dayparts.daysHint")}>
          <OwnSelect
            label={t("dayparts.days")}
            value={days}
            max={MAX_DAYS}
            option={(n) => t("dayparts.daysOption", { count: n })}
            disabled={!canEdit}
            onChange={setDays}
          />
        </Field>
        {track.categories.length === 0 ? (
          <p className="muted small" style={{ margin: 0 }}>
            {t("trackDayparts.noCategory")}
          </p>
        ) : (
          <table className="table small">
            <thead>
              <tr>
                <th>{t("trackDayparts.inCategory")}</th>
                <th className="num">{t("categories.dayparts")}</th>
                <th className="num">{t("trackDayparts.sameDaypart")}</th>
              </tr>
            </thead>
            <tbody>
              {track.categories.map((category) => {
                const values = categories.data?.find((c) => c.id === category.category_id)?.separation;
                const n = separation ?? values?.dayparts ?? 0;
                const x = days ?? values?.daypart_days ?? 0;
                return (
                  <tr key={category.category_id}>
                    <td>
                      <span className="row">
                        <span className="swatch" style={{ background: category.color ?? undefined }} />
                        {category.name}
                      </span>
                    </td>
                    <td className="num">
                      {n === 0 ? t("trackDayparts.offShort") : number(n)}
                      {separation !== null && <span className="muted"> ({own})</span>}
                    </td>
                    <td className="num">
                      {x === 0 ? t("trackDayparts.offShort") : t("format.days", { count: x })}
                      {days !== null && <span className="muted"> ({own})</span>}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </Card>
  );
}

/** The bulk edit: keep, set or go back to the categories', each value on
 * its own. */
export function TrackDaypartsDialog({
  open,
  onClose,
  selection,
  tracks,
}: Readonly<{
  open: boolean;
  onClose: () => void;
  selection: Selection;
  tracks: number;
}>) {
  const [separation, setSeparation] = useState<Change<string>>({ kind: "keep" });
  const [days, setDays] = useState<Change<string>>({ kind: "keep" });
  useEffect(() => {
    if (open) {
      setSeparation({ kind: "keep" });
      setDays({ kind: "keep" });
    }
  }, [open]);
  const apply = useAction(
    () => {
      const change: OwnDayparts = {};
      const n = changeValue(separation);
      const x = changeValue(days);
      if (n !== undefined) {
        change.dayparts = n === null ? null : Number(n);
      }
      if (x !== undefined) {
        change.daypart_days = x === null ? null : Number(x);
      }
      return setOwn(selection, change);
    },
    {
      success: (result) => t("trackDayparts.done", { count: result.tracks }),
      invalidate: TRACKS,
      onSuccess: onClose,
    },
  );
  const choose = (max: number, option: (n: number) => string, label: string) =>
    function Input(value: string, set: (value: string) => void) {
      return (
        <select className="input" aria-label={label} value={value} onChange={(event) => set(event.target.value)}>
          <option value="">{t("selection.choose")}</option>
          {Array.from({ length: max + 1 }, (_, n) => (
            <option key={n} value={n}>
              {n === 0 ? t("dayparts.offOption") : option(n)}
            </option>
          ))}
        </select>
      );
    };
  const invalid = (separation.kind === "set" && !separation.value) || (days.kind === "set" && !days.value);
  const nothing = separation.kind === "keep" && days.kind === "keep";
  return (
    <Modal
      title={t("trackDayparts.bulkTitle", { count: tracks })}
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
        <p className="muted small" style={{ margin: 0 }}>
          {t("trackDayparts.intro")}
        </p>
        <ChangeField
          label={t("dayparts.category")}
          change={separation}
          onChange={setSeparation}
          clearLabel={t("trackDayparts.categoryOption")}
          input={choose(MAX_SEPARATION, (n) => String(n), t("dayparts.category"))}
        />
        <ChangeField
          label={t("dayparts.days")}
          change={days}
          onChange={setDays}
          clearLabel={t("trackDayparts.categoryOption")}
          input={choose(MAX_DAYS, (n) => t("dayparts.daysOption", { count: n }), t("dayparts.days"))}
        />
        <p className="muted small" style={{ margin: 0 }}>
          {t("trackDayparts.maxHint")}
        </p>
      </div>
    </Modal>
  );
}
