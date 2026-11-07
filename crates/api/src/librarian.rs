// SPDX-License-Identifier: AGPL-3.0-or-later

//! Library management through `heron-librarian`: the review of
//! identifications, organizing the library after a dry run, undoing
//! batches, and deleting tracks through the trash.
//!
//! The API cannot write files, so work on files is queued as library tasks
//! (`library_tasks`) that the librarian runs in order; clients poll the
//! task for its result. Keeping a track's own tags only changes the
//! database and is done here.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use heron_core::LibrarySettings;
use heron_core::library::{
    BatchReport, CoversReport, DetailsReport, OrganizePlan, PurgeReport, RelationsReport,
    TRASH_DIR, TaskRequest,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings;
use crate::tracks::{ListContext, TrackFilter, TrackQuery, TrackSummary};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_tasks))
        .routes(routes!(get_task))
        .routes(routes!(plan_organization))
        .routes(routes!(apply_organization))
        .routes(routes!(list_batches))
        .routes(routes!(undo_batch))
        .routes(routes!(todo_counts))
        .routes(routes!(list_review))
        .routes(routes!(apply_release))
        .routes(routes!(keep_tags))
        .routes(routes!(fetch_details))
        .routes(routes!(delete_tracks))
        .routes(routes!(list_trash))
        .routes(routes!(restore_batch))
        .routes(routes!(empty_trash))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Waiting for heron-librarian.
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LibraryTask {
    pub id: u64,
    pub request: TaskRequest,
    pub status: TaskStatus,
    /// Why it failed.
    pub error: Option<String>,
    /// The moves of a finished dry run (`organize_plan`).
    pub plan: Option<OrganizePlan>,
    /// What a finished apply, undo or review did.
    pub report: Option<BatchReport>,
    /// What a finished MusicBrainz relationships lookup found.
    pub relations: Option<RelationsReport>,
    /// What a finished MusicBrainz details lookup did.
    pub details: Option<DetailsReport>,
    /// What emptying the trash removed.
    pub purge: Option<PurgeReport>,
    /// What a finished Cover Art Archive lookup did.
    pub covers: Option<CoversReport>,
    pub requested_by: Option<String>,
    pub created_at_unix_ms: i64,
    pub started_at_unix_ms: Option<i64>,
    pub finished_at_unix_ms: Option<i64>,
}

type TaskRow = (
    u64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<i64>,
    Option<i64>,
);

const TASK_SELECT: &str = "SELECT t.id, CAST(t.params AS CHAR), CAST(t.status AS CHAR),
        t.error, CAST(t.result AS CHAR), u.username,
        CAST(UNIX_TIMESTAMP(t.created_at) * 1000 AS SIGNED),
        CAST(UNIX_TIMESTAMP(t.started_at) * 1000 AS SIGNED),
        CAST(UNIX_TIMESTAMP(t.finished_at) * 1000 AS SIGNED)
    FROM library_tasks t LEFT JOIN users u ON u.id = t.requested_by";

fn task(
    (id, params, status, error, result, requested_by, created, started, finished): TaskRow,
) -> ApiResult<LibraryTask> {
    let request: TaskRequest = serde_json::from_str(&params)
        .map_err(|error| ApiError::Internal(format!("task {id}: {error}")))?;
    let status = match status.as_str() {
        "running" => TaskStatus::Running,
        "done" => TaskStatus::Done,
        "failed" => TaskStatus::Failed,
        _ => TaskStatus::Pending,
    };
    let result = result.as_deref().filter(|_| status == TaskStatus::Done);
    let (mut plan, mut report, mut relations, mut details, mut purge, mut covers) =
        (None, None, None, None, None, None);
    match (&request, result) {
        (_, None) => {}
        (TaskRequest::OrganizePlan, Some(result)) => plan = serde_json::from_str(result).ok(),
        (TaskRequest::ArtistRelations, Some(result)) => {
            relations = serde_json::from_str(result).ok();
        }
        (TaskRequest::MusicbrainzDetails { .. }, Some(result)) => {
            details = serde_json::from_str(result).ok();
        }
        (TaskRequest::EmptyTrash, Some(result)) => purge = serde_json::from_str(result).ok(),
        (TaskRequest::FetchCovers { .. } | TaskRequest::ApplyCover { .. }, Some(result)) => {
            covers = serde_json::from_str(result).ok();
        }
        (_, Some(result)) => report = serde_json::from_str(result).ok(),
    }
    Ok(LibraryTask {
        id,
        request,
        status,
        error,
        plan,
        report,
        relations,
        details,
        purge,
        covers,
        requested_by,
        created_at_unix_ms: created,
        started_at_unix_ms: started,
        finished_at_unix_ms: finished,
    })
}

async fn find_task(state: &AppState, id: u64) -> ApiResult<LibraryTask> {
    let row: Option<TaskRow> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{TASK_SELECT} WHERE t.id = ?")))
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    task(row.ok_or_else(|| ApiError::not_found("task"))?)
}

/// Queues a task for heron-librarian.
pub async fn ask(
    state: &AppState,
    user: &CurrentUser,
    request: TaskRequest,
) -> ApiResult<LibraryTask> {
    let params =
        serde_json::to_string(&request).map_err(|error| ApiError::Internal(error.to_string()))?;
    let id = sqlx::query("INSERT INTO library_tasks (kind, params, requested_by) VALUES (?, ?, ?)")
        .bind(request.kind())
        .bind(params)
        .bind(user.id)
        .execute(&state.pool)
        .await?
        .last_insert_id();
    find_task(state, id).await
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// 1 to 200, 20 by default.
    pub limit: Option<u32>,
}

fn limit(query: &ListQuery) -> ApiResult<u32> {
    let limit = query.limit.unwrap_or(20);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::BadRequest("limit is 1 to 200".to_owned()));
    }
    Ok(limit)
}

/// The most recent library tasks, newest first.
#[utoipa::path(get, path = "/library/tasks", tag = "library",
    params(ListQuery),
    responses((status = 200, body = Vec<LibraryTask>), (status = "4XX", body = ErrorBody)))]
async fn list_tasks(
    _user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<LibraryTask>>> {
    let rows: Vec<TaskRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{TASK_SELECT} ORDER BY t.id DESC LIMIT ?"
    )))
    .bind(limit(&query)?)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(task).collect::<ApiResult<_>>()?))
}

/// A library task, with its result once done.
#[utoipa::path(get, path = "/library/tasks/{id}", tag = "library",
    params(("id" = u64, Path)),
    responses((status = 200, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn get_task(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<LibraryTask>> {
    Ok(Json(find_task(&state, id).await?))
}

/// Asks for a dry run of organizing the library with the layout in the
/// library settings: the task's `plan` lists every move, nothing changes.
/// Admins.
#[utoipa::path(post, path = "/library/organize", tag = "library",
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn plan_organization(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Admin)?;
    let task = ask(&state, &user, TaskRequest::OrganizePlan).await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

/// Carries out the moves of a finished dry run, as one batch that can be
/// undone. Tracks moved since the dry run are skipped. Admins.
#[utoipa::path(post, path = "/library/organize/{plan_task_id}/apply", tag = "library",
    params(("plan_task_id" = u64, Path)),
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn apply_organization(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(plan_task_id): Path<u64>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Admin)?;
    let plan = find_task(&state, plan_task_id).await?;
    if plan.request != TaskRequest::OrganizePlan || plan.status != TaskStatus::Done {
        return Err(ApiError::Conflict(format!(
            "task {plan_task_id} is not a finished dry run"
        )));
    }
    let applied: Option<u64> = sqlx::query_scalar(
        "SELECT id FROM library_tasks
         WHERE kind = 'organize_apply' AND status IN ('pending', 'running', 'done')
           AND JSON_VALUE(params, '$.plan_task_id') = ?
         LIMIT 1",
    )
    .bind(plan_task_id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(applied) = applied {
        return Err(ApiError::Conflict(format!(
            "this dry run is applied already (task {applied})"
        )));
    }
    let task = ask(&state, &user, TaskRequest::OrganizeApply { plan_task_id }).await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BatchKind {
    /// Files filed from the incoming folder.
    Import,
    Organize,
    Undo,
    /// A release picked on the review screen.
    Review,
    /// Tags edited from the browser.
    Edit,
    /// Tracks moved to the trash (undone to restore them).
    Delete,
    /// Tracks removed from the trash for good.
    Purge,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Batch {
    pub id: u64,
    pub kind: BatchKind,
    /// For an undo, the batch it undoes.
    pub undoes: Option<u64>,
    /// The undo of this batch.
    pub undone_by: Option<u64>,
    /// Files moved or re-tagged.
    pub files: u64,
    pub created_at_unix_ms: i64,
    /// `None` while it runs, or when it was interrupted.
    pub finished_at_unix_ms: Option<i64>,
}

/// The most recent batches of file operations, newest first.
#[utoipa::path(get, path = "/library/batches", tag = "library",
    params(ListQuery),
    responses((status = 200, body = Vec<Batch>), (status = "4XX", body = ErrorBody)))]
async fn list_batches(
    _user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<Batch>>> {
    type BatchRow = (u64, String, Option<u64>, Option<u64>, i64, i64, Option<i64>);
    let rows: Vec<BatchRow> = sqlx::query_as(
        "SELECT b.id, CAST(b.kind AS CHAR), b.undoes,
                (SELECT u.id FROM library_batches u WHERE u.undoes = b.id LIMIT 1),
                CAST((SELECT COUNT(*) FROM library_moves m
                      WHERE m.batch_id = b.id AND m.status IN ('done', 'undone')) AS SIGNED),
                CAST(UNIX_TIMESTAMP(b.created_at) * 1000 AS SIGNED),
                CAST(UNIX_TIMESTAMP(b.finished_at) * 1000 AS SIGNED)
         FROM library_batches b ORDER BY b.id DESC LIMIT ?",
    )
    .bind(limit(&query)?)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, kind, undoes, undone_by, files, created, finished)| Batch {
                    id,
                    kind: match kind.as_str() {
                        "organize" => BatchKind::Organize,
                        "undo" => BatchKind::Undo,
                        "review" => BatchKind::Review,
                        "edit" => BatchKind::Edit,
                        "delete" => BatchKind::Delete,
                        "purge" => BatchKind::Purge,
                        _ => BatchKind::Import,
                    },
                    undoes,
                    undone_by,
                    files: u64::try_from(files).unwrap_or(0),
                    created_at_unix_ms: created,
                    finished_at_unix_ms: finished,
                },
            )
            .collect(),
    ))
}

/// Reverts a batch: files go back to their previous paths and get their
/// previous tags back. Nothing is overwritten: a file whose previous path
/// is taken stays where it is (see the task's report). Admins.
#[utoipa::path(post, path = "/library/batches/{id}/undo", tag = "library",
    params(("id" = u64, Path)),
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn undo_batch(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(batch_id): Path<u64>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Admin)?;
    let batch: Option<(String, Option<u64>)> = sqlx::query_as(
        "SELECT CAST(b.kind AS CHAR),
                (SELECT u.id FROM library_batches u WHERE u.undoes = b.id AND u.finished_at IS NOT NULL LIMIT 1)
         FROM library_batches b WHERE b.id = ?",
    )
    .bind(batch_id)
    .fetch_optional(&state.pool)
    .await?;
    match batch {
        None => return Err(ApiError::not_found("batch")),
        Some((kind, _)) if kind == "undo" => {
            return Err(ApiError::Conflict(
                "an undo cannot be undone: organize again instead".to_owned(),
            ));
        }
        Some((kind, _)) if kind == "purge" => {
            return Err(ApiError::Conflict(
                "purged tracks are gone for good".to_owned(),
            ));
        }
        Some((_, Some(by))) => {
            return Err(ApiError::Conflict(format!(
                "batch {batch_id} was undone by batch {by}"
            )));
        }
        Some(_) => {}
    }
    let task = ask(&state, &user, TaskRequest::Undo { batch_id }).await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

/// A release a track may be on, from AcoustID.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ReviewCandidate {
    pub score: f64,
    pub recording_id: String,
    pub release_id: String,
    pub release_title: Option<String>,
    pub release_group_id: Option<String>,
    pub release_group_title: Option<String>,
    /// `YYYY-MM-DD`, `YYYY-MM` or `YYYY`.
    pub date: Option<String>,
    pub country: Option<String>,
    pub disc: Option<u32>,
    pub track: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewItem {
    pub track: TrackSummary,
    /// Why the identification was not applied.
    pub reason: Option<String>,
    pub score: Option<f64>,
    /// Best first.
    pub candidates: Vec<ReviewCandidate>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewPage {
    pub items: Vec<ReviewItem>,
    pub total: u64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReviewQuery {
    /// 1 to 200, 50 by default.
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// What there is to do on the library: the counts of the *To do* page.
#[derive(Debug, Serialize, ToSchema)]
pub struct TodoCounts {
    /// Tracks waiting for review (`GET /library/review`'s `total`).
    pub to_identify: u64,
    /// Active songs (`GET /tracks?active_songs=true`) with no intro point.
    pub no_intro: u64,
    /// Active songs with no hook.
    pub no_hook: u64,
    /// Active songs with no cover; `null` while cover art is turned off.
    pub no_cover: Option<u64>,
}

/// How many tracks to identify, and active songs missing an intro, a hook
/// or a cover: the counts of the *To do* page, whose lists are
/// `GET /library/review` and `GET /tracks?active_songs=true&filter=…`.
#[utoipa::path(get, path = "/library/todo", tag = "library",
    responses((status = 200, body = TodoCounts), (status = "4XX", body = ErrorBody)))]
async fn todo_counts(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<TodoCounts>> {
    let context = ListContext::load(&state.pool).await?;
    let songs = |filter| TrackQuery {
        active_songs: Some(true),
        filter: Some(filter),
        ..TrackQuery::default()
    };
    let no_cover = if crate::modules::installation(&state.pool).await?.cover_art {
        Some(
            context
                .count(&state.pool, &songs(TrackFilter::NoCover))
                .await?,
        )
    } else {
        None
    };
    Ok(Json(TodoCounts {
        to_identify: needs_review(&state.pool).await?,
        no_intro: context
            .count(&state.pool, &songs(TrackFilter::NoIntro))
            .await?,
        no_hook: context
            .count(&state.pool, &songs(TrackFilter::NoHook))
            .await?,
        no_cover,
    }))
}

/// How many tracks wait for review.
async fn needs_review(pool: &sqlx::MySqlPool) -> ApiResult<u64> {
    let total: i64 = sqlx::query_scalar(
        "SELECT CAST(COUNT(*) AS SIGNED) FROM track_identifications WHERE status = 'needs_review'",
    )
    .fetch_one(pool)
    .await?;
    Ok(u64::try_from(total).unwrap_or(0))
}

/// Tracks whose identification was not confident enough to apply, with the
/// releases they may be on.
#[utoipa::path(get, path = "/library/review", tag = "library",
    params(ReviewQuery),
    responses((status = 200, body = ReviewPage), (status = "4XX", body = ErrorBody)))]
async fn list_review(
    _user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<ReviewQuery>,
) -> ApiResult<Json<ReviewPage>> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::BadRequest("limit is 1 to 200".to_owned()));
    }
    let total = needs_review(&state.pool).await?;
    let ids: Vec<u64> = sqlx::query_scalar(
        "SELECT t.id FROM tracks t
         JOIN track_identifications i ON i.track_id = t.id AND i.status = 'needs_review'
         ORDER BY t.path LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(query.offset.unwrap_or(0))
    .fetch_all(&state.pool)
    .await?;
    let mut tracks = ListContext::load(&state.pool)
        .await?
        .summaries(&state.pool, &ids)
        .await?;
    tracks.sort_by(|a, b| a.path.cmp(&b.path));
    let mut items = Vec::with_capacity(tracks.len());
    for track in tracks {
        let (reason, score, candidates): (Option<String>, Option<f64>, Option<String>) =
            sqlx::query_as(
                "SELECT reason, score, CAST(candidates AS CHAR)
                 FROM track_identifications WHERE track_id = ?",
            )
            .bind(track.id)
            .fetch_one(&state.pool)
            .await?;
        items.push(ReviewItem {
            track,
            reason,
            score,
            candidates: candidates
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default(),
        });
    }
    Ok(Json(ReviewPage { items, total }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseChoice {
    pub release_id: String,
    pub recording_id: String,
}

/// Applies one of the track's candidate releases: heron-librarian writes
/// its tags into the file and files it where the layout says, as a batch
/// that can be undone. Operators and admins.
#[utoipa::path(post, path = "/tracks/{id}/review/apply", tag = "library",
    params(("id" = u64, Path)), request_body = ReleaseChoice,
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn apply_release(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(track_id): Path<u64>,
    Json(choice): Json<ReleaseChoice>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Operator)?;
    let candidates = review_candidates(&state, track_id).await?;
    let Some(candidate) = candidates.into_iter().find(|candidate| {
        candidate.release_id == choice.release_id && candidate.recording_id == choice.recording_id
    }) else {
        return Err(ApiError::Invalid(
            "pick one of the track's candidate releases".to_owned(),
        ));
    };
    let pending: Option<u64> = sqlx::query_scalar(
        "SELECT id FROM library_tasks
         WHERE kind = 'review_apply' AND status IN ('pending', 'running')
           AND JSON_VALUE(params, '$.track_id') = ?
         LIMIT 1",
    )
    .bind(track_id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(pending) = pending {
        return Err(ApiError::Conflict(format!(
            "a release is being applied to this track already (task {pending})"
        )));
    }
    let request = TaskRequest::ReviewApply {
        track_id,
        release_id: candidate.release_id,
        recording_id: candidate.recording_id,
        disc: candidate.disc,
        track: candidate.track,
    };
    let task = ask(&state, &user, request).await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

/// The candidates of a track that needs review.
async fn review_candidates(state: &AppState, track_id: u64) -> ApiResult<Vec<ReviewCandidate>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT CAST(status AS CHAR), CAST(candidates AS CHAR)
         FROM track_identifications WHERE track_id = ?",
    )
    .bind(track_id)
    .fetch_optional(&state.pool)
    .await?;
    match row {
        Some((status, candidates)) if status == "needs_review" => Ok(candidates
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()),
        _ => Err(ApiError::Conflict(
            "the track does not need review".to_owned(),
        )),
    }
}

/// Marks the track as reviewed: its file keeps its own tags. Operators and
/// admins.
#[utoipa::path(post, path = "/tracks/{id}/review/keep", tag = "library",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn keep_tags(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(track_id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let updated = sqlx::query(
        "UPDATE track_identifications SET status = 'kept'
         WHERE track_id = ? AND status = 'needs_review'",
    )
    .bind(track_id)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(ApiError::Conflict(
            "the track does not need review".to_owned(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DetailsRequest {
    /// Look up every identified track again, not only those never looked up.
    pub all: bool,
}

/// Fetches the programming details of identified tracks from MusicBrainz
/// (album, record label, ISRC, first release year, genres), one release a
/// second. From then on, new tracks are looked up in the background.
/// Operators and admins.
#[utoipa::path(post, path = "/library/musicbrainz-details", tag = "library",
    request_body = DetailsRequest,
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn fetch_details(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<DetailsRequest>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Operator)?;
    let task = ask(
        &state,
        &user,
        TaskRequest::MusicbrainzDetails { all: request.all },
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TrackSelection {
    pub track_ids: Vec<u64>,
}

/// Tracks deleted in one request.
const MAX_DELETE: usize = 10_000;

/// Moves tracks to the trash: heron-librarian moves their files to
/// `.heron-trash/` in the library, as one batch. They leave the library,
/// rotation, their categories and the queue (a track on air plays to its
/// end), and are purged after `trash_days`; until then they can be
/// restored whole. Tracks whose file is missing can be deleted too.
/// Operators and admins.
#[utoipa::path(post, path = "/tracks/delete", tag = "library",
    request_body = TrackSelection,
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn delete_tracks(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(selection): Json<TrackSelection>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Operator)?;
    let mut track_ids = selection.track_ids;
    track_ids.sort_unstable();
    track_ids.dedup();
    if track_ids.is_empty() || track_ids.len() > MAX_DELETE {
        return Err(ApiError::Invalid(format!(
            "select 1 to {MAX_DELETE} tracks"
        )));
    }
    let task = ask(&state, &user, TaskRequest::Delete { track_ids }).await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrashItem {
    pub track: TrackSummary,
    /// Where it was in the library, and goes back to when restored.
    pub original_path: String,
    /// The `delete` batch: restoring undoes it.
    pub batch_id: Option<u64>,
    pub deleted_at_unix_ms: i64,
    /// When it is purged for good.
    pub purge_at_unix_ms: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrashPage {
    pub items: Vec<TrashItem>,
    pub total: u64,
    /// Days deleted tracks are kept.
    pub trash_days: u32,
}

/// Tracks in the trash, most recently deleted first.
#[utoipa::path(get, path = "/library/trash", tag = "library",
    params(ReviewQuery),
    responses((status = 200, body = TrashPage), (status = "4XX", body = ErrorBody)))]
async fn list_trash(
    _user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<ReviewQuery>,
) -> ApiResult<Json<TrashPage>> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::BadRequest("limit is 1 to 200".to_owned()));
    }
    let settings: LibrarySettings =
        settings::load(&state.pool, LibrarySettings::SETTINGS_NAME).await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT CAST(COUNT(*) AS SIGNED) FROM tracks WHERE deleted_at IS NOT NULL",
    )
    .fetch_one(&state.pool)
    .await?;
    let ids: Vec<u64> = sqlx::query_scalar(
        "SELECT id FROM tracks WHERE deleted_at IS NOT NULL
         ORDER BY deleted_at DESC, id LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(query.offset.unwrap_or(0))
    .fetch_all(&state.pool)
    .await?;
    let mut tracks = ListContext::load(&state.pool)
        .await?
        .summaries(&state.pool, &ids)
        .await?;
    tracks.sort_by_key(|track| ids.iter().position(|id| *id == track.id));
    let mut items = Vec::with_capacity(tracks.len());
    for track in tracks {
        let deleted: i64 = sqlx::query_scalar(
            "SELECT CAST(UNIX_TIMESTAMP(deleted_at) * 1000 AS SIGNED) FROM tracks WHERE id = ?",
        )
        .bind(track.id)
        .fetch_one(&state.pool)
        .await?;
        let (batch_id, original_path) = trash_origin(&track.path);
        items.push(TrashItem {
            original_path,
            batch_id,
            deleted_at_unix_ms: deleted,
            purge_at_unix_ms: deleted + i64::from(settings.trash_days) * 86_400_000,
            track,
        });
    }
    Ok(Json(TrashPage {
        items,
        total: u64::try_from(total).unwrap_or(0),
        trash_days: settings.trash_days,
    }))
}

/// The batch and the original path of a path in the trash
/// (`.heron-trash/<batch>/<path>`).
fn trash_origin(path: &str) -> (Option<u64>, String) {
    let Some(rest) = path
        .strip_prefix(TRASH_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return (None, path.to_owned());
    };
    match rest.split_once('/') {
        Some((batch, original)) => (batch.parse().ok(), original.to_owned()),
        None => (None, rest.to_owned()),
    }
}

/// Restores the tracks of a deletion (undoes its batch): their files go
/// back where they were, with their analysis, categories and history. A
/// path taken since is reported, and that track stays in the trash.
/// Operators and admins.
#[utoipa::path(post, path = "/library/trash/{batch_id}/restore", tag = "library",
    params(("batch_id" = u64, Path)),
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn restore_batch(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(batch_id): Path<u64>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Operator)?;
    let batch: Option<(String, Option<u64>)> = sqlx::query_as(
        "SELECT CAST(b.kind AS CHAR),
                (SELECT u.id FROM library_batches u WHERE u.undoes = b.id AND u.finished_at IS NOT NULL LIMIT 1)
         FROM library_batches b WHERE b.id = ?",
    )
    .bind(batch_id)
    .fetch_optional(&state.pool)
    .await?;
    match batch {
        None => return Err(ApiError::not_found("batch")),
        Some((kind, _)) if kind != "delete" => {
            return Err(ApiError::Conflict(format!(
                "batch {batch_id} is not a deletion"
            )));
        }
        Some((_, Some(by))) => {
            return Err(ApiError::Conflict(format!(
                "batch {batch_id} was restored by batch {by}"
            )));
        }
        Some(_) => {}
    }
    let pending: Option<u64> = sqlx::query_scalar(
        "SELECT id FROM library_tasks
         WHERE kind = 'undo' AND status IN ('pending', 'running')
           AND JSON_VALUE(params, '$.batch_id') = ?
         LIMIT 1",
    )
    .bind(batch_id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(pending) = pending {
        return Err(ApiError::Conflict(format!(
            "these tracks are being restored already (task {pending})"
        )));
    }
    let task = ask(&state, &user, TaskRequest::Undo { batch_id }).await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

/// Purges the whole trash now: the tracks and their files are removed for
/// good; their plays keep their artist and title. Admins.
#[utoipa::path(post, path = "/library/trash/empty", tag = "library",
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn empty_trash(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Admin)?;
    let task = ask(&state, &user, TaskRequest::EmptyTrash).await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_paths_tell_their_batch_and_origin() {
        assert_eq!(
            trash_origin(".heron-trash/12/Band/Album/01 Song.mp3"),
            (Some(12), "Band/Album/01 Song.mp3".to_owned())
        );
        assert_eq!(trash_origin("Band/x.mp3"), (None, "Band/x.mp3".to_owned()));
    }
}
