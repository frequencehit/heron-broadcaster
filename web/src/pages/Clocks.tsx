// SPDX-License-Identifier: AGPL-3.0-or-later

// Clocks: ordered slots, each calling a category. The editor shows the
// hour as it will sound (expected starts, categories running short) and
// simulates it.

import {
  DndContext,
  type DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  type Category,
  type ClockPreview,
  type ClockSlot,
  type ClockView,
  type Method,
  type SimulationReport,
  api,
  done,
  query,
  unwrap,
} from "../api";
import { SeparationEditor, applyOverrides } from "../components/Separation";
import { useDebounced } from "../components/TrackSearch";
import {
  Banner,
  Card,
  ConfirmButton,
  Empty,
  ErrorView,
  Field,
  Icon,
  Loading,
  Modal,
  PageHead,
} from "../components/ui";
import { number, rich, t } from "../i18n";
import { duration } from "../lib/format";
import { METHODS_LIST, describeOverrides, methodLabel, stepLabel } from "../lib/rotation";
import { Link, navigate } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { here, useStation } from "../lib/station";
import { useCategoryTypes } from "./CategoryTypes";

const HOUR_MS = 3_600_000;

export function useCategories() {
  return useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
}

/** The categories the station shown can use: those tagged with it. */
export function useStationCategories() {
  const station = useStation();
  return useQuery({
    queryKey: ["categories", station],
    queryFn: () =>
      unwrap(api.GET("/api/v1/categories", { params: { query: query({ station_id: station }) } })),
  });
}

export function Clocks() {
  const canEdit = useCan("operator");
  const clocks = useQuery({ queryKey: ["clocks"], queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/clocks", { params: { path: here() } })) });
  const categories = useCategories();
  const byId = new Map((categories.data ?? []).map((c) => [c.id, c]));
  return (
    <>
      <PageHead
        title={t("nav.clocks")}
        description={rich("clocks.description", { link: <Link to="/schedule">{t("clocks.scheduleLink")}</Link> })}
        actions={
          canEdit && (
            <button type="button" className="btn primary" onClick={() => navigate("/clocks/new")}>
              <Icon name="plus" /> {t("clocks.new")}
            </button>
          )
        }
      />
      <Card flush>
        {clocks.isPending && <Loading />}
        {clocks.isError && <ErrorView error={clocks.error} />}
        {clocks.data?.length === 0 && (
          <Empty>{t("clocks.none")}</Empty>
        )}
        {clocks.data && clocks.data.length > 0 && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("categories.name")}</th>
                  <th>{t("clocks.slots")}</th>
                  <th>{t("clocks.scheduled")}</th>
                </tr>
              </thead>
              <tbody>
                {clocks.data.map((clock) => (
                  <tr key={clock.id}>
                    <td>
                      <Link to={`/clocks/${clock.id}`}>{clock.name}</Link>
                      {clock.looping && (
                        <>
                          {" "}
                          <span className="badge" title={t("clocks.loopsHelp")}>
                            {t("clocks.loops")}
                          </span>
                        </>
                      )}
                    </td>
                    <td>
                      <span className="slot-strip" title={t("clocks.slotCount", { count: clock.slots.length })}>
                        {clock.slots.map((slot, index) => (
                          <span
                            key={index}
                            className="swatch"
                            title={byId.get(slot.category_id)?.name}
                            style={{ background: byId.get(slot.category_id)?.color ?? "var(--wave)" }}
                          />
                        ))}
                        {clock.slots.length === 0 && <span className="muted small">{t("clocks.empty")}</span>}
                      </span>
                    </td>
                    <td className="num">
                      {clock.scheduled_hours === 0 ? (
                        <span className="muted">{t("clocks.notScheduled")}</span>
                      ) : (
                        t("clocks.hoursAWeek", { count: clock.scheduled_hours })
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </>
  );
}

/** A slot as edited, with a key that stays when slots move. */
interface Draft extends ClockSlot {
  key: number;
}

export function ClockEditor({ id }: { id: number | null }) {
  const clock = useQuery({
    queryKey: ["clocks", id],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/clocks/{id}", { params: { path: here({ id: id ?? 0 }) } })),
    enabled: id !== null,
  });
  if (id !== null && clock.isPending) {
    return <Loading />;
  }
  if (clock.isError) {
    return <ErrorView error={clock.error} />;
  }
  return <Editor key={id ?? "new"} clock={clock.data ?? null} />;
}

function Editor({ clock }: { clock: ClockView | null }) {
  const canEdit = useCan("operator");
  const categories = useStationCategories();
  const rotation = useQuery({
    queryKey: ["settings", "rotation"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/rotation", { params: { path: here() } })),
  });
  const nextKey = useRef(1);
  const withKeys = (slots: ClockSlot[]): Draft[] => slots.map((slot) => ({ ...slot, key: nextKey.current++ }));
  const [name, setName] = useState(clock?.name ?? "");
  const [looping, setLooping] = useState(clock?.looping ?? false);
  const [slots, setSlots] = useState<Draft[]>(() => withKeys(clock?.slots ?? []));
  const [separating, setSeparating] = useState<number | null>(null);
  const saved = useMemo(
    () => JSON.stringify({ name: clock?.name ?? "", looping: clock?.looping ?? false, slots: clock?.slots ?? [] }),
    [clock],
  );
  const plain = slots.map(({ key: _key, ...slot }) => slot);
  const dirty = JSON.stringify({ name, looping, slots: plain }) !== saved;

  const settled = useDebounced(plain, 300);
  const preview = useQuery({
    queryKey: ["clock-preview", settled, looping],
    queryFn: () => unwrap(api.POST("/api/v1/stations/{station_id}/clocks/preview", { params: { path: here() }, body: { slots: settled, looping } })),
    placeholderData: keepPreviousData,
  });

  const save = useAction(
    () => {
      const body = { name, looping, slots: plain };
      return clock
        ? unwrap(api.PUT("/api/v1/stations/{station_id}/clocks/{id}", { params: { path: here({ id: clock.id }) }, body }))
        : unwrap(api.POST("/api/v1/stations/{station_id}/clocks", { params: { path: here() }, body }));
    },
    {
      success: t("clocks.saved"),
      invalidate: [["clocks"]],
      onSuccess: (saved) => {
        if (!clock) {
          navigate(`/clocks/${saved.id}`, { replace: true });
        }
      },
    },
  );
  const remove = useAction(
    () => done(api.DELETE("/api/v1/stations/{station_id}/clocks/{id}", { params: { path: here({ id: clock?.id ?? 0 }) } })),
    { success: t("clocks.deleted"), invalidate: [["clocks"], ["schedule"]], onSuccess: () => navigate("/clocks") },
  );

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  const onDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) {
      return;
    }
    const from = slots.findIndex((slot) => slot.key === active.id);
    const to = slots.findIndex((slot) => slot.key === over.id);
    if (from >= 0 && to >= 0) {
      setSlots(arrayMove(slots, from, to));
    }
  };
  const update = (key: number, changes: Partial<ClockSlot>) =>
    setSlots((current) => current.map((slot) => (slot.key === key ? { ...slot, ...changes } : slot)));

  const byId = new Map((categories.data ?? []).map((c) => [c.id, c]));
  const firstCategory = categories.data?.[0];
  const editingSlot = slots.find((slot) => slot.key === separating);
  const shown = preview.data && preview.data.slots.length === slots.length ? preview.data : undefined;

  return (
    <>
      <PageHead
        title={clock ? clock.name : t("clocks.new")}
        actions={
          canEdit && (
            <>
              {clock && (
                <ConfirmButton
                  className="btn danger"
                  confirm={t("categories.deleteButton")}
                  question={<p>{rich("clocks.deleteQuestion", { name: <strong>{clock.name}</strong> })}</p>}
                  onConfirm={() => remove.mutate(undefined)}
                >
                  <Icon name="trash" /> {t("categories.deleteButton")}
                </ConfirmButton>
              )}
              <button
                type="button"
                className="btn primary"
                disabled={!dirty || !name.trim() || save.isPending}
                onClick={() => save.mutate(undefined)}
              >
                {t("common.save")}
              </button>
            </>
          )
        }
      />
      <div className="grid sidebar-right">
        <div className="stack">
          <Card title={t("clocks.slots")} flush>
            <div className="card-pad">
              <Field label={t("categories.name")}>
                <input
                  className="input"
                  value={name}
                  maxLength={255}
                  disabled={!canEdit}
                  onChange={(event) => setName(event.target.value)}
                />
              </Field>
              <label className="check" style={{ marginTop: "0.75rem" }}>
                <input
                  type="checkbox"
                  checked={looping}
                  disabled={!canEdit}
                  onChange={(event) => setLooping(event.target.checked)}
                />
                {t("clocks.loop")}
              </label>
              <p className="muted small" style={{ margin: "0.25rem 0 0" }}>
                {looping
                  ? t("clocks.loopHelp")
                  : t("clocks.noLoopHelp")}
              </p>
            </div>
            {categories.data?.length === 0 && (
              <Banner kind="warn">
                {rich("clocks.needCategories", { link: <Link to="/categories">{t("clocks.createSome")}</Link> })}
              </Banner>
            )}
            {slots.length === 0 ? (
              <Empty>{t("clocks.noSlot")}</Empty>
            ) : (
              <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
                <SortableContext items={slots.map((slot) => slot.key)} strategy={verticalListSortingStrategy}>
                  <ol className="list" aria-label={t("clocks.slots")}>
                    {slots.map((slot, index) => (
                      <SlotRow
                        key={slot.key}
                        slot={slot}
                        index={index}
                        categories={categories.data ?? []}
                        preview={shown?.slots[index]}
                        canEdit={canEdit}
                        onChange={(changes) => update(slot.key, changes)}
                        onSeparation={() => setSeparating(slot.key)}
                        onRemove={() => setSlots(slots.filter((s) => s.key !== slot.key))}
                      />
                    ))}
                  </ol>
                </SortableContext>
              </DndContext>
            )}
            {canEdit && firstCategory && (
              <div className="card-pad row">
                <button
                  type="button"
                  className="btn"
                  onClick={() => {
                    const last = slots[slots.length - 1];
                    setSlots([
                      ...slots,
                      {
                        key: nextKey.current++,
                        category_id: last?.category_id ?? firstCategory.id,
                        method: "least_recently_played",
                        enforce_rules: true,
                        separation: {},
                      },
                    ]);
                  }}
                >
                  <Icon name="plus" /> {t("clocks.addSlot")}
                </button>
                <span className="muted small">{t("clocks.dragHelp")}</span>
              </div>
            )}
          </Card>
        </div>
        <div className="stack">
          <HourCard
            preview={shown}
            categories={byId}
            slots={plain}
            looping={looping}
            onRulesOff={
              canEdit
                ? () =>
                    setSlots((current) =>
                      current.map((slot, index) =>
                        shown?.slots[index]?.short ? { ...slot, enforce_rules: false } : slot,
                      ),
                    )
                : undefined
            }
          />
          {clock && <SimulateCard clockId={clock.id} dirty={dirty} />}
        </div>
      </div>
      {editingSlot && (
        <Modal
          title={t("clocks.slotSeparation", { slot: slots.indexOf(editingSlot) + 1 })}
          open
          onClose={() => setSeparating(null)}
          footer={
            <button type="button" className="btn primary" onClick={() => setSeparating(null)}>
              {t("clocks.done")}
            </button>
          }
        >
          <p className="muted small">
            {t("clocks.overrideHelp", { category: byId.get(editingSlot.category_id)?.name ?? t("clocks.theCategory") })}
          </p>
          <SeparationEditor
            value={editingSlot.separation ?? {}}
            inherited={
              rotation.data
                ? applyOverrides(rotation.data.separation, byId.get(editingSlot.category_id)?.separation)
                : undefined
            }
            disabled={!canEdit}
            onChange={(separation) => update(editingSlot.key, { separation })}
          />
        </Modal>
      )}
    </>
  );
}

function SlotRow({
  slot,
  index,
  categories,
  preview,
  canEdit,
  onChange,
  onSeparation,
  onRemove,
}: {
  slot: Draft;
  index: number;
  categories: Category[];
  preview: ClockPreview["slots"][number] | undefined;
  canEdit: boolean;
  onChange: (changes: Partial<ClockSlot>) => void;
  onSeparation: () => void;
  onRemove: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: slot.key,
    disabled: !canEdit,
  });
  const category = categories.find((c) => c.id === slot.category_id);
  const overrides = describeOverrides(slot.separation);
  return (
    <li
      ref={setNodeRef}
      className={`queue-item slot${isDragging ? " dragging" : ""}${preview?.beyond_hour ? " beyond" : ""}`}
      style={{ transform: CSS.Transform.toString(transform), transition }}
    >
      {canEdit && (
        <button
          type="button"
          className="btn ghost small icon handle"
          aria-label={t("clocks.moveSlot", { slot: index + 1 })}
          {...attributes}
          {...listeners}
        >
          <Icon name="grip" />
        </button>
      )}
      <span className="index">{index + 1}</span>
      <span className="swatch" style={{ background: category?.color ?? "var(--wave)" }} />
      <div className="meta stack" style={{ gap: "0.35rem" }}>
        <div className="row">
          <select
            className="input"
            aria-label={t("clocks.slotCategory", { slot: index + 1 })}
            value={slot.category_id}
            disabled={!canEdit}
            onChange={(event) => onChange({ category_id: Number(event.target.value) })}
          >
            {categories.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
          <select
            className="input"
            aria-label={t("clocks.slotMethod", { slot: index + 1 })}
            value={slot.method}
            disabled={!canEdit}
            onChange={(event) => onChange({ method: event.target.value as Method })}
          >
            {METHODS_LIST.map((method) => (
              <option key={method} value={method}>
                {methodLabel(method)}
              </option>
            ))}
          </select>
          <label className="check small" title={t("clocks.enforceHelp")}>
            <input
              type="checkbox"
              checked={slot.enforce_rules}
              disabled={!canEdit}
              onChange={(event) => onChange({ enforce_rules: event.target.checked })}
            />
            {t("clocks.enforce")}
          </label>
        </div>
        <div className="row small muted" style={{ gap: "0.4rem" }}>
          {preview && (
            <>
              <span className="num" title={t("clocks.expectedStart")}>
                +{duration(preview.expected_start_ms)}
              </span>
              <span>·</span>
              <span>
                {preview.average_ms !== null && preview.average_ms !== undefined
                  ? t("clocks.tracksAverage", { count: preview.tracks, average: duration(preview.average_ms) })
                  : t("common.tracks", { count: preview.tracks })}
              </span>
              {preview.short && (
                <span
                  className="badge warn"
                  title={t("clocks.neededTracks", { count: preview.needed_tracks })}
                >
                  {t("clocks.runningShort")}
                </span>
              )}
              {preview.beyond_hour && <span className="badge">{t("clocks.afterHour")}</span>}
            </>
          )}
          <button type="button" className="link-button" onClick={onSeparation}>
            {overrides ? t("clocks.separationValue", { windows: overrides }) : canEdit ? t("clocks.overrideSeparation") : t("categories.separation")}
          </button>
        </div>
      </div>
      {canEdit && (
        <button
          type="button"
          className="btn ghost small icon danger"
          aria-label={t("clocks.removeSlot", { slot: index + 1 })}
          onClick={onRemove}
        >
          <Icon name="trash" />
        </button>
      )}
    </li>
  );
}

/** Marks along the bar: quarters of the hour, or half hours of a long loop. */
function ticks(span: number): { at: number; label: string }[] {
  if (span <= HOUR_MS) {
    return [0, 15, 30, 45, 60].map((minute) => ({ at: minute * 60_000, label: `:${String(minute).padStart(2, "0")}` }));
  }
  const marks = [];
  for (let at = 0; at <= span; at += 30 * 60_000) {
    marks.push({ at, label: duration(at).replace(/:00$/, "") });
  }
  return marks;
}

/** The hour (or the loop), drawn: each slot as long as its category's average track. */
function HourCard({
  preview,
  categories,
  slots,
  looping,
  onRulesOff,
}: {
  preview: ClockPreview | undefined;
  categories: Map<number, Category>;
  slots: ClockSlot[];
  looping: boolean;
  /** Turns enforce rules off on the slots running short; unset for viewers. */
  onRulesOff: (() => void) | undefined;
}) {
  const short = preview?.slots.filter((slot) => slot.short) ?? [];
  // Two slots in a row calling a type that never plays twice in a row: the
  // second is passed over (the last slot is followed by the first).
  const types = useCategoryTypes();
  const unrepeatable = (slot: ClockSlot | undefined) => {
    const type = slot && categories.get(slot.category_id)?.type_id;
    return type != null && types.data?.some((each) => each.id === type && each.no_two_in_a_row) ? type : null;
  };
  const inARow = slots.filter((slot, index) => {
    const type = unrepeatable(slot);
    const before = index > 0 ? slots[index - 1] : slots.length > 1 ? slots[slots.length - 1] : undefined;
    return type !== null && unrepeatable(before) === type;
  });
  const span = looping && preview ? Math.max(HOUR_MS, preview.duration_ms) : HOUR_MS;
  return (
    <Card title={looping ? t("clocks.theLoop") : t("clocks.theHour")}>
      {!preview ? (
        <Loading />
      ) : slots.length === 0 ? (
        <Empty>{t("clocks.addSlotsToSee")}</Empty>
      ) : (
        <div className="stack">
          <div className="hour-bar" role="img" aria-label={looping ? t("clocks.barLoop") : t("clocks.barHour")}>
            {preview.slots.map((slot, index) => {
              const start = slot.expected_start_ms;
              if (start >= span) {
                return null;
              }
              const length = Math.min(slot.average_ms ?? 210_000, span - start);
              return (
                <span
                  key={index}
                  className="segment"
                  title={t("clocks.segment", { slot: index + 1, category: categories.get(slot.category_id)?.name ?? "?", start: duration(start) })}
                  style={{
                    left: `${(start / span) * 100}%`,
                    width: `${(length / span) * 100}%`,
                    background: slot.color ?? "var(--wave)",
                  }}
                />
              );
            })}
            {preview.duration_ms > 0 && preview.duration_ms < span && (
              <span
                className="segment repeat"
                title={t("clocks.startsOverHelp")}
                style={{ left: `${(preview.duration_ms / span) * 100}%`, right: 0 }}
              />
            )}
          </div>
          <div className="hour-ticks muted small" aria-hidden="true">
            {ticks(span).map((tick) => (
              <span key={tick.at}>{tick.label}</span>
            ))}
          </div>
          <dl className="facts">
            <dt>{looping ? t("clocks.oneRound") : t("clocks.expectedLength")}</dt>
            <dd className="num">{duration(preview.duration_ms)}</dd>
            {preview.runs_per_hour > 1 && (
              <>
                <dt>{t("clocks.startsOver")}</dt>
                <dd>{t("clocks.timesAnHour", { times: number(preview.runs_per_hour, { minimumFractionDigits: 1, maximumFractionDigits: 1 }) })}</dd>
              </>
            )}
            {!looping && preview.duration_ms > HOUR_MS && (
              <>
                <dt>{t("clocks.pastHour")}</dt>
                <dd>{t("clocks.pastHourHelp", { count: preview.slots.filter((s) => s.beyond_hour).length })}</dd>
              </>
            )}
          </dl>
          {inARow.length > 0 && (
            <Banner kind="warn">
              {t("clocks.inARow", {
                count: inARow.length,
                types: [
                  ...new Set(
                    inARow.map((slot) => categories.get(slot.category_id)?.type_name ?? t("clocks.aCategory")),
                  ),
                ].join(", "),
              })}
            </Banner>
          )}
          {short.length > 0 && (
            <Banner kind="warn">
              {t("clocks.tooFew", {
                count: short.length,
                names: [...new Set(short.map((slot) => categories.get(slot.category_id)?.name ?? t("clocks.aCategory")))].join(", "),
              })}
              {onRulesOff && (
                <div className="row" style={{ marginTop: "0.5rem" }}>
                  <button type="button" className="btn small" title={t("clocks.enforceHelp")} onClick={onRulesOff}>
                    {t("clocks.rulesOff", { count: short.length })}
                  </button>
                </div>
              )}
            </Banner>
          )}
          <p className="muted small" style={{ margin: 0 }}>
            {t("clocks.lengthsHelp")}{" "}
            {looping ? t("clocks.loopCarriesOn") : t("clocks.hourStarts")}
          </p>
        </div>
      )}
    </Card>
  );
}

function SimulateCard({ clockId, dirty }: { clockId: number; dirty: boolean }) {
  const canSimulate = useCan("operator");
  const [report, setReport] = useState<SimulationReport | null>(null);
  const simulate = useAction(
    () => unwrap(api.POST("/api/v1/stations/{station_id}/rotation/simulate", { params: { path: here() }, body: { hours: 1, clock_id: clockId } })),
    { onSuccess: (result) => setReport(result) },
  );
  useEffect(() => setReport(null), [clockId]);
  if (!canSimulate) {
    return null;
  }
  const names = new Map((report?.tracks ?? []).map((track) => [track.id, track]));
  const start = report?.simulation.start_unix_ms ?? 0;
  return (
    <Card
      title={t("clocks.simulation")}
      actions={
        <button
          type="button"
          className="btn small"
          disabled={dirty || simulate.isPending}
          title={dirty ? t("clocks.saveFirst") : undefined}
          onClick={() => simulate.mutate(undefined)}
        >
          {simulate.isPending ? t("clocks.simulating") : t("clocks.simulateHour")}
        </button>
      }
    >
      {!report ? (
        <p className="muted small" style={{ margin: 0 }}>
          {t("clocks.simulateHelp")}
        </p>
      ) : (
        <div className="stack">
          {report.simulation.violations.length > 0 && (
            <Banner kind="bad">{t("clocks.rulesBroken", { count: report.simulation.violations.length })}</Banner>
          )}
          <ol className="list compact">
            {report.simulation.picks.map((pick, index) => {
              const track = names.get(pick.track_id);
              return (
                <li key={index} className="row" style={{ flexWrap: "nowrap" }}>
                  <span className="num muted small">+{duration(pick.start_unix_ms - start)}</span>
                  <span className="ellipsis" style={{ flex: 1 }}>
                    <Link to={`/library/${pick.track_id}`}>
                      {track ? `${track.artist ?? "?"} – ${track.title ?? "?"}` : t("clocks.trackNumber", { id: String(pick.track_id) })}
                    </Link>
                  </span>
                  {pick.explanation.step > 0 && (
                    <span className="badge warn" title={stepLabel(pick.explanation.step)}>
                      {t("clocks.relaxed", { step: pick.explanation.step })}
                    </span>
                  )}
                </li>
              );
            })}
          </ol>
          <p className="muted small" style={{ margin: 0 }}>
            {t("clocks.randomHelp")}
          </p>
        </div>
      )}
    </Card>
  );
}
