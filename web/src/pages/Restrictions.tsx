// SPDX-License-Identifier: AGPL-3.0-or-later

// Restrictions: hard rules on tracks, artists and categories, never
// relaxed. Dayparting, date ranges and sequences.

import { useQuery } from "@tanstack/react-query";
import { type FormEvent, useState } from "react";
import { type RestrictionView, type Rule, type Target, api, done, query, unwrap } from "../api";
import { ArtistPicker } from "../components/ArtistPicker";
import { TrackSearch } from "../components/TrackSearch";
import {
  Card,
  ConfirmButton,
  Empty,
  ErrorView,
  Field,
  Icon,
  Loading,
  Modal,
  PageHead,
  Segmented,
} from "../components/ui";
import { type PlainKey, t } from "../i18n";
import { trackName } from "../lib/format";
import { useModule } from "../lib/modules";
import { DAY_NUMBERS, dayName, describeRule, hourLabel, targetKind } from "../lib/rotation";
import { Link, setSearch, useLocation } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { useCategories } from "./Clocks";
import { StationBadges, StationChecks, useSeveralStations } from "../components/Stations";

type TargetKind = Target["kind"];

// Partial: the kind comes from the URL, which may name any.
const FILTERED: Partial<Record<string, PlainKey>> = {
  track: "restrictions.filteredTrack",
  artist: "restrictions.filteredArtist",
  category: "restrictions.filteredCategory",
};

function targetLink(target: Target): string {
  switch (target.kind) {
    case "track":
      return `/library/${target.id}`;
    case "artist":
      return `/artists?artist=${target.id}`;
    case "category":
      return `/library?category=${target.id}`;
  }
}

export function Restrictions() {
  // An artist has no page to link to while the artists module is off.
  const artistsOn = useModule("artists");
  const canEdit = useCan("operator");
  const { search } = useLocation();
  const kind = (search.get("target") as TargetKind | null) ?? undefined;
  const targetId = search.get("target_id") ? Number(search.get("target_id")) : undefined;
  const restrictions = useQuery({
    queryKey: ["restrictions", kind, targetId],
    queryFn: () =>
      unwrap(api.GET("/api/v1/restrictions", { params: { query: query({ target: kind, target_id: targetId }) } })),
  });
  const categories = useCategories();
  const categoryName = (id: number) => categories.data?.find((c) => c.id === id)?.name;
  const [editing, setEditing] = useState<RestrictionView | "new" | null>(null);
  const toggle = useAction(
    (restriction: RestrictionView) =>
      unwrap(
        api.PUT("/api/v1/restrictions/{id}", {
          params: { path: { id: restriction.id } },
          body: {
            target: restriction.target,
            rule: restriction.rule,
            enabled: !restriction.enabled,
            note: restriction.note ?? null,
          },
        }),
      ),
    { invalidate: [["restrictions"]] },
  );
  const remove = useAction(
    (id: number) => done(api.DELETE("/api/v1/restrictions/{id}", { params: { path: { id } } })),
    { success: t("restrictions.deleted"), invalidate: [["restrictions"]] },
  );

  return (
    <>
      <PageHead
        title={t("nav.restrictions")}
        description={t("restrictions.description")}
        actions={
          canEdit && (
            <button type="button" className="btn primary" onClick={() => setEditing("new")}>
              <Icon name="plus" /> {t("restrictions.new")}
            </button>
          )
        }
      />
      {(kind || targetId) && (
        <p className="row small">
          <span className="muted">{t((kind && FILTERED[kind]) ?? "restrictions.filteredTarget")}</span>
          <button type="button" className="link-button" onClick={() => setSearch({ target: null, target_id: null })}>
            {t("restrictions.showAll")}
          </button>
        </p>
      )}
      <Card flush>
        {restrictions.isPending && <Loading />}
        {restrictions.isError && <ErrorView error={restrictions.error} />}
        {restrictions.data?.length === 0 && <Empty>{t("restrictions.none")}</Empty>}
        {restrictions.data && restrictions.data.length > 0 && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("restrictions.on")}</th>
                  <th>{t("restrictions.rule")}</th>
                  <th>{t("restrictions.note")}</th>
                  <th>{t("restrictions.active")}</th>
                  <th>
                    <span className="visually-hidden">{t("library.actions")}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {restrictions.data.map((restriction) => (
                  <tr key={restriction.id} className={restriction.enabled ? undefined : "muted"}>
                    <td className="title-cell">
                      <span className="badge">{targetKind(restriction.target)}</span>{" "}
                      {restriction.target.kind === "artist" && !artistsOn ? (
                        (restriction.target_name ?? `#${restriction.target.id}`)
                      ) : (
                        <Link to={targetLink(restriction.target)}>
                          {restriction.target_name ?? `#${restriction.target.id}`}
                        </Link>
                      )}
                    </td>
                    <td>{describeRule(restriction.rule, categoryName)}</td>
                    <td className="muted small">
                      <StationBadges ids={restriction.station_ids} emptyMeansAll /> {restriction.note}
                    </td>
                    <td>
                      <input
                        type="checkbox"
                        aria-label={t("restrictions.active")}
                        checked={restriction.enabled}
                        disabled={!canEdit || toggle.isPending}
                        onChange={() => toggle.mutate(restriction)}
                      />
                    </td>
                    <td className="actions">
                      {canEdit && (
                        <>
                          <button
                            type="button"
                            className="btn ghost small icon"
                            aria-label={t("restrictions.edit")}
                            onClick={() => setEditing(restriction)}
                          >
                            <Icon name="edit" />
                          </button>
                          <ConfirmButton
                            className="btn ghost small icon danger"
                            title={t("categories.deleteButton")}
                            confirm={t("categories.deleteButton")}
                            question={<p>{t("restrictions.deleteQuestion")}</p>}
                            onConfirm={() => remove.mutate(restriction.id)}
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
          </div>
        )}
      </Card>
      {editing && (
        <RestrictionDialog restriction={editing === "new" ? null : editing} onClose={() => setEditing(null)} />
      )}
    </>
  );
}

const ALL_DAYS = [1, 2, 3, 4, 5, 6, 7];

function defaultRule(kind: Rule["kind"], categoryId: number): Rule {
  const year = new Date().getFullYear();
  switch (kind) {
    case "daypart":
      return { kind, mode: "ban", days: ALL_DAYS, start_hour: 6, end_hour: 22 };
    case "date_range":
      return { kind, mode: "allow", start: `${year}-12-01`, end: `${year + 1}-01-06`, every_year: true };
    case "not_after":
      return { kind, category_id: categoryId };
  }
}

export function RestrictionDialog({
  restriction,
  target: fixedTarget,
  onClose,
}: {
  restriction: RestrictionView | null;
  /** For a new restriction on a known target. */
  target?: { target: Target; name: string };
  onClose: () => void;
}) {
  const categories = useCategories();
  const firstCategory = categories.data?.[0]?.id ?? 0;
  const [target, setTarget] = useState<Target | null>(restriction?.target ?? fixedTarget?.target ?? null);
  const [targetName, setTargetName] = useState(restriction?.target_name ?? fixedTarget?.name ?? "");
  const [kind, setKind] = useState<TargetKind>(target?.kind ?? "category");
  const [rule, setRule] = useState<Rule>(restriction?.rule ?? defaultRule("daypart", firstCategory));
  const [note, setNote] = useState(restriction?.note ?? "");
  const several = useSeveralStations();
  const [stationIds, setStationIds] = useState<number[]>(restriction?.station_ids ?? []);
  const save = useAction(
    () => {
      if (!target) {
        throw new Error(t("restrictions.pickTarget"));
      }
      const body = {
        target,
        rule,
        enabled: restriction?.enabled ?? true,
        note: note.trim() || null,
        ...(several ? { station_ids: stationIds } : {}),
      };
      return restriction
        ? unwrap(api.PUT("/api/v1/restrictions/{id}", { params: { path: { id: restriction.id } }, body }))
        : unwrap(api.POST("/api/v1/restrictions", { body }));
    },
    { success: t("restrictions.saved"), invalidate: [["restrictions"]], onSuccess: onClose },
  );
  const submit = (event: FormEvent) => {
    event.preventDefault();
    save.mutate(undefined);
  };
  const pick = (next: Target, name: string) => {
    setTarget(next);
    setTargetName(name);
  };

  return (
    <Modal
      title={restriction ? t("restrictions.editTitle") : t("restrictions.new")}
      open
      wide
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="submit" form="restriction-form" className="btn primary" disabled={!target || save.isPending}>
            {t("common.save")}
          </button>
        </>
      }
    >
      <form id="restriction-form" className="stack" onSubmit={submit}>
        <Field label={t("restrictions.appliesTo")}>
          {fixedTarget ? (
            <span>
              <span className="badge">{targetKind(fixedTarget.target)}</span> {fixedTarget.name}
            </span>
          ) : (
            <div className="stack">
              <Segmented
                label={t("restrictions.targetKind")}
                value={kind}
                options={[
                  { value: "category", label: t("rotation.target.category") },
                  { value: "artist", label: t("rotation.target.artist") },
                  { value: "track", label: t("rotation.target.track") },
                ]}
                onChange={(next) => {
                  setKind(next);
                  setTarget(null);
                  setTargetName("");
                }}
              />
              {kind === "category" && (
                <select
                  className="input"
                  aria-label={t("rotation.target.category")}
                  value={target?.kind === "category" ? target.id : ""}
                  onChange={(event) => {
                    const id = Number(event.target.value);
                    pick({ kind: "category", id }, categories.data?.find((c) => c.id === id)?.name ?? "");
                  }}
                >
                  <option value="" disabled>
                    {t("restrictions.pickCategory")}
                  </option>
                  {categories.data?.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>
              )}
              {kind !== "category" && target && (
                <div className="row">
                  <strong>{targetName}</strong>
                  <button
                    type="button"
                    className="link-button"
                    onClick={() => {
                      setTarget(null);
                      setTargetName("");
                    }}
                  >
                    {t("restrictions.change")}
                  </button>
                </div>
              )}
              {kind === "artist" && !target && (
                <ArtistPicker
                  actions={(artist) => (
                    <button
                      type="button"
                      className="btn small"
                      onClick={() => pick({ kind: "artist", id: artist.id }, artist.name)}
                    >
                      {t("restrictions.pick")}
                    </button>
                  )}
                />
              )}
              {kind === "track" && !target && (
                <TrackSearch
                  playableOnly={false}
                  actions={(track) => (
                    <button
                      type="button"
                      className="btn small"
                      onClick={() => pick({ kind: "track", id: track.id }, trackName(track))}
                    >
                      {t("restrictions.pick")}
                    </button>
                  )}
                />
              )}
            </div>
          )}
        </Field>
        <Field label={t("restrictions.rule")}>
          <Segmented
            label={t("restrictions.ruleKind")}
            value={rule.kind}
            options={[
              { value: "daypart", label: t("restrictions.daypart") },
              { value: "date_range", label: t("restrictions.dates") },
              { value: "not_after", label: t("restrictions.sequence") },
            ]}
            onChange={(next) => setRule(next === rule.kind ? rule : defaultRule(next, firstCategory))}
          />
        </Field>
        <RuleFields rule={rule} onChange={setRule} />
        <p className="muted small" style={{ margin: 0 }}>
          {describeRule(rule, (id) => categories.data?.find((c) => c.id === id)?.name)}.
        </p>
        {several && (
          <Field label={t("restrictions.stations")} hint={t("restrictions.stationsHint")}>
            <StationChecks value={stationIds} onChange={setStationIds} />
          </Field>
        )}
        <Field label={t("restrictions.note")} hint={t("restrictions.noteHint")}>
          <input className="input" value={note} maxLength={512} onChange={(event) => setNote(event.target.value)} />
        </Field>
      </form>
    </Modal>
  );
}

function RuleFields({ rule, onChange }: { rule: Rule; onChange: (rule: Rule) => void }) {
  const categories = useCategories();
  const mode =
    rule.kind === "not_after" ? null : (
      <Segmented
        label={t("restrictions.mode")}
        value={rule.mode}
        options={[
          { value: "ban", label: t("restrictions.ban") },
          { value: "allow", label: t("restrictions.allow") },
        ]}
        onChange={(next) => onChange({ ...rule, mode: next })}
      />
    );
  switch (rule.kind) {
    case "daypart":
      return (
        <div className="stack">
          {mode}
          <fieldset>
            <legend>{t("restrictions.days")}</legend>
            <div className="row">
              {DAY_NUMBERS.map((day) => {
                const name = dayName(day);
                return (
                  <label key={day} className="check">
                    <input
                      type="checkbox"
                      checked={rule.days.includes(day)}
                      onChange={(event) =>
                        onChange({
                          ...rule,
                          days: event.target.checked
                            ? [...rule.days, day].sort((a, b) => a - b)
                            : rule.days.filter((d) => d !== day),
                        })
                      }
                    />
                    {name}
                  </label>
                );
              })}
            </div>
          </fieldset>
          <div className="form-grid">
            <Field label={t("review.from")}>
              <select
                className="input"
                value={rule.start_hour}
                onChange={(event) => onChange({ ...rule, start_hour: Number(event.target.value) })}
              >
                {Array.from({ length: 24 }, (_, hour) => (
                  <option key={hour} value={hour}>
                    {hourLabel(hour)}
                  </option>
                ))}
              </select>
            </Field>
            <Field label={t("review.to")} hint={rule.end_hour <= rule.start_hour ? t("restrictions.pastMidnight") : undefined}>
              <select
                className="input"
                value={rule.end_hour}
                onChange={(event) => onChange({ ...rule, end_hour: Number(event.target.value) })}
              >
                {Array.from({ length: 24 }, (_, index) => index + 1).map((hour) => (
                  <option key={hour} value={hour}>
                    {hour === 24 ? "24:00" : hourLabel(hour)}
                  </option>
                ))}
              </select>
            </Field>
          </div>
        </div>
      );
    case "date_range":
      return (
        <div className="stack">
          {mode}
          <div className="form-grid">
            <Field label={t("review.from")}>
              <input
                className="input"
                type="date"
                value={rule.start}
                required
                onChange={(event) => onChange({ ...rule, start: event.target.value })}
              />
            </Field>
            <Field label={t("review.to")} hint={t("restrictions.included")}>
              <input
                className="input"
                type="date"
                value={rule.end}
                required
                onChange={(event) => onChange({ ...rule, end: event.target.value })}
              />
            </Field>
            <label className="check">
              <input
                type="checkbox"
                checked={rule.every_year}
                onChange={(event) => onChange({ ...rule, every_year: event.target.checked })}
              />
              {t("restrictions.everyYear")}
            </label>
          </div>
        </div>
      );
    case "not_after":
      return (
        <Field label={t("restrictions.notAfter")} hint={t("restrictions.notAfterHint")}>
          <select
            className="input"
            value={rule.category_id}
            onChange={(event) => onChange({ ...rule, category_id: Number(event.target.value) })}
          >
            {categories.data?.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </Field>
      );
  }
}
