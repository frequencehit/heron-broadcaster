// SPDX-License-Identifier: AGPL-3.0-or-later

// Library tasks: work the API queues for heron-librarian, polled until done.

import { useQuery } from "@tanstack/react-query";
import { type LibraryTask, api, unwrap } from "../api";
import { t } from "../i18n";
import { ago } from "../lib/format";

/** A library task, polled until heron-librarian finishes it. */
export function useTask(id: number | null) {
  return useQuery({
    queryKey: ["library-task", id],
    enabled: id !== null,
    queryFn: () => unwrap(api.GET("/api/v1/library/tasks/{id}", { params: { path: { id: id ?? 0 } } })),
    refetchInterval: (query) => {
      const status = query.state.data?.status;
      return status === "done" || status === "failed" ? false : 1000;
    },
  });
}

export function TaskState({ task }: Readonly<{ task: LibraryTask | undefined }>) {
  if (!task) {
    return null;
  }
  switch (task.status) {
    case "pending":
      return (
        <span className="row small muted">
          <span className="spinner" /> {t("review.waiting")}
        </span>
      );
    case "running":
      return (
        <span className="row small muted">
          <span className="spinner" /> {t("review.working")}
        </span>
      );
    case "failed":
      return <span className="small bad">{t("review.failed", { error: task.error ?? "" })}</span>;
    case "done":
      return <span className="small good">{t("review.doneAgo", { ago: ago(task.finished_at_unix_ms) })}</span>;
  }
}

