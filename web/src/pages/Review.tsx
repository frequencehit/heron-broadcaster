// SPDX-License-Identifier: AGPL-3.0-or-later

// Library management through heron-librarian: identifications to review,
// organizing the library after a dry run, and undoing batches.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { type Schemas, api, done, unwrap } from "../api";
import { TaskState, useTask } from "../components/Task";
import {
  Banner,
  Card,
  ConfirmButton,
  Empty,
  ErrorView,
  Icon,
  Loading,
  PageHead,
  useToast,
} from "../components/ui";
import { type PlainKey, number, rich, t } from "../i18n";
import { dateTime, duration, trackName } from "../lib/format";
import { Link, setSearch, useLocation } from "../lib/router";
import { useAction, useCan } from "../lib/session";

type Tab = "review" | "organize" | "batches";

export function Review() {
  const { search } = useLocation();
  const tab = (search.get("tab") ?? "review") as Tab;
  return (
    <>
      <PageHead
        title={t("nav.review")}
        description={t("review.description")}
      />
      <nav className="tabs" aria-label={t("review.sections")}>
        {(
          [
            ["review", "review.tab.identifications"],
            ["organize", "review.tab.organize"],
            ["batches", "review.tab.changes"],
          ] as const satisfies readonly (readonly [Tab, PlainKey])[]
        ).map(([value, label]) => (
          <button
            key={value}
            type="button"
            aria-selected={tab === value}
            onClick={() => setSearch({ tab: value === "review" ? undefined : value })}
          >
            {t(label)}
          </button>
        ))}
      </nav>
      {tab === "review" && <Identifications />}
      {tab === "organize" && <Organize />}
      {tab === "batches" && <Batches />}
    </>
  );
}

// Identifications -----------------------------------------------------------

function Identifications() {
  const review = useQuery({
    queryKey: ["review"],
    queryFn: () => unwrap(api.GET("/api/v1/library/review", { params: { query: { limit: 50 } } })),
  });
  if (review.isPending) {
    return <Loading />;
  }
  if (review.isError) {
    return <ErrorView error={review.error} />;
  }
  if (review.data.items.length === 0) {
    return (
      <Card>
        <Empty>
          {rich("review.nothing", { buffer: <Link to="/import-music">{t("review.nothingBuffer")}</Link> })}
        </Empty>
      </Card>
    );
  }
  return (
    <div className="stack">
      <p className="muted" style={{ margin: 0 }}>
        {t("review.intro", { count: review.data.total })}
      </p>
      {review.data.items.map((item) => (
        <ReviewItem key={item.track.id} item={item} />
      ))}
    </div>
  );
}

function ReviewItem({ item }: { item: Schemas["ReviewItem"] }) {
  const canEdit = useCan("operator");
  const client = useQueryClient();
  const toast = useToast();
  const [taskId, setTaskId] = useState<number | null>(null);
  const task = useTask(taskId);
  const apply = useAction(
    (candidate: Schemas["ReviewCandidate"]) =>
      unwrap(
        api.POST("/api/v1/tracks/{id}/review/apply", {
          params: { path: { id: item.track.id } },
          body: { release_id: candidate.release_id, recording_id: candidate.recording_id },
        }),
      ),
    { onSuccess: (queued) => setTaskId(queued.id) },
  );
  const keep = useAction(
    () => done(api.POST("/api/v1/tracks/{id}/review/keep", { params: { path: { id: item.track.id } } })),
    { success: t("review.kept"), invalidate: [["review"], ["review-count"]] },
  );

  useEffect(() => {
    if (task.data?.status === "done") {
      toast(t("review.applied", { track: trackName(item.track) }));
      void client.invalidateQueries({ queryKey: ["review"] });
      void client.invalidateQueries({ queryKey: ["review-count"] });
      void client.invalidateQueries({ queryKey: ["library-batches"] });
    } else if (task.data?.status === "failed") {
      toast(task.data.error ?? t("review.applyFailed"), true);
    }
  }, [task.data?.status, task.data?.error, client, toast, item.track]);

  const busy = taskId !== null && task.data?.status !== "failed";
  return (
    <Card
      title={
        <div style={{ minWidth: 0 }}>
          <h2 className="ellipsis">
            <Link to={`/library/${item.track.id}`}>{trackName(item.track)}</Link>
          </h2>
          <div className="muted small ellipsis">
            <code>{item.track.path}</code> · {duration(item.track.duration_ms)}
          </div>
        </div>
      }
      actions={
        canEdit && (
          <button type="button" className="btn small" disabled={busy} onClick={() => keep.mutate(undefined)}>
            <Icon name="check" size={14} /> {t("review.keep")}
          </button>
        )
      }
      flush
    >
      <div className="card-body row between">
        <span className="warn small">{item.reason ?? t("library.filter.needsReview")}</span>
        <TaskState task={task.data} />
      </div>
      {item.candidates.length === 0 ? (
        <Empty>{t("review.noCandidate")}</Empty>
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>{t("review.release")}</th>
                <th className="hide-narrow">{t("review.date")}</th>
                <th className="hide-narrow">{t("review.country")}</th>
                <th>{t("history.track")}</th>
                <th>{t("review.score")}</th>
                <th>
                  <span className="visually-hidden">{t("library.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {item.candidates.map((candidate) => (
                <tr key={`${candidate.release_id}-${candidate.recording_id}-${candidate.disc}-${candidate.track}`}>
                  <td className="title-cell">
                    <a
                      href={`https://musicbrainz.org/release/${candidate.release_id}`}
                      target="_blank"
                      rel="noreferrer"
                    >
                      {candidate.release_title ?? candidate.release_group_title ?? candidate.release_id}
                    </a>
                  </td>
                  <td className="hide-narrow">{candidate.date ?? "–"}</td>
                  <td className="hide-narrow">{candidate.country ?? "–"}</td>
                  <td className="num">
                    {candidate.disc && candidate.disc > 1 ? `${candidate.disc}-` : ""}
                    {candidate.track ?? "–"}
                  </td>
                  <td className="num">{number(candidate.score, { style: "percent" })}</td>
                  <td className="actions">
                    {canEdit && (
                      <button
                        type="button"
                        className="btn small primary"
                        disabled={busy || apply.isPending}
                        onClick={() => apply.mutate(candidate)}
                      >
                        {t("review.apply")}
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}

// Organize ------------------------------------------------------------------

function Organize() {
  const isAdmin = useCan("admin");
  const client = useQueryClient();
  const settings = useQuery({
    queryKey: ["settings", "library"],
    queryFn: () => unwrap(api.GET("/api/v1/settings/library")),
  });
  const recent = useQuery({
    queryKey: ["library-tasks"],
    queryFn: () => unwrap(api.GET("/api/v1/library/tasks", { params: { query: { limit: 50 } } })),
  });
  const [planId, setPlanId] = useState<number | null>(null);
  const [applyId, setApplyId] = useState<number | null>(null);

  // Pick up the last dry run (and its apply) after a reload.
  useEffect(() => {
    if (planId !== null || !recent.data) {
      return;
    }
    const plan = recent.data.find((task) => task.request.kind === "organize_plan");
    if (plan) {
      setPlanId(plan.id);
      const applied = recent.data.find(
        (task) => task.request.kind === "organize_apply" && task.request.plan_task_id === plan.id,
      );
      if (applied) {
        setApplyId(applied.id);
      }
    }
  }, [recent.data, planId]);

  const plan = useTask(planId);
  const applied = useTask(applyId);
  const dryRun = useAction(() => unwrap(api.POST("/api/v1/library/organize")), {
    onSuccess: (task) => {
      setPlanId(task.id);
      setApplyId(null);
    },
  });
  const apply = useAction(
    (id: number) =>
      unwrap(api.POST("/api/v1/library/organize/{plan_task_id}/apply", { params: { path: { plan_task_id: id } } })),
    { onSuccess: (task) => setApplyId(task.id) },
  );
  useEffect(() => {
    if (applied.data?.status === "done") {
      void client.invalidateQueries({ queryKey: ["library-batches"] });
      void client.invalidateQueries({ queryKey: ["tracks"] });
    }
  }, [applied.data?.status, client]);

  const moves = plan.data?.plan?.moves ?? [];
  const [shown, setShown] = useState(100);

  return (
    <div className="stack">
      <Card
        title={t("review.organizeTitle")}
        actions={
          isAdmin && (
            <button
              type="button"
              className="btn primary"
              disabled={dryRun.isPending || plan.data?.status === "pending" || plan.data?.status === "running"}
              onClick={() => dryRun.mutate(undefined)}
            >
              {plan.data ? t("review.newDryRun") : t("review.dryRun")}
            </button>
          )
        }
      >
        <div className="stack">
          <p style={{ margin: 0 }}>
            {rich("review.organizeHelp", {
              layout: <code>{settings.data?.layout ?? "…"}</code>,
              link: <Link to="/settings">{t("review.changeIt")}</Link>,
            })}
          </p>
          <p className="muted small" style={{ margin: 0 }}>
            {t("review.dryRunHelp")}
          </p>
          {!isAdmin && <p className="muted small">{t("review.adminsOnly")}</p>}
        </div>
      </Card>

      {plan.data && (
        <Card
          title={t("review.dryRunAt", { date: dateTime(plan.data.created_at_unix_ms) })}
          actions={
            <>
              <TaskState task={plan.data} />
              {isAdmin && plan.data.status === "done" && moves.length > 0 && !applyId && (
                <ConfirmButton
                  className="btn primary"
                  confirm={t("review.moveFiles", { count: moves.length })}
                  question={<p>{t("review.moveQuestion", { count: moves.length })}</p>}
                  onConfirm={() => apply.mutate(plan.data?.id ?? 0)}
                >
                  {t("review.applyMoves", { count: moves.length })}
                </ConfirmButton>
              )}
            </>
          }
          flush
        >
          {applied.data && (
            <div className="card-body stack">
              <div className="row between">
                <strong>{t("review.applying")}</strong>
                <TaskState task={applied.data} />
              </div>
              {applied.data.report && (
                <BatchReport report={applied.data.report} />
              )}
            </div>
          )}
          {plan.data.plan && (
            <>
              <div className="card-body">
                {t("review.moves", { count: moves.length })}, {t("review.inPlace", { count: plan.data.plan.unchanged })}
                {plan.data.plan.unreadable.length > 0 &&
                  `, ${t("review.unreadable", { count: plan.data.plan.unreadable.length })}`}
                {t("library.end")}
              </div>
              {moves.length > 0 && (
                <div className="table-wrap">
                  <table className="table">
                    <thead>
                      <tr>
                        <th>{t("review.from")}</th>
                        <th>{t("review.to")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {moves.slice(0, shown).map((move) => (
                        <tr key={move.track_id}>
                          <td className="mono small" style={{ overflowWrap: "anywhere" }}>
                            {move.from}
                          </td>
                          <td className="mono small" style={{ overflowWrap: "anywhere" }}>
                            {move.to}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              {moves.length > shown && (
                <div className="pager">
                  <span className="muted small">{t("queue.more", { count: moves.length - shown })}</span>
                  <button type="button" className="btn small" onClick={() => setShown((n) => n + 500)}>
                    {t("review.showMore")}
                  </button>
                </div>
              )}
            </>
          )}
        </Card>
      )}
    </div>
  );
}

function BatchReport({ report }: { report: Schemas["BatchReport"] }) {
  return (
    <div className="stack">
      <span>
        {t("review.batchReport", { batch: String(report.batch_id), count: report.done })}
        {report.skipped.length > 0 && `, ${t("review.skipped", { count: report.skipped.length })}`}
        {t("library.end")}
      </span>
      {report.skipped.length > 0 && (
        <ul className="small" style={{ margin: 0 }}>
          {report.skipped.slice(0, 50).map((skipped) => (
            <li key={skipped.path}>
              <code>{skipped.path}</code>: {skipped.reason}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// Batches -------------------------------------------------------------------

const KINDS: Record<string, PlainKey> = {
  import: "review.kind.import",
  organize: "review.kind.organize",
  undo: "review.kind.undo",
  review: "review.kind.review",
  edit: "review.kind.edit",
  delete: "review.kind.delete",
  purge: "review.kind.purge",
};

function kindLabel(kind: string): string {
  const key = KINDS[kind];
  return key ? t(key) : kind;
}

function Batches() {
  const isAdmin = useCan("admin");
  const client = useQueryClient();
  const toast = useToast();
  const batches = useQuery({
    queryKey: ["library-batches"],
    queryFn: () => unwrap(api.GET("/api/v1/library/batches", { params: { query: { limit: 50 } } })),
  });
  const [undoId, setUndoId] = useState<number | null>(null);
  const undoing = useTask(undoId);
  const undo = useAction(
    (id: number) => unwrap(api.POST("/api/v1/library/batches/{id}/undo", { params: { path: { id } } })),
    { onSuccess: (task) => setUndoId(task.id) },
  );
  useEffect(() => {
    const status = undoing.data?.status;
    if (status === "done") {
      const report = undoing.data?.report;
      toast(
        report
          ? report.skipped.length
            ? t("review.undoneSkipped", { count: report.done, skipped: report.skipped.length })
            : t("review.undoneCount", { count: report.done })
          : t("review.undone"),
      );
      void client.invalidateQueries({ queryKey: ["library-batches"] });
      void client.invalidateQueries({ queryKey: ["review"] });
    } else if (status === "failed") {
      toast(undoing.data?.error ?? t("review.undoFailed"), true);
    }
  }, [undoing.data?.status, undoing.data?.report, undoing.data?.error, client, toast]);

  if (batches.isPending) {
    return <Loading />;
  }
  if (batches.isError) {
    return <ErrorView error={batches.error} />;
  }
  const busy = undoId !== null && undoing.data?.status !== "done" && undoing.data?.status !== "failed";
  return (
    <div className="stack">
      {busy && (
        <Banner kind="warn">
          {t("review.undoing")} <TaskState task={undoing.data} />
        </Banner>
      )}
      <Card flush>
        {batches.data.length === 0 ? (
          <Empty>{t("review.noChange")}</Empty>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>{t("review.batch")}</th>
                <th>{t("review.when")}</th>
                <th>{t("review.files")}</th>
                <th>{t("review.state")}</th>
                <th>
                  <span className="visually-hidden">{t("library.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {batches.data.map((batch) => (
                <tr key={batch.id}>
                  <td>
                    #{batch.id} {kindLabel(batch.kind)}
                    {batch.undoes && <span className="muted"> {t("review.of", { batch: String(batch.undoes) })}</span>}
                  </td>
                  <td>{dateTime(batch.created_at_unix_ms)}</td>
                  <td className="num">{number(batch.files)}</td>
                  <td>
                    {batch.undone_by ? (
                      <span className="badge">{t("review.undoneBy", { batch: String(batch.undone_by) })}</span>
                    ) : batch.finished_at_unix_ms ? (
                      <span className="badge good">{t("review.done")}</span>
                    ) : (
                      <span className="badge warn">{t("review.unfinished")}</span>
                    )}
                  </td>
                  <td className="actions">
                    {isAdmin && batch.kind !== "undo" && batch.kind !== "purge" && !batch.undone_by && batch.files > 0 && (
                      <ConfirmButton
                        className="btn small"
                        confirm={t("review.kind.undo")}
                        disabled={busy}
                        question={<p>{t("review.undoQuestion", { count: batch.files, batch: String(batch.id) })}</p>}
                        onConfirm={() => undo.mutate(batch.id)}
                      >
                        <Icon name="undo" size={14} /> {t("review.kind.undo")}
                      </ConfirmButton>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
    </div>
  );
}
