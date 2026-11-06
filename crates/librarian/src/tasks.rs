// SPDX-License-Identifier: AGPL-3.0-or-later

//! Work asked through the API (`library_tasks`), which cannot write files:
//! organize dry runs and applies, undos, releases picked on the review
//! screen, MusicBrainz relationships between artists, tag edits,
//! MusicBrainz programming details, imports from the buffer zone,
//! deletions and emptying the trash. Tasks run one at a time, in order;
//! their results are stored as JSON for the API to show.
//!
//! Between tasks, new identified tracks get their MusicBrainz details,
//! albums without a cover are looked up when the station asked for it, and
//! once an hour the trash is purged of tracks deleted more than
//! `trash_days` ago.

use std::time::{Duration, Instant};

use heron_core::Module;
use heron_core::library::{OrganizePlan, TaskRequest};
use sqlx::MySqlPool;
use tokio::sync::watch;

use crate::identify::{Endpoints, Identifier};
use crate::journal::Roots;
use crate::layout::Layout;
use crate::review::{self, Choice};
use crate::{covers, edit, import, medley, musicbrainz, organize, relations, staging, trash};

/// Wait between looks at the table when there is nothing to do.
const POLL: Duration = Duration::from_secs(1);
/// How often the trash is purged of expired tracks.
const PURGE_INTERVAL: Duration = Duration::from_secs(3600);

/// Wait between background MusicBrainz details passes.
const DETAILS_INTERVAL: Duration = Duration::from_secs(60);

pub struct TaskRunner {
    pool: MySqlPool,
    roots: Roots,
    endpoints: Endpoints,
}

impl TaskRunner {
    pub fn new(pool: MySqlPool, roots: Roots, endpoints: Endpoints) -> Self {
        Self {
            pool,
            roots,
            endpoints,
        }
    }

    /// Runs tasks until `stopped` turns true. A task that is running then
    /// finishes first.
    pub async fn run(self, mut stopped: watch::Receiver<bool>) {
        // A task running when the service stopped was cut short: undos are
        // resumed by asking again, a partial organization is undone or
        // finished like one interrupted on the command line.
        if let Err(error) = sqlx::query(
            "UPDATE library_tasks SET status = 'failed', finished_at = NOW(6),
                 error = 'interrupted: heron-librarian stopped while it ran'
             WHERE status = 'running'",
        )
        .execute(&self.pool)
        .await
        {
            tracing::warn!(%error, "cannot close interrupted library tasks");
        }
        let mut details_due = Instant::now();
        let mut last_purge: Option<Instant> = None;
        while !*stopped.borrow() {
            if last_purge.is_none_or(|at| at.elapsed() >= PURGE_INTERVAL) {
                last_purge = Some(Instant::now());
                self.purge_expired().await;
            }
            match self.run_next().await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "cannot read library tasks"),
            }
            if Instant::now() >= details_due {
                details_due = Instant::now() + DETAILS_INTERVAL;
                if let Err(error) = self.new_details().await {
                    tracing::warn!(%error, "cannot fetch MusicBrainz details");
                }
                if let Err(error) = self.new_covers().await {
                    tracing::warn!(%error, "cannot look covers up");
                }
            }
            tokio::select! {
                () = tokio::time::sleep(POLL) => {}
                _ = stopped.changed() => {}
            }
        }
    }

    /// Purges the tracks in the trash for more than `trash_days`.
    pub async fn purge_expired(&self) {
        let days = match import::load_settings(&self.pool).await {
            Ok(settings) => settings.trash_days,
            Err(error) => {
                tracing::warn!(%error, "cannot read the library settings");
                return;
            }
        };
        if let Err(error) = trash::purge(&self.pool, &self.roots, Some(days)).await {
            tracing::warn!(%error, "cannot purge the trash");
        }
    }

    /// Runs the oldest pending task. Returns whether there was one.
    pub async fn run_next(&self) -> Result<bool, sqlx::Error> {
        let next: Option<(u64, String)> = sqlx::query_as(
            "SELECT id, CAST(params AS CHAR) FROM library_tasks
             WHERE status = 'pending' ORDER BY id LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, params)) = next else {
            return Ok(false);
        };
        let claimed = sqlx::query(
            "UPDATE library_tasks SET status = 'running', started_at = NOW(6)
             WHERE id = ? AND status = 'pending'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if claimed == 0 {
            return Ok(true);
        }
        let outcome = match serde_json::from_str::<TaskRequest>(&params) {
            Ok(request) => {
                tracing::info!(id, kind = request.kind(), "running a library task");
                self.execute(id, &request).await
            }
            Err(error) => Err(format!("invalid task: {error}")),
        };
        match &outcome {
            Ok(_) => tracing::info!(id, "library task done"),
            Err(error) => tracing::warn!(id, error, "library task failed"),
        }
        let (status, result, error) = match outcome {
            Ok(result) => ("done", Some(result.to_string()), None),
            Err(error) => ("failed", None, Some(error)),
        };
        sqlx::query(
            "UPDATE library_tasks SET status = ?, result = ?, error = ?, finished_at = NOW(6)
             WHERE id = ?",
        )
        .bind(status)
        .bind(result)
        .bind(error)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    async fn execute(&self, id: u64, request: &TaskRequest) -> Result<serde_json::Value, String> {
        let settings = import::load_settings(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        match request {
            TaskRequest::OrganizePlan => {
                let layout = Layout::from_settings(&settings);
                json(organize::plan(&self.pool, &self.roots, &layout).await)
            }
            TaskRequest::OrganizeApply { plan_task_id } => {
                let plan = self.plan(id, *plan_task_id).await?;
                json(organize::apply(&self.pool, &self.roots, &plan).await)
            }
            TaskRequest::Undo { batch_id } => {
                json(organize::undo(&self.pool, &self.roots, *batch_id).await)
            }
            TaskRequest::ReviewApply {
                track_id,
                release_id,
                recording_id,
                disc,
                track,
            } => {
                // Looking a release up needs no AcoustID key.
                let key = settings.acoustid_key.clone().unwrap_or_default();
                let identifier = Identifier::new(self.endpoints.clone(), key, settings.min_score)
                    .map_err(|error| error.to_string())?;
                let choice = Choice {
                    track_id: *track_id,
                    release_id: release_id.clone(),
                    recording_id: recording_id.clone(),
                    disc: *disc,
                    track: *track,
                };
                json(review::apply(&self.pool, &self.roots, &identifier, &settings, &choice).await)
            }
            TaskRequest::ArtistRelations => {
                json(relations::look_up(&self.pool, &self.musicbrainz()?).await)
            }
            // Asked before `write_tags` was turned off: the files are left alone.
            TaskRequest::EditTags { .. } if !settings.write_tags => Err(
                "tags are kept in Heron's database (the library setting write_tags is off): \
                 the files were not changed"
                    .to_owned(),
            ),
            TaskRequest::EditTags {
                track_ids,
                set,
                from_details,
            } => json(edit::apply(&self.pool, &self.roots, track_ids, set, from_details).await),
            TaskRequest::MusicbrainzDetails { all } => {
                json(musicbrainz::look_up(&self.pool, &self.musicbrainz()?, *all, None).await)
            }
            TaskRequest::StagingImport { file_ids } => json(
                staging::import(
                    &self.pool,
                    &self.roots,
                    &self.endpoints,
                    &settings,
                    file_ids,
                )
                .await,
            ),
            TaskRequest::Delete { track_ids } => {
                json(trash::delete(&self.pool, &self.roots, track_ids).await)
            }
            TaskRequest::EmptyTrash => json(trash::purge(&self.pool, &self.roots, None).await),
            TaskRequest::PreviewMedley { medley_id, parts } => {
                self.module_on(Module::Medleys).await?;
                let report = medley::preview(&self.pool, &self.roots, *medley_id, parts).await?;
                serde_json::to_value(report).map_err(|error| error.to_string())
            }
            TaskRequest::FetchCovers {
                track_ids,
                candidates,
            } => {
                self.module_on(Module::CoverArt).await?;
                let albums = if track_ids.is_empty() {
                    covers::Albums::AllMissing
                } else {
                    covers::Albums::Of(track_ids)
                };
                json(covers::look_up(&self.pool, &self.musicbrainz()?, albums, *candidates).await)
            }
            TaskRequest::ApplyCover {
                candidate_id,
                track_id,
            } => {
                self.module_on(Module::CoverArt).await?;
                json(
                    covers::apply(&self.pool, &self.musicbrainz()?, *candidate_id, *track_id).await,
                )
            }
            TaskRequest::RenderMedley { medley_id } => {
                self.module_on(Module::Medleys).await?;
                let report = medley::render(&self.pool, &self.roots, *medley_id).await?;
                serde_json::to_value(report).map_err(|error| error.to_string())
            }
        }
    }

    /// Fails a task of a module turned off for the installation, queued
    /// before it was (`docs/modules.md`).
    async fn module_on(&self, module: Module) -> Result<(), String> {
        let modules = import::load_modules(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        if modules.is_on(module) {
            Ok(())
        } else {
            Err(format!("the {} module is turned off", module.name()))
        }
    }

    /// A MusicBrainz client (MusicBrainz needs no AcoustID key).
    fn musicbrainz(&self) -> Result<Identifier, String> {
        Identifier::new(self.endpoints.clone(), String::new(), 1.0)
            .map_err(|error| error.to_string())
    }

    /// Fetches the MusicBrainz details of tracks that have none yet, once
    /// the station asked for them.
    async fn new_details(&self) -> Result<(), String> {
        if !musicbrainz::background_enabled(&self.pool)
            .await
            .map_err(|error| error.to_string())?
        {
            return Ok(());
        }
        musicbrainz::look_up(
            &self.pool,
            &self.musicbrainz()?,
            false,
            Some(musicbrainz::BACKGROUND_RELEASES),
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    /// Looks up albums without a cover never looked up, when the library
    /// setting `fetch_covers` and cover art are on.
    async fn new_covers(&self) -> Result<(), String> {
        let settings = import::load_settings(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        if !settings.fetch_covers {
            return Ok(());
        }
        let modules = import::load_modules(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        if !modules.cover_art {
            return Ok(());
        }
        covers::look_up(
            &self.pool,
            &self.musicbrainz()?,
            covers::Albums::NeverLookedUp(covers::BACKGROUND_ALBUMS),
            false,
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    /// The plan of a finished dry run, not applied by another task.
    async fn plan(&self, id: u64, plan_task_id: u64) -> Result<OrganizePlan, String> {
        let row: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT CAST(kind AS CHAR), CAST(status AS CHAR), CAST(result AS CHAR)
             FROM library_tasks WHERE id = ?",
        )
        .bind(plan_task_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        let Some((_, _, Some(result))) =
            row.filter(|(kind, status, _)| kind == "organize_plan" && status == "done")
        else {
            return Err(format!("task {plan_task_id} is not a finished dry run"));
        };
        // A failed apply can be run again: moves done since are skipped.
        let applied: Option<u64> = sqlx::query_scalar(
            "SELECT id FROM library_tasks
             WHERE kind = 'organize_apply' AND status = 'done' AND id <> ?
               AND JSON_VALUE(params, '$.plan_task_id') = ?
             LIMIT 1",
        )
        .bind(id)
        .bind(plan_task_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        if let Some(applied) = applied {
            return Err(format!("this dry run was applied already (task {applied})"));
        }
        serde_json::from_str(&result).map_err(|error| format!("invalid dry run: {error}"))
    }
}

/// A task's result as stored, or its error for people.
fn json<T: serde::Serialize, E: std::fmt::Display>(
    result: Result<T, E>,
) -> Result<serde_json::Value, String> {
    let value = result.map_err(|error| error.to_string())?;
    serde_json::to_value(value).map_err(|error| error.to_string())
}
