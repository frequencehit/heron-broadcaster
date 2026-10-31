// SPDX-License-Identifier: AGPL-3.0-or-later

// The queue: reorder by dragging, add tracks from the library, autofill.

import { useQuery } from "@tanstack/react-query";
import { api, done, unwrap } from "../api";
import { QueueList, useWaiting } from "../components/QueueList";
import { TrackSearch } from "../components/TrackSearch";
import { Card, ConfirmButton, Icon, PageHead } from "../components/ui";
import { t } from "../i18n";
import { duration } from "../lib/format";
import { useAction, useCan } from "../lib/session";
import { here } from "../lib/station";

export function QueuePage() {
  const waiting = useWaiting();
  const canEdit = useCan("operator");
  const settings = useQuery({
    queryKey: ["settings", "queue"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/queue", { params: { path: here() } })),
  });

  const enqueue = useAction(
    ({ id, next }: { id: number; next: boolean }) =>
      unwrap(
        api.POST("/api/v1/stations/{station_id}/queue", { params: { path: here() },
          body: next ? { track_ids: [id], position: 0 } : { track_ids: [id] },
        }),
      ),
    { success: t("queuePage.queued") },
  );
  const clear = useAction(() => done(api.DELETE("/api/v1/stations/{station_id}/queue", { params: { path: here() } })), { success: t("queuePage.cleared") });
  const setAutofill = useAction(
    (autofill: boolean) =>
      unwrap(
        api.PUT("/api/v1/stations/{station_id}/settings/queue", { params: { path: here() },
          body: { autofill, min_queued: settings.data?.min_queued ?? 3 },
        }),
      ),
    { invalidate: [["settings", "queue"]] },
  );

  const total =
    waiting?.reduce((sum, entry) => sum + Math.max(0, entry.track.cue_out_ms - entry.track.cue_in_ms), 0) ??
    0;

  return (
    <>
      <PageHead
        title={t("nav.queue")}
        description={
          waiting
            ? t("queuePage.description", { count: waiting.length, duration: duration(total) })
            : undefined
        }
        actions={
          canEdit && (
            <>
              {settings.data && (
                <label className="check" title={t("queuePage.autofillHelp")}>
                  <input
                    type="checkbox"
                    checked={settings.data.autofill}
                    disabled={setAutofill.isPending}
                    onChange={(event) => setAutofill.mutate(event.target.checked)}
                  />
                  {t("queuePage.autofill", { min: settings.data.min_queued })}
                </label>
              )}
              <ConfirmButton
                confirm={t("queuePage.clearConfirm")}
                question={
                  <p>{settings.data?.autofill ? t("queuePage.clearQuestionAutofill") : t("queuePage.clearQuestion")}</p>
                }
                onConfirm={() => clear.mutate(undefined)}
                disabled={!waiting?.length}
              >
                <Icon name="trash" /> {t("queuePage.clear")}
              </ConfirmButton>
            </>
          )
        }
      />
      {settings.data && !settings.data.autofill && (
        <div className="banner warn" role="status">
          <Icon name="warn" /> {t("queuePage.autofillOff")}
        </div>
      )}
      <div className="grid sidebar-right">
        <Card flush>
          <QueueList />
        </Card>
        {canEdit && (
          <Card title={t("queuePage.add")}>
            <TrackSearch
              actions={(track) => (
                <>
                  <button
                    type="button"
                    className="btn small"
                    title={t("queue.playNext")}
                    onClick={() => enqueue.mutate({ id: track.id, next: true })}
                  >
                    <Icon name="next" size={14} /> {t("dashboard.next")}
                  </button>
                  <button
                    type="button"
                    className="btn small"
                    title={t("queuePage.addEnd")}
                    onClick={() => enqueue.mutate({ id: track.id, next: false })}
                  >
                    <Icon name="plus" size={14} />
                    <span className="visually-hidden">{t("queuePage.addEnd")}</span>
                  </button>
                </>
              )}
            />
          </Card>
        )}
      </div>
    </>
  );
}
