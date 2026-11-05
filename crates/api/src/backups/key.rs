// SPDX-License-Identifier: AGPL-3.0-or-later

//! The backup key (docs/backups.md#encryption): the public key backups are
//! encrypted with, the earlier ones, and each admin's checks. Neither is in
//! the configuration file, nor changed by a restore.

use age::x25519::Recipient;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::crypto;
use super::{BackupKey, Kind};
use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};

/// Days between an admin's key checks, one step further after each check
/// that passes; the last step repeats.
const CHECK_STEPS: [u32; 5] = [1, 3, 7, 14, 30];

/// The `backup_encryption` settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Encryption {
    /// The key backups are encrypted with; none: in clear.
    pub current: Option<KeyRecord>,
    /// A key shown or given, until it is typed back.
    pub pending: Option<KeyRecord>,
    /// The keys used before, oldest first.
    pub previous: Vec<EarlierKey>,
    /// When encryption last changed (a key set, encryption turned off):
    /// backups are named from then on, so that each says which key it has.
    pub changed_unix_ms: i64,
}

/// The start of the next second: backups are named to the second, so a
/// change takes effect on a second of its own.
fn next_second(unix_ms: i64) -> i64 {
    (unix_ms.div_euclid(1000) + 1) * 1000
}

/// When a change of encryption takes effect: after now, and after the
/// newest backup, whose name may be ahead of the clock (each backup has a
/// second of its own).
fn change_time(state: &AppState) -> ApiResult<i64> {
    let newest = state
        .backups
        .list()?
        .first()
        .map_or(0, |backup| backup.created_at_unix_ms + 1000);
    Ok(next_second(Timestamp::now().as_millisecond()).max(newest))
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct KeyRecord {
    /// The public key, `age1…`.
    pub recipient: String,
    pub since_unix_ms: i64,
    /// Who set it.
    pub set_by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct EarlierKey {
    pub recipient: String,
    pub since_unix_ms: i64,
    pub until_unix_ms: i64,
    pub set_by: String,
}

impl Encryption {
    pub const SETTINGS_NAME: &str = "backup_encryption";

    pub async fn load(pool: &MySqlPool) -> ApiResult<Self> {
        crate::settings::load(pool, Self::SETTINGS_NAME).await
    }

    async fn save(&self, pool: &MySqlPool) -> ApiResult<()> {
        crate::settings::save(pool, Self::SETTINGS_NAME, self).await
    }

    /// The public key new backups are encrypted with.
    pub fn recipient(&self) -> ApiResult<Option<Recipient>> {
        self.current
            .as_ref()
            .map(|key| crypto::parse_recipient(&key.recipient))
            .transpose()
    }

    /// Which key opens a backup taken at `created_at_unix_ms`, and its
    /// public key: the first key still in use after that time. A backup in
    /// clear then is encrypted with the next key set.
    pub fn key_of(&self, created_at_unix_ms: i64, kind: Kind) -> (BackupKey, Option<String>) {
        if kind == Kind::Uploaded {
            return (BackupKey::Unknown, None);
        }
        match self
            .previous
            .iter()
            .find(|key| key.until_unix_ms > created_at_unix_ms)
        {
            Some(key) => (BackupKey::Earlier, Some(key.recipient.clone())),
            None => match &self.current {
                Some(key) => (BackupKey::Current, Some(key.recipient.clone())),
                None => (BackupKey::Unknown, None),
            },
        }
    }

    fn state(&self) -> KeyState {
        match (&self.current, &self.pending) {
            (_, Some(_)) => KeyState::Pending,
            (Some(_), None) => KeyState::On,
            (None, None) => KeyState::Off,
        }
    }

    fn view(&self) -> KeyView {
        KeyView {
            state: self.state(),
            current: self.current.clone(),
            pending_recipient: self.pending.as_ref().map(|key| key.recipient.clone()),
            previous: self.previous.clone(),
        }
    }
}

/// The last backup tests (`backup_tests`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct Tests {
    pub last: Option<TestRecord>,
    /// The last one that passed.
    pub last_ok: Option<TestRecord>,
}

/// A backup read back to its end, nothing restored.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct TestRecord {
    pub name: String,
    pub at_unix_ms: i64,
    /// Who gave the key; none: Heron, for a backup in clear.
    pub by: Option<String>,
    pub ok: bool,
    /// What failed, and in which file.
    pub error: Option<String>,
}

impl Tests {
    pub const SETTINGS_NAME: &str = "backup_tests";

    pub async fn load(pool: &MySqlPool) -> ApiResult<Self> {
        crate::settings::load(pool, Self::SETTINGS_NAME).await
    }

    pub async fn record(pool: &MySqlPool, test: TestRecord) -> ApiResult<()> {
        let mut tests = Self::load(pool).await?;
        if test.ok {
            tests.last_ok = Some(test.clone());
        }
        tests.last = Some(test);
        crate::settings::save(pool, Self::SETTINGS_NAME, &tests).await
    }

    /// Whether the last test failed, or none passed for 30 days.
    pub fn due(&self, now_unix_ms: i64) -> bool {
        const DAYS_30: i64 = 30 * 24 * 3600 * 1000;
        self.last.as_ref().is_some_and(|test| !test.ok)
            || self
                .last_ok
                .as_ref()
                .is_none_or(|test| now_unix_ms - test.at_unix_ms > DAYS_30)
    }
}

/// The next interval after a check that passed.
fn next_interval(current: Option<u32>) -> u32 {
    current
        .and_then(|days| CHECK_STEPS.iter().copied().find(|step| *step > days))
        .unwrap_or(match current {
            None => CHECK_STEPS[0],
            Some(_) => CHECK_STEPS[CHECK_STEPS.len() - 1],
        })
}

/// Whether an admin's key check is due.
pub async fn check_due(pool: &MySqlPool, user_id: u64) -> ApiResult<bool> {
    let due: Option<bool> = sqlx::query_scalar(
        "SELECT (backup_key_checked_at IS NULL
                 OR backup_key_checked_at + INTERVAL COALESCE(backup_key_interval_days, 1) DAY <= NOW(6))
            AND (backup_key_later_until IS NULL OR backup_key_later_until <= NOW(6))
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(due.unwrap_or(false))
}

/// A check that passed: the next one further away. When it is due.
async fn record_check(pool: &MySqlPool, user_id: u64) -> ApiResult<i64> {
    let current: Option<Option<u32>> =
        sqlx::query_scalar("SELECT backup_key_interval_days FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    let days = next_interval(current.flatten());
    sqlx::query(
        "UPDATE users SET backup_key_checked_at = NOW(6), backup_key_interval_days = ?,
             backup_key_later_until = NULL
         WHERE id = ?",
    )
    .bind(days)
    .bind(user_id)
    .execute(pool)
    .await?;
    let next: i64 = sqlx::query_scalar(
        "SELECT CAST(UNIX_TIMESTAMP(backup_key_checked_at + INTERVAL backup_key_interval_days DAY) * 1000 AS SIGNED)
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(next)
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_key, new_key, drop_key))
        .routes(routes!(confirm_key))
        .routes(routes!(check_key))
        .routes(routes!(later))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    /// Backups are written in clear.
    Off,
    /// A key waits to be typed back.
    Pending,
    On,
}

#[derive(Serialize, ToSchema)]
pub struct KeyView {
    pub state: KeyState,
    /// The key backups are encrypted with.
    pub current: Option<KeyRecord>,
    /// The public key waiting to be typed back.
    pub pending_recipient: Option<String>,
    /// The keys used before, oldest first.
    pub previous: Vec<EarlierKey>,
}

/// The backup key: its public key, when and by whom it was set, the
/// earlier ones. Never the secret key. Admins only.
#[utoipa::path(get, path = "/backups/key", tag = "backups",
    responses((status = 200, body = KeyView), (status = "4XX", body = ErrorBody)))]
async fn get_key(user: CurrentUser, State(state): State<AppState>) -> ApiResult<Json<KeyView>> {
    user.require(Role::Admin)?;
    Ok(Json(Encryption::load(&state.pool).await?.view()))
}

#[derive(Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewKeyRequest {
    /// The admin's own public key, `age1…`; left out: Heron generates one.
    #[serde(default)]
    pub recipient: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct NewKey {
    /// The secret key, `AGE-SECRET-KEY-1…`: shown this once, never stored.
    /// Missing for the admin's own key.
    pub key: Option<String>,
    /// `heron-backup-key.txt`, an age identity file, with the secret key.
    pub key_file: Option<String>,
    pub recipient: String,
}

/// A new key, waiting to be typed back (`/backups/key/confirm`): generated
/// and returned this once, or the admin's own public key. Admins only.
#[utoipa::path(post, path = "/backups/key", tag = "backups", request_body = NewKeyRequest,
    responses((status = 201, body = NewKey), (status = "4XX", body = ErrorBody)))]
async fn new_key(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<NewKeyRequest>,
) -> ApiResult<(StatusCode, Json<NewKey>)> {
    user.require(Role::Admin)?;
    let now = Timestamp::now();
    let (key, recipient) = match request.recipient {
        Some(recipient) => {
            let recipient = crypto::parse_recipient(&recipient)?.to_string();
            (None, recipient)
        }
        None => {
            let (secret, recipient) = crypto::generate();
            (Some(secret), recipient)
        }
    };
    let mut encryption = Encryption::load(&state.pool).await?;
    if encryption
        .current
        .as_ref()
        .is_some_and(|current| current.recipient == recipient)
    {
        return Err(ApiError::Invalid(
            "backups are already encrypted with this key".to_owned(),
        ));
    }
    encryption.pending = Some(KeyRecord {
        recipient: recipient.clone(),
        since_unix_ms: now.as_millisecond(),
        set_by: user.username.clone(),
    });
    encryption.save(&state.pool).await?;
    tracing::info!(user = %user.username, %recipient, "backup key waiting to be typed back");
    let key_file = key
        .as_deref()
        .map(|secret| crypto::key_file(secret, &recipient, &now.to_string()));
    Ok((
        StatusCode::CREATED,
        Json(NewKey {
            key,
            key_file,
            recipient,
        }),
    ))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyRequest {
    /// The secret key, `AGE-SECRET-KEY-1…`, or the contents of its file.
    pub key: String,
}

#[derive(Serialize, ToSchema)]
pub struct Confirmed {
    pub key: KeyView,
    /// Backups that were in clear, now encrypted.
    pub encrypted: Vec<String>,
    /// Backups left in clear: their files fail their checksums, or could
    /// not be written.
    pub left_in_clear: Vec<String>,
    /// The backup taken with the new key.
    pub backup: Option<String>,
}

/// Turns encryption on with the key waiting, once typed back: the backups
/// in clear are encrypted, a backup is taken and tested. Admins only.
#[utoipa::path(post, path = "/backups/key/confirm", tag = "backups", request_body = KeyRequest,
    responses((status = 200, body = Confirmed), (status = "4XX", body = ErrorBody)))]
async fn confirm_key(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<KeyRequest>,
) -> ApiResult<Json<Confirmed>> {
    user.require(Role::Admin)?;
    let identity = crypto::parse_identity(&request.key)?;
    let mut encryption = Encryption::load(&state.pool).await?;
    let Some(pending) = encryption.pending.take() else {
        return Err(ApiError::Conflict(
            "no key waits to be typed back: set one first".to_owned(),
        ));
    };
    if crypto::recipient_of(&identity) != pending.recipient {
        return Err(ApiError::Invalid(
            "this is not the key just set: check it and type it again".to_owned(),
        ));
    }
    let now = change_time(&state)?;
    encryption.changed_unix_ms = now;
    if let Some(current) = encryption.current.take() {
        encryption.previous.push(EarlierKey {
            recipient: current.recipient,
            since_unix_ms: current.since_unix_ms,
            until_unix_ms: now,
            set_by: current.set_by,
        });
    }
    let recipient = crypto::parse_recipient(&pending.recipient)?;
    encryption.current = Some(KeyRecord {
        since_unix_ms: now,
        set_by: user.username.clone(),
        ..pending
    });
    encryption.save(&state.pool).await?;
    // Every other admin checks the new key; this one just did.
    sqlx::query(
        "UPDATE users SET backup_key_checked_at = NULL, backup_key_interval_days = NULL,
             backup_key_later_until = NULL
         WHERE id <> ?",
    )
    .bind(user.id)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE users SET backup_key_checked_at = NOW(6), backup_key_interval_days = ?,
             backup_key_later_until = NULL
         WHERE id = ?",
    )
    .bind(CHECK_STEPS[0])
    .bind(user.id)
    .execute(&state.pool)
    .await?;
    tracing::info!(user = %user.username, recipient = %encryption.view().current.map(|key| key.recipient).unwrap_or_default(), "backups are encrypted with a new key");

    let (encrypted, left_in_clear) = state.backups.encrypt_existing(&recipient).await?;
    let backup = match state.backups.create(&state.pool, Kind::Manual).await {
        Ok(backup) => Some(backup.name),
        Err(error) => {
            tracing::error!(%error, "cannot back up with the new key");
            None
        }
    };
    if let Some(name) = &backup {
        spawn_test(&state, name.clone(), identity, &user.username);
    }
    Ok(Json(Confirmed {
        key: encryption.view(),
        encrypted,
        left_in_clear,
        backup,
    }))
}

/// Tests a backup in the background, with the key just given.
fn spawn_test(state: &AppState, name: String, identity: age::x25519::Identity, by: &str) {
    if let Err(error) = state.backups.spawn_test(
        &state.pool,
        name.clone(),
        Some(identity),
        Some(by.to_owned()),
    ) {
        tracing::warn!(backup = %name, %error, "the backup is not tested");
    }
}

/// Drops the key waiting to be typed back; with none, turns encryption
/// off: new backups are written in clear. Admins only.
#[utoipa::path(delete, path = "/backups/key", tag = "backups",
    responses((status = 200, body = KeyView), (status = "4XX", body = ErrorBody)))]
async fn drop_key(user: CurrentUser, State(state): State<AppState>) -> ApiResult<Json<KeyView>> {
    user.require(Role::Admin)?;
    let mut encryption = Encryption::load(&state.pool).await?;
    if encryption.pending.take().is_some() {
        tracing::info!(user = %user.username, "the backup key waiting was dropped");
    } else if let Some(current) = encryption.current.take() {
        let now = change_time(&state)?;
        encryption.changed_unix_ms = now;
        encryption.previous.push(EarlierKey {
            recipient: current.recipient,
            since_unix_ms: current.since_unix_ms,
            until_unix_ms: now,
            set_by: current.set_by,
        });
        tracing::info!(user = %user.username, "backup encryption turned off");
    }
    encryption.save(&state.pool).await?;
    Ok(Json(encryption.view()))
}

#[derive(Serialize, ToSchema)]
pub struct Checked {
    /// When this admin's next check is due.
    pub next_check_unix_ms: i64,
    /// The backup being tested with the key.
    pub testing: Option<String>,
}

/// Checks the key an admin types back: their next check further away, and
/// the newest backup encrypted with it tested in the background. A wrong
/// key changes nothing. Admins only.
#[utoipa::path(post, path = "/backups/key/check", tag = "backups", request_body = KeyRequest,
    responses((status = 200, body = Checked), (status = "4XX", body = ErrorBody)))]
async fn check_key(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<KeyRequest>,
) -> ApiResult<Json<Checked>> {
    user.require(Role::Admin)?;
    let encryption = Encryption::load(&state.pool).await?;
    let Some(current) = &encryption.current else {
        return Err(ApiError::Conflict(
            "backups are not encrypted: there is no key to check".to_owned(),
        ));
    };
    let identity = crypto::parse_identity(&request.key)?;
    // Public keys: no secret to compare in constant time.
    if crypto::recipient_of(&identity) != current.recipient {
        tracing::info!(user = %user.username, "wrong backup key typed");
        return Err(ApiError::Invalid("this is not the backup key".to_owned()));
    }
    let next_check_unix_ms = record_check(&state.pool, user.id).await?;
    tracing::info!(user = %user.username, "backup key checked");
    let newest = state
        .backups
        .views(&state.pool)
        .await?
        .into_iter()
        .find(|backup| backup.key == Some(BackupKey::Current))
        .map(|backup| backup.name);
    if let Some(name) = &newest {
        spawn_test(&state, name.clone(), identity, &user.username);
    }
    Ok(Json(Checked {
        next_check_unix_ms,
        testing: newest,
    }))
}

/// Asks this admin for the key again the next day. Admins only.
#[utoipa::path(post, path = "/backups/key/later", tag = "backups",
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn later(user: CurrentUser, State(state): State<AppState>) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    sqlx::query("UPDATE users SET backup_key_later_until = NOW(6) + INTERVAL 1 DAY WHERE id = ?")
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_takes_effect_on_the_next_second() {
        assert_eq!(next_second(1_000), 2_000);
        assert_eq!(next_second(1_999), 2_000);
        assert_eq!(next_second(0), 1_000);
    }

    #[test]
    fn checks_move_further_away_then_every_30_days() {
        let mut days = None;
        let mut steps = Vec::new();
        for _ in 0..7 {
            let next = next_interval(days);
            steps.push(next);
            days = Some(next);
        }
        assert_eq!(steps, [1, 3, 7, 14, 30, 30, 30]);
        // An interval from before a change of steps lands on the next one.
        assert_eq!(next_interval(Some(5)), 7);
        assert_eq!(next_interval(Some(90)), 30);
    }

    fn key(recipient: &str, since: i64) -> KeyRecord {
        KeyRecord {
            recipient: recipient.to_owned(),
            since_unix_ms: since,
            set_by: "admin".to_owned(),
        }
    }

    #[test]
    fn each_backup_names_the_key_that_opens_it() {
        let mut encryption = Encryption::default();
        assert_eq!(
            encryption.key_of(10, Kind::Daily),
            (BackupKey::Unknown, None)
        );
        encryption.current = Some(key("age1b", 300));
        encryption.previous.push(EarlierKey {
            recipient: "age1a".to_owned(),
            since_unix_ms: 100,
            until_unix_ms: 200,
            set_by: "admin".to_owned(),
        });
        let earlier = (BackupKey::Earlier, Some("age1a".to_owned()));
        let current = (BackupKey::Current, Some("age1b".to_owned()));
        // In clear before the first key, then encrypted with it.
        assert_eq!(encryption.key_of(50, Kind::Daily), earlier);
        assert_eq!(encryption.key_of(150, Kind::Manual), earlier);
        // In clear while encryption was off, then encrypted with the next.
        assert_eq!(encryption.key_of(250, Kind::Daily), current);
        assert_eq!(encryption.key_of(350, Kind::BeforeRestore), current);
        assert_eq!(
            encryption.key_of(350, Kind::Uploaded),
            (BackupKey::Unknown, None)
        );
        assert_eq!(encryption.state(), KeyState::On);
        encryption.pending = Some(key("age1c", 400));
        assert_eq!(encryption.state(), KeyState::Pending);
    }

    fn test(ok: bool, at: i64) -> TestRecord {
        TestRecord {
            name: "x".to_owned(),
            at_unix_ms: at,
            by: None,
            ok,
            error: None,
        }
    }

    #[test]
    fn a_test_is_due_when_the_last_failed_or_none_passed_for_30_days() {
        let day = 24 * 3600 * 1000;
        let mut tests = Tests::default();
        assert!(tests.due(0));
        tests.last = Some(test(true, 0));
        tests.last_ok = Some(test(true, 0));
        assert!(!tests.due(30 * day));
        assert!(tests.due(31 * day));
        tests.last = Some(test(false, day));
        assert!(tests.due(2 * day));
    }
}
