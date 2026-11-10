// SPDX-License-Identifier: AGPL-3.0-or-later

//! Signing in and out, users and API tokens.
//!
//! The first admin is created on the command line:
//! `heron-api user add <name> --role admin` (see `docs/api.md`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{
    self, Credential, CurrentUser, Passwords, Role, check_username, create_api_token,
    create_session, is_https, session_cookie,
};
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate};
use crate::status_bar::StatusBarSettings;
use crate::two_factor::login::{PENDING_COOKIE, PENDING_PATH};
use crate::two_factor::{
    PENDING_SIGN_IN, SecondFactor, TwoFactorSettings, TwoFactorStatus, highest_role,
};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(login))
        .routes(routes!(logout))
        .routes(routes!(me))
        .routes(routes!(change_password))
        .routes(routes!(set_preferences))
        .routes(routes!(list_users, create_user))
        .routes(routes!(update_user, delete_user))
        .routes(routes!(list_tokens, create_token))
        .routes(routes!(revoke_token))
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct User {
    pub id: u64,
    pub username: String,
    pub role: Role,
    pub enabled: bool,
    pub theme: Theme,
    /// The web UI's language: a language tag (`en`, `fr`…), or `system` to
    /// follow the browser's languages.
    pub language: String,
    pub width: Width,
    /// The On air page shows the whole queue, editable there, instead of
    /// its first tracks.
    pub on_air_whole_queue: bool,
    /// The status bar's settings, the defaults filled in.
    pub status_bar: StatusBarSettings,
    pub last_login_at_unix_ms: Option<i64>,
    pub created_at_unix_ms: i64,
    /// Their role on each station; a station left out is `none`. `role` is
    /// the shared role (the library, what stations share); a shared admin
    /// is admin on every station.
    pub station_roles: Vec<StationRole>,
    /// Their second factors, and whether their role requires one.
    pub two_factor: TwoFactorStatus,
}

/// A role on a station; `none`: the user does not see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StationRoleName {
    None,
    Viewer,
    Operator,
    Admin,
}

impl StationRoleName {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Admin => "admin",
        }
    }

    fn parse(role: &str) -> Self {
        match role {
            "viewer" => Self::Viewer,
            "operator" => Self::Operator,
            "admin" => Self::Admin,
            _ => Self::None,
        }
    }
}

impl From<Role> for StationRoleName {
    fn from(role: Role) -> Self {
        match role {
            Role::Viewer => Self::Viewer,
            Role::Operator => Self::Operator,
            Role::Admin => Self::Admin,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StationRole {
    pub station_id: u64,
    pub role: StationRoleName,
}

/// The users' roles on stations, and their second factors.
async fn attach_roles(pool: &MySqlPool, users: &mut [User]) -> ApiResult<()> {
    let rows: Vec<(u64, u64, String)> = sqlx::query_as(
        "SELECT user_id, station_id, CAST(role AS CHAR) FROM user_station_roles
         WHERE role <> 'none' ORDER BY user_id, station_id",
    )
    .fetch_all(pool)
    .await?;
    for user in users.iter_mut() {
        user.station_roles = rows
            .iter()
            .filter(|(user_id, ..)| *user_id == user.id)
            .map(|(_, station_id, role)| StationRole {
                station_id: *station_id,
                role: StationRoleName::parse(role),
            })
            .collect();
    }
    let required: TwoFactorSettings =
        crate::settings::load(pool, TwoFactorSettings::SETTINGS_NAME).await?;
    let factors = crate::two_factor::statuses(pool).await?;
    for user in users {
        let (totp, passkeys) = factors.get(&user.id).copied().unwrap_or_default();
        let station_roles: Vec<Role> = rows
            .iter()
            .filter(|(user_id, ..)| *user_id == user.id)
            .filter_map(|(_, _, role)| Role::parse(role))
            .collect();
        user.two_factor = TwoFactorStatus {
            totp,
            passkeys,
            required: required.requires(highest_role(user.role, &station_roles)),
        };
    }
    Ok(())
}

/// Replaces a user's roles on stations: those left out are `none`.
async fn write_roles(pool: &MySqlPool, user_id: u64, roles: &[StationRole]) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM user_station_roles WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for role in roles
        .iter()
        .filter(|role| role.role != StationRoleName::None)
    {
        sqlx::query(
            "INSERT INTO user_station_roles (user_id, station_id, role) VALUES (?, ?, ?)
             ON DUPLICATE KEY UPDATE role = VALUES(role)",
        )
        .bind(user_id)
        .bind(role.station_id)
        .bind(role.role.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if crate::error::is_still_used(&error) {
                ApiError::Invalid(format!("no station {}", role.station_id))
            } else {
                error.into()
            }
        })?;
    }
    tx.commit().await?;
    Ok(())
}

/// The web UI's theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follows the browser's setting (`prefers-color-scheme`).
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn parse(theme: &str) -> Self {
        match theme {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }
}

/// The web UI's width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Width {
    /// A centred column.
    #[default]
    Normal,
    /// 80 % of the window, centred.
    Wide,
    /// The whole window.
    Full,
}

impl Width {
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Wide => "wide",
            Self::Full => "full",
        }
    }

    fn parse(width: &str) -> Self {
        match width {
            "wide" => Self::Wide,
            "full" => Self::Full,
            _ => Self::Normal,
        }
    }
}

type UserRow = (
    u64,
    String,
    String,
    bool,
    String,
    String,
    String,
    bool,
    Option<String>,
    Option<i64>,
    i64,
);

const USER_COLUMNS: &str =
    "id, username, CAST(role AS CHAR), enabled, CAST(theme AS CHAR), language, CAST(width AS CHAR), on_air_whole_queue, status_bar,
    CAST(UNIX_TIMESTAMP(last_login_at) * 1000 AS SIGNED),
    CAST(UNIX_TIMESTAMP(created_at) * 1000 AS SIGNED)";

fn user(
    (
        id,
        username,
        role,
        enabled,
        theme,
        language,
        width,
        on_air_whole_queue,
        status_bar,
        last_login,
        created,
    ): UserRow,
) -> User {
    User {
        id,
        username,
        role: Role::parse(&role).unwrap_or(Role::Viewer),
        enabled,
        theme: Theme::parse(&theme),
        language,
        width: Width::parse(&width),
        on_air_whole_queue,
        status_bar: StatusBarSettings::parse(status_bar.as_deref()),
        last_login_at_unix_ms: last_login,
        created_at_unix_ms: created,
        station_roles: Vec::new(),
        two_factor: TwoFactorStatus::default(),
    }
}

async fn find_user(pool: &MySqlPool, id: u64) -> ApiResult<User> {
    let row: Option<UserRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {USER_COLUMNS} FROM users WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let mut users: Vec<User> = row.map(user).into_iter().collect();
    attach_roles(pool, &mut users).await?;
    users.pop().ok_or_else(|| ApiError::not_found("user"))
}

pub async fn all_users(pool: &MySqlPool) -> ApiResult<Vec<User>> {
    let rows: Vec<UserRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {USER_COLUMNS} FROM users ORDER BY username"
    )))
    .fetch_all(pool)
    .await?;
    let mut users: Vec<User> = rows.into_iter().map(user).collect();
    attach_roles(pool, &mut users).await?;
    Ok(users)
}

/// Creates a user with their shared role on every station; used by the API
/// and the command line.
pub async fn add_user(
    pool: &MySqlPool,
    passwords: &Passwords,
    username: &str,
    password: &str,
    role: Role,
) -> ApiResult<User> {
    check_username(username)?;
    let hash = passwords.hash(password).await?;
    let id = sqlx::query("INSERT INTO users (username, password_hash, role) VALUES (?, ?, ?)")
        .bind(username)
        .bind(hash)
        .bind(role.as_str())
        .execute(pool)
        .await
        .map_err(|error| {
            if is_duplicate(&error) {
                ApiError::Conflict(format!("user {username:?} already exists"))
            } else {
                error.into()
            }
        })?
        .last_insert_id();
    sqlx::query(
        "INSERT INTO user_station_roles (user_id, station_id, role) SELECT ?, id, ? FROM stations",
    )
    .bind(id)
    .bind(role.as_str())
    .execute(pool)
    .await?;
    find_user(pool, id).await
}

/// Sets a user's password and signs them out everywhere else. Returns
/// whether the user exists.
pub async fn set_password(
    pool: &MySqlPool,
    passwords: &Passwords,
    user_id: u64,
    password: &str,
    keep_session: Option<[u8; 32]>,
) -> ApiResult<()> {
    let hash = passwords.hash(password).await?;
    let updated = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(user_id)
        .execute(pool)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::not_found("user"));
    }
    sqlx::query("DELETE FROM sessions WHERE user_id = ? AND token_hash <> ?")
        .bind(user_id)
        .bind(keep_session.unwrap_or_default().as_slice())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn user_id(pool: &MySqlPool, username: &str) -> ApiResult<u64> {
    sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("user {username:?}")))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// A sign-in waiting for its second factor.
#[derive(Serialize, ToSchema)]
pub struct SecondFactorNeeded {
    /// The factors the user can finish signing in with.
    pub second_factor: Vec<SecondFactor>,
}

/// Signs in: answers with the user and sets the `heron_session` cookie.
/// When the user has a second factor, answers 202 with the factors they can
/// use and a `heron_pending` cookie instead (5 minutes): finish with
/// `/auth/login/totp`, `/auth/login/recovery` or `/auth/login/webauthn/…`.
#[utoipa::path(post, path = "/auth/login", tag = "auth", security(()),
    request_body = LoginRequest,
    responses((status = 200, body = User),
              (status = 202, body = SecondFactorNeeded, description = "A second factor is needed"),
              (status = 401, body = ErrorBody),
              (status = 429, body = ErrorBody, description = "Too many failures for this username")))]
async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> ApiResult<Response> {
    state.passwords.check_lockout(&request.username)?;
    let row: Option<(u64, String, bool)> =
        sqlx::query_as("SELECT id, password_hash, enabled FROM users WHERE username = ?")
            .bind(&request.username)
            .fetch_optional(&state.pool)
            .await?;
    let (id, hash, enabled) = match row {
        Some((id, hash, enabled)) => (Some(id), Some(hash), enabled),
        None => (None, None, false),
    };
    let valid = state.passwords.verify(hash, &request.password).await?;
    state.passwords.record(&request.username, valid);
    let (Some(id), true, true) = (id, valid, enabled) else {
        tracing::info!(username = %request.username, "failed sign-in");
        metrics::counter!("heron_api_logins_total", "result" => "failed").increment(1);
        return Err(ApiError::Unauthorized);
    };
    let second_factor = crate::two_factor::factors(&state.pool, id).await?;
    if !second_factor.is_empty() {
        let token = state.two_factor.start_sign_in(id, &request.username)?;
        tracing::info!(username = %request.username, "password accepted, second factor next");
        let cookie = auth::set_cookie(
            PENDING_COOKIE,
            PENDING_PATH,
            Some(&token),
            PENDING_SIGN_IN.as_secs(),
            is_https(&headers),
        );
        return Ok((
            StatusCode::ACCEPTED,
            [(header::SET_COOKIE, cookie)],
            Json(SecondFactorNeeded { second_factor }),
        )
            .into_response());
    }
    signed_in(&state, &headers, id, &request.username).await
}

/// Opens the session of a user who signed in, and answers with them.
pub async fn signed_in(
    state: &AppState,
    headers: &HeaderMap,
    user_id: u64,
    username: &str,
) -> ApiResult<Response> {
    let token = create_session(&state.pool, user_id).await?;
    tracing::info!(%username, "signed in");
    metrics::counter!("heron_api_logins_total", "result" => "ok").increment(1);
    let user = find_user(&state.pool, user_id).await?;
    let https = is_https(headers);
    let session = session_cookie(Some(&token), https);
    let pending = auth::set_cookie(PENDING_COOKIE, PENDING_PATH, None, 0, https);
    Ok((
        axum::response::AppendHeaders([
            (header::SET_COOKIE, session),
            (header::SET_COOKIE, pending),
        ]),
        Json(user),
    )
        .into_response())
}

/// Signs out: ends the session and clears the cookie.
#[utoipa::path(post, path = "/auth/logout", tag = "auth",
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn logout(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if let Credential::Session { token_hash } = user.credential {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash.as_slice())
            .execute(&state.pool)
            .await?;
    }
    let cookie = session_cookie(None, is_https(&headers));
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response())
}

/// The signed-in user.
#[utoipa::path(get, path = "/auth/me", tag = "auth",
    responses((status = 200, body = User), (status = "4XX", body = ErrorBody)))]
async fn me(user: CurrentUser, State(state): State<AppState>) -> ApiResult<Json<User>> {
    Ok(Json(find_user(&state.pool, user.id).await?))
}

/// What to change; what is left out stays as it is.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    #[serde(default)]
    pub theme: Option<Theme>,
    /// A language tag (`en`, `fr`…) or `system`. The API does not know which
    /// languages the web UI has: it falls back on the browser's for one it
    /// lacks.
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub width: Option<Width>,
    /// The On air page shows the whole queue, editable there.
    #[serde(default)]
    pub on_air_whole_queue: Option<bool>,
    /// The status bar's settings, replaced whole: a field left out takes
    /// its default (`{}` restores them all).
    #[serde(default)]
    pub status_bar: Option<StatusBarSettings>,
}

/// `system`, or a language tag: a 2 or 3 letter language and optional
/// subtags (`fr`, `pt-BR`), up to 16 characters.
fn check_language(language: &str) -> ApiResult<()> {
    let mut parts = language.split('-');
    let primary = parts.next().unwrap_or_default();
    let valid = language == "system"
        || (language.len() <= 16
            && (2..=3).contains(&primary.len())
            && primary.chars().all(|c| c.is_ascii_lowercase())
            && parts.all(|part| {
                (1..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())
            }));
    if valid {
        Ok(())
    } else {
        Err(ApiError::Invalid(format!(
            "unknown language {language:?}: use `system` or a language tag such as `en` or `fr`"
        )))
    }
}

/// Saves your preferences: the web UI's theme, language, width, whether the
/// On air page shows the whole queue, and the status bar's settings.
#[utoipa::path(put, path = "/auth/preferences", tag = "auth",
    request_body = Preferences,
    responses((status = 200, body = User), (status = "4XX", body = ErrorBody)))]
async fn set_preferences(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(preferences): Json<Preferences>,
) -> ApiResult<Json<User>> {
    if let Some(language) = &preferences.language {
        check_language(language)?;
    }
    if let Some(status_bar) = &preferences.status_bar {
        status_bar.check()?;
    }
    let status_bar = preferences
        .status_bar
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    sqlx::query(
        "UPDATE users SET theme = COALESCE(?, theme), language = COALESCE(?, language),
             width = COALESCE(?, width), on_air_whole_queue = COALESCE(?, on_air_whole_queue),
             status_bar = COALESCE(?, status_bar)
         WHERE id = ?",
    )
    .bind(preferences.theme.map(Theme::as_str))
    .bind(preferences.language)
    .bind(preferences.width.map(Width::as_str))
    .bind(preferences.on_air_whole_queue)
    .bind(status_bar)
    .bind(user.id)
    .execute(&state.pool)
    .await?;
    Ok(Json(find_user(&state.pool, user.id).await?))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordChange {
    pub current_password: String,
    pub new_password: String,
}

/// Changes your password. Your other sessions are signed out.
#[utoipa::path(put, path = "/auth/password", tag = "auth",
    request_body = PasswordChange,
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn change_password(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(change): Json<PasswordChange>,
) -> ApiResult<StatusCode> {
    state.passwords.check_lockout(&user.username)?;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await?;
    let valid = state
        .passwords
        .verify(Some(hash), &change.current_password)
        .await?;
    state.passwords.record(&user.username, valid);
    if !valid {
        return Err(ApiError::Invalid(
            "the current password is wrong".to_owned(),
        ));
    }
    let keep = match user.credential {
        Credential::Session { token_hash } => Some(token_hash),
        Credential::ApiToken { .. } => None,
    };
    set_password(
        &state.pool,
        &state.passwords,
        user.id,
        &change.new_password,
        keep,
    )
    .await?;
    tracing::info!(user = %user.username, "password changed");
    Ok(StatusCode::NO_CONTENT)
}

/// Every user. Admins only.
#[utoipa::path(get, path = "/users", tag = "users",
    responses((status = 200, body = Vec<User>), (status = "4XX", body = ErrorBody)))]
async fn list_users(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<User>>> {
    user.require(Role::Admin)?;
    Ok(Json(all_users(&state.pool).await?))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewUser {
    pub username: String,
    /// At least 8 characters.
    pub password: String,
    pub role: Role,
    /// Roles on stations (left out: `none`); the shared role on every
    /// station when left out.
    #[serde(default)]
    pub station_roles: Option<Vec<StationRole>>,
}

/// Admins only.
#[utoipa::path(post, path = "/users", tag = "users",
    request_body = NewUser,
    responses((status = 201, body = User), (status = "4XX", body = ErrorBody)))]
async fn create_user(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(new): Json<NewUser>,
) -> ApiResult<(StatusCode, Json<User>)> {
    user.require(Role::Admin)?;
    let mut created = add_user(
        &state.pool,
        &state.passwords,
        &new.username,
        &new.password,
        new.role,
    )
    .await?;
    if let Some(roles) = &new.station_roles {
        write_roles(&state.pool, created.id, roles).await?;
        created = find_user(&state.pool, created.id).await?;
    }
    tracing::info!(by = %user.username, username = %created.username, role = ?created.role, "user created");
    Ok((StatusCode::CREATED, Json(created)))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UserPatch {
    pub role: Option<Role>,
    /// Disabled users cannot sign in, and their tokens stop working.
    pub enabled: Option<bool>,
    /// Signs the user out everywhere.
    pub password: Option<String>,
    /// Replaces their roles on stations (left out: `none`).
    pub station_roles: Option<Vec<StationRole>>,
}

/// Refuses changes that would leave no enabled admin.
async fn keep_an_admin(pool: &MySqlPool, user_id: u64) -> ApiResult<()> {
    let others: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE role = 'admin' AND enabled AND id <> ?",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    if others == 0 {
        return Err(ApiError::Conflict(
            "this is the last enabled admin".to_owned(),
        ));
    }
    Ok(())
}

/// Changes a user's role, enables or disables them, or resets their
/// password. Admins only.
#[utoipa::path(patch, path = "/users/{id}", tag = "users",
    params(("id" = u64, Path)), request_body = UserPatch,
    responses((status = 200, body = User), (status = "4XX", body = ErrorBody)))]
async fn update_user(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(patch): Json<UserPatch>,
) -> ApiResult<Json<User>> {
    user.require(Role::Admin)?;
    let target = find_user(&state.pool, id).await?;
    let demoted = patch.role.is_some_and(|role| role != Role::Admin);
    let disabled = patch.enabled == Some(false);
    if target.role == Role::Admin && target.enabled && (demoted || disabled) {
        keep_an_admin(&state.pool, id).await?;
    }
    if let Some(password) = &patch.password {
        set_password(&state.pool, &state.passwords, id, password, None).await?;
    }
    if let Some(role) = patch.role {
        sqlx::query("UPDATE users SET role = ? WHERE id = ?")
            .bind(role.as_str())
            .bind(id)
            .execute(&state.pool)
            .await?;
    }
    if let Some(roles) = &patch.station_roles {
        write_roles(&state.pool, id, roles).await?;
    }
    if let Some(enabled) = patch.enabled {
        sqlx::query("UPDATE users SET enabled = ? WHERE id = ?")
            .bind(enabled)
            .bind(id)
            .execute(&state.pool)
            .await?;
        if !enabled {
            sqlx::query("DELETE FROM sessions WHERE user_id = ?")
                .bind(id)
                .execute(&state.pool)
                .await?;
        }
    }
    tracing::info!(by = %user.username, username = %target.username, "user changed");
    Ok(Json(find_user(&state.pool, id).await?))
}

/// Deletes a user with their sessions and tokens. Admins only.
#[utoipa::path(delete, path = "/users/{id}", tag = "users",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_user(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    let target = find_user(&state.pool, id).await?;
    if target.role == Role::Admin && target.enabled {
        keep_an_admin(&state.pool, id).await?;
    }
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    tracing::info!(by = %user.username, username = %target.username, "user deleted");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct ApiToken {
    pub id: u64,
    pub name: String,
    pub created_at_unix_ms: i64,
    pub last_used_at_unix_ms: Option<i64>,
    pub expires_at_unix_ms: Option<i64>,
}

/// Your API tokens (not revoked). The tokens themselves are never shown again.
#[utoipa::path(get, path = "/tokens", tag = "auth",
    responses((status = 200, body = Vec<ApiToken>), (status = "4XX", body = ErrorBody)))]
async fn list_tokens(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ApiToken>>> {
    let tokens = sqlx::query_as(
        "SELECT id, name,
                CAST(UNIX_TIMESTAMP(created_at) * 1000 AS SIGNED) AS created_at_unix_ms,
                CAST(UNIX_TIMESTAMP(last_used_at) * 1000 AS SIGNED) AS last_used_at_unix_ms,
                CAST(UNIX_TIMESTAMP(expires_at) * 1000 AS SIGNED) AS expires_at_unix_ms
         FROM api_tokens WHERE user_id = ? AND revoked_at IS NULL ORDER BY created_at",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(tokens))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewToken {
    /// What the token is for.
    pub name: String,
    /// Never expires when missing.
    pub expires_in_days: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedToken {
    pub id: u64,
    pub name: String,
    /// Send it as `Authorization: Bearer <token>`. Shown only once.
    pub token: String,
}

/// Creates an API token acting as you, with your role.
#[utoipa::path(post, path = "/tokens", tag = "auth",
    request_body = NewToken,
    responses((status = 201, body = CreatedToken), (status = "4XX", body = ErrorBody)))]
async fn create_token(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(new): Json<NewToken>,
) -> ApiResult<(StatusCode, Json<CreatedToken>)> {
    let name = new.name.trim();
    if name.is_empty() || name.chars().count() > 255 {
        return Err(ApiError::Invalid(
            "names are 1 to 255 characters".to_owned(),
        ));
    }
    if new.expires_in_days == Some(0) {
        return Err(ApiError::Invalid(
            "expires_in_days must be at least 1".to_owned(),
        ));
    }
    let (id, token) = create_api_token(&state.pool, user.id, name, new.expires_in_days).await?;
    tracing::info!(user = %user.username, id, %name, "API token created");
    Ok((
        StatusCode::CREATED,
        Json(CreatedToken {
            id,
            name: name.to_owned(),
            token,
        }),
    ))
}

/// Revokes one of your API tokens (admins: anyone's).
#[utoipa::path(delete, path = "/tokens/{id}", tag = "auth",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn revoke_token(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    let revoked = sqlx::query(
        "UPDATE api_tokens SET revoked_at = NOW(6)
         WHERE id = ? AND revoked_at IS NULL AND (user_id = ? OR ?)",
    )
    .bind(id)
    .bind(user.id)
    .bind(user.role == Role::Admin)
    .execute(&state.pool)
    .await?;
    if revoked.rows_affected() == 0 {
        return Err(ApiError::not_found("token"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Removes sessions that expired, every hour.
pub async fn purge_sessions(pool: MySqlPool) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        interval.tick().await;
        match auth::purge_expired_sessions(&pool).await {
            Ok(0) => {}
            Ok(purged) => tracing::debug!(purged, "expired sessions removed"),
            Err(error) => tracing::warn!(%error, "cannot remove expired sessions"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_tags() {
        for language in ["system", "en", "fr", "pt-BR", "zh-Hant-TW", "gsw"] {
            assert!(check_language(language).is_ok(), "{language}");
        }
        for language in [
            "",
            "e",
            "EN",
            "fr_FR",
            "french",
            "fr-",
            "en-abcdefghi",
            "a-b-c-d-e-f-g-h",
        ] {
            assert!(check_language(language).is_err(), "{language}");
        }
    }
}
