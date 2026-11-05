// SPDX-License-Identifier: AGPL-3.0-or-later

//! Two-factor authentication: an authenticator app (TOTP), passkeys
//! (WebAuthn) and recovery codes, optional unless the user's role requires
//! it. See `docs/api.md#two-factor-authentication`.
//!
//! What waits for the user (a sign-in waiting for its second factor, an
//! app or passkey being set up) is kept in memory: a restart of the API
//! only means starting that step again.

pub mod login;
pub mod totp;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, RegisterPublicKeyCredential, Url, Uuid,
    Webauthn, WebauthnBuilder,
};

use crate::app::AppState;
use crate::auth::{Credential, CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings;

/// How long a sign-in waits for its second factor.
pub const PENDING_SIGN_IN: Duration = Duration::from_secs(5 * 60);
/// How long an app or a passkey being set up waits for its confirmation.
const SETUP: Duration = Duration::from_secs(10 * 60);
pub const RECOVERY_CODES: usize = 10;
/// The name apps show next to the account.
const ISSUER: &str = "Heron";

/// Which roles require a second factor. With several stations, a user's
/// highest role counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TwoFactorSettings {
    pub admin: bool,
    pub operator: bool,
    pub viewer: bool,
}

impl TwoFactorSettings {
    pub const SETTINGS_NAME: &'static str = "two_factor";

    pub fn requires(&self, role: Role) -> bool {
        match role {
            Role::Admin => self.admin,
            Role::Operator => self.operator,
            Role::Viewer => self.viewer,
        }
    }

    fn any(&self) -> bool {
        self.admin || self.operator || self.viewer
    }
}

/// A user's second factors, as `GET /users` and `GET /auth/me` show them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, ToSchema)]
pub struct TwoFactorStatus {
    /// An authenticator app is set up.
    pub totp: bool,
    /// How many passkeys.
    pub passkeys: u32,
    /// Their role requires a second factor.
    pub required: bool,
}

/// The highest of a shared role and roles on stations (`none` left out).
pub fn highest_role(shared: Role, station_roles: &[Role]) -> Role {
    station_roles.iter().copied().fold(shared, Role::max)
}

async fn station_roles(pool: &MySqlPool, user_id: u64) -> ApiResult<Vec<Role>> {
    let roles: Vec<String> =
        sqlx::query_scalar("SELECT CAST(role AS CHAR) FROM user_station_roles WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(pool)
            .await?;
    Ok(roles.iter().filter_map(|role| Role::parse(role)).collect())
}

/// The role that makes a second factor required for this user, if any.
pub async fn required_by(pool: &MySqlPool, user_id: u64, shared: Role) -> ApiResult<Option<Role>> {
    let settings: TwoFactorSettings =
        settings::load(pool, TwoFactorSettings::SETTINGS_NAME).await?;
    if !settings.any() {
        return Ok(None);
    }
    let role = highest_role(shared, &station_roles(pool, user_id).await?);
    Ok(settings.requires(role).then_some(role))
}

pub async fn required(pool: &MySqlPool, user_id: u64, shared: Role) -> ApiResult<bool> {
    Ok(required_by(pool, user_id, shared).await?.is_some())
}

/// What a session that must set up a second factor first can reach:
/// the user, their preferences, signing out and setting up.
pub fn allowed_before_enrolment(path: &str) -> bool {
    let path = path.strip_prefix("/api/v1").unwrap_or(path);
    matches!(path, "/auth/me" | "/auth/logout" | "/auth/preferences")
        || path == "/auth/two-factor"
        || path.starts_with("/auth/two-factor/")
}

/// Every user's second factors, by user ID.
pub async fn statuses(pool: &MySqlPool) -> ApiResult<HashMap<u64, (bool, u32)>> {
    let rows: Vec<(u64, bool, i64)> = sqlx::query_as(
        "SELECT u.id,
                EXISTS (SELECT 1 FROM user_totp t WHERE t.user_id = u.id),
                (SELECT COUNT(*) FROM user_webauthn w WHERE w.user_id = u.id)
         FROM users u",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, totp, passkeys)| (id, (totp, u32::try_from(passkeys).unwrap_or(0))))
        .collect())
}

/// What waits in memory.
#[derive(Default)]
pub struct TwoFactor {
    /// Sign-ins waiting for their second factor, by the hash of their
    /// cookie's token.
    pending: Mutex<HashMap<[u8; 32], PendingSignIn>>,
    /// Apps being set up, by user: the secret.
    totp_setups: Mutex<HashMap<u64, (Vec<u8>, Instant)>>,
    /// Passkeys being added, by user: the challenge and the name.
    passkey_setups: Mutex<HashMap<u64, (PasskeyRegistration, String, Instant)>>,
}

#[derive(Clone)]
pub struct PendingSignIn {
    pub user_id: u64,
    pub username: String,
    expires: Instant,
    /// The passkey challenge sent, for `/auth/login/webauthn/finish`.
    pub webauthn: Option<PasskeyAuthentication>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl TwoFactor {
    /// Starts waiting for a user's second factor; returns the cookie's token.
    pub fn start_sign_in(&self, user_id: u64, username: &str) -> ApiResult<String> {
        let (token, hash) = crate::auth::new_token("")?;
        let mut pending = lock(&self.pending);
        pending.retain(|_, sign_in| sign_in.expires > Instant::now());
        pending.insert(
            hash,
            PendingSignIn {
                user_id,
                username: username.to_owned(),
                expires: Instant::now() + PENDING_SIGN_IN,
                webauthn: None,
            },
        );
        Ok(token)
    }

    /// The sign-in waiting behind a cookie's token.
    pub fn pending(&self, token: &str) -> Option<PendingSignIn> {
        let hash = crate::auth::token_hash(token);
        lock(&self.pending)
            .get(&hash)
            .filter(|sign_in| sign_in.expires > Instant::now())
            .cloned()
    }

    pub fn set_challenge(&self, token: &str, state: Option<PasskeyAuthentication>) {
        let hash = crate::auth::token_hash(token);
        if let Some(sign_in) = lock(&self.pending).get_mut(&hash) {
            sign_in.webauthn = state;
        }
    }

    pub fn finish_sign_in(&self, token: &str) {
        lock(&self.pending).remove(&crate::auth::token_hash(token));
    }

    fn start_totp(&self, user_id: u64, secret: Vec<u8>) {
        let mut setups = lock(&self.totp_setups);
        setups.retain(|_, (_, at)| at.elapsed() < SETUP);
        setups.insert(user_id, (secret, Instant::now()));
    }

    fn totp_secret(&self, user_id: u64) -> Option<Vec<u8>> {
        lock(&self.totp_setups)
            .get(&user_id)
            .filter(|(_, at)| at.elapsed() < SETUP)
            .map(|(secret, _)| secret.clone())
    }

    fn start_passkey(&self, user_id: u64, state: PasskeyRegistration, name: String) {
        let mut setups = lock(&self.passkey_setups);
        setups.retain(|_, (_, _, at)| at.elapsed() < SETUP);
        setups.insert(user_id, (state, name, Instant::now()));
    }

    fn take_passkey(&self, user_id: u64) -> Option<(PasskeyRegistration, String)> {
        lock(&self.passkey_setups)
            .remove(&user_id)
            .filter(|(_, _, at)| at.elapsed() < SETUP)
            .map(|(state, name, _)| (state, name))
    }
}

/// The relying party of passkeys: the address the browser shows. Its origin
/// is the request's `Origin` header (browsers send it with every `POST`),
/// or else what the proxy reports (`X-Forwarded-Proto`, `X-Forwarded-Host`
/// or `Host`); its ID is the origin's host name.
pub fn relying_party(headers: &HeaderMap) -> ApiResult<Webauthn> {
    let text = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "null")
    };
    let origin = match text(header::ORIGIN.as_str()) {
        Some(origin) => origin.to_owned(),
        None => {
            let proto = if crate::auth::is_https(headers) {
                "https"
            } else {
                "http"
            };
            let host = text("x-forwarded-host")
                .and_then(|hosts| hosts.split(',').next())
                .or_else(|| text(header::HOST.as_str()))
                .ok_or_else(|| ApiError::BadRequest("the request names no host".to_owned()))?;
            format!("{proto}://{}", host.trim())
        }
    };
    let unusable = || {
        ApiError::Invalid(format!(
            "passkeys need a host name, over HTTPS or on localhost: {origin} cannot have them"
        ))
    };
    let url = Url::parse(&origin).map_err(|_| unusable())?;
    let rp_id = url.domain().ok_or_else(unusable)?.to_owned();
    WebauthnBuilder::new(&rp_id, &url)
        .map_err(|_| unusable())?
        .rp_name(ISSUER)
        .build()
        .map_err(|_| unusable())
}

/// Passkeys' user handle: the user's ID, which never changes (a backup
/// keeps it).
fn user_handle(user_id: u64) -> Uuid {
    Uuid::from_u64_pair(0, user_id)
}

pub async fn passkeys(pool: &MySqlPool, user_id: u64) -> ApiResult<Vec<(u64, Passkey)>> {
    let rows: Vec<(u64, String)> = sqlx::query_as(
        "SELECT id, CAST(passkey AS CHAR) FROM user_webauthn WHERE user_id = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|(id, json)| {
            serde_json::from_str(&json)
                .map(|passkey| (id, passkey))
                .map_err(|error| ApiError::Internal(format!("stored passkey {id}: {error}")))
        })
        .collect()
}

/// The second factors a user can sign in with.
pub async fn factors(pool: &MySqlPool, user_id: u64) -> ApiResult<Vec<SecondFactor>> {
    let (totp, passkeys, codes): (bool, i64, i64) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM user_totp WHERE user_id = ?),
                (SELECT COUNT(*) FROM user_webauthn WHERE user_id = ?),
                (SELECT COUNT(*) FROM user_recovery_codes WHERE user_id = ? AND used_at IS NULL)",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    let mut factors = Vec::new();
    if totp {
        factors.push(SecondFactor::Totp);
    }
    if passkeys > 0 {
        factors.push(SecondFactor::Webauthn);
    }
    if !factors.is_empty() && codes > 0 {
        factors.push(SecondFactor::Recovery);
    }
    Ok(factors)
}

/// A way to finish signing in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecondFactor {
    /// A code of the authenticator app: `POST /auth/login/totp`.
    Totp,
    /// A passkey: `POST /auth/login/webauthn/start`, then `/finish`.
    Webauthn,
    /// A recovery code: `POST /auth/login/recovery`.
    Recovery,
}

/// Recovery codes: 8 characters (`XXXX-XXXX`) of an alphabet without 0, 1,
/// I and O, so they can be read back from paper.
const CODE_ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

fn new_recovery_codes() -> ApiResult<Vec<String>> {
    let mut bytes = [0u8; RECOVERY_CODES * 8];
    getrandom::fill(&mut bytes)
        .map_err(|error| ApiError::Internal(format!("no randomness: {error}")))?;
    Ok(bytes
        .chunks(8)
        .map(|chunk| {
            let code: String = chunk
                .iter()
                .map(|byte| char::from(CODE_ALPHABET[usize::from(byte & 31)]))
                .collect();
            format!("{}-{}", &code[..4], &code[4..])
        })
        .collect())
}

/// A recovery code as typed, as it is hashed: upper case, without the dash
/// or spaces. `None` when it cannot be one.
pub fn normalize_code(code: &str) -> Option<String> {
    let code: String = code
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (code.len() == 8 && code.bytes().all(|byte| CODE_ALPHABET.contains(&byte))).then_some(code)
}

/// Replaces a user's recovery codes with 10 new ones, and returns them.
async fn replace_recovery_codes(state: &AppState, user_id: u64) -> ApiResult<Vec<String>> {
    let codes = new_recovery_codes()?;
    let normalized = codes
        .iter()
        .filter_map(|code| normalize_code(code))
        .collect();
    let hashes = state.passwords.hash_codes(normalized).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for hash in hashes {
        sqlx::query("INSERT INTO user_recovery_codes (user_id, code_hash) VALUES (?, ?)")
            .bind(user_id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(codes)
}

/// Removes a user's app, passkeys and recovery codes. Returns whether they
/// had any. For `heron-api user reset-2fa`.
pub async fn reset(pool: &MySqlPool, user_id: u64) -> ApiResult<bool> {
    let mut tx = pool.begin().await?;
    let mut removed = 0;
    for table in ["user_totp", "user_webauthn", "user_recovery_codes"] {
        removed += sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE user_id = ?"
        )))
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    tx.commit().await?;
    Ok(removed > 0)
}

/// Asks for the user's password again, with the sign-in lockout.
async fn check_password(state: &AppState, user: &CurrentUser, password: &str) -> ApiResult<()> {
    state.passwords.check_lockout(&user.username)?;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await?;
    let valid = state.passwords.verify(Some(hash), password).await?;
    state.passwords.record(&user.username, valid);
    if valid {
        Ok(())
    } else {
        Err(ApiError::Invalid("the password is wrong".to_owned()))
    }
}

/// Ends the user's other sessions, after a factor changed.
async fn sign_out_elsewhere(pool: &MySqlPool, user: &CurrentUser) -> ApiResult<()> {
    let keep = match user.credential {
        Credential::Session { token_hash } => token_hash,
        Credential::ApiToken { .. } => [0; 32],
    };
    sqlx::query("DELETE FROM sessions WHERE user_id = ? AND token_hash <> ?")
        .bind(user.id)
        .bind(keep.as_slice())
        .execute(pool)
        .await?;
    Ok(())
}

/// After a factor was set up: the recovery codes when it is the first one.
async fn first_codes(state: &AppState, user_id: u64) -> ApiResult<Option<Vec<String>>> {
    let left: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM user_recovery_codes WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(&state.pool)
            .await?;
    if left > 0 {
        return Ok(None);
    }
    Ok(Some(replace_recovery_codes(state, user_id).await?))
}

/// Refuses to remove a user's last factor when their role requires one;
/// removes their recovery codes when it was the last.
async fn after_removal(pool: &MySqlPool, user: &CurrentUser) -> ApiResult<()> {
    let left = factors(pool, user.id).await?;
    if !left.contains(&SecondFactor::Totp) && !left.contains(&SecondFactor::Webauthn) {
        sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = ?")
            .bind(user.id)
            .execute(pool)
            .await?;
        tracing::info!(user = %user.username, "two-factor authentication turned off");
    }
    Ok(())
}

async fn is_last_factor(pool: &MySqlPool, user: &CurrentUser) -> ApiResult<bool> {
    let left = factors(pool, user.id).await?;
    let count = left
        .iter()
        .filter(|factor| **factor != SecondFactor::Recovery)
        .count();
    Ok(count <= 1)
}

async fn refuse_last_removal(pool: &MySqlPool, user: &CurrentUser, shared: Role) -> ApiResult<()> {
    if is_last_factor(pool, user).await? && required(pool, user.id, shared).await? {
        return Err(ApiError::Conflict(
            "your role requires two-factor authentication: set up another factor first".to_owned(),
        ));
    }
    Ok(())
}

/// The user's shared role (a station's route may have given another one).
async fn shared_role(pool: &MySqlPool, user_id: u64) -> ApiResult<Role> {
    let role: String = sqlx::query_scalar("SELECT CAST(role AS CHAR) FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(Role::parse(&role).unwrap_or(Role::Viewer))
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_two_factor))
        .routes(routes!(start_totp, remove_totp))
        .routes(routes!(confirm_totp))
        .routes(routes!(start_passkey))
        .routes(routes!(finish_passkey))
        .routes(routes!(rename_passkey, remove_passkey))
        .routes(routes!(regenerate_codes))
        .routes(routes!(get_two_factor_settings, put_two_factor_settings))
        .merge(login::router())
}

#[derive(Serialize, ToSchema)]
pub struct TotpView {
    pub created_at_unix_ms: i64,
    pub last_used_at_unix_ms: Option<i64>,
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct PasskeyView {
    pub id: u64,
    pub name: String,
    pub created_at_unix_ms: i64,
    pub last_used_at_unix_ms: Option<i64>,
}

/// Your second factors.
#[derive(Serialize, ToSchema)]
pub struct TwoFactorView {
    /// The authenticator app, when one is set up.
    pub totp: Option<TotpView>,
    pub passkeys: Vec<PasskeyView>,
    /// Recovery codes not used yet.
    pub recovery_codes_left: u32,
    /// The role that requires a second factor (your highest), if one does.
    pub required_by: Option<Role>,
}

/// Your authenticator app, passkeys and recovery codes.
#[utoipa::path(get, path = "/auth/two-factor", tag = "auth",
    responses((status = 200, body = TwoFactorView), (status = "4XX", body = ErrorBody)))]
async fn get_two_factor(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<TwoFactorView>> {
    let totp: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT CAST(UNIX_TIMESTAMP(created_at) * 1000 AS SIGNED),
                CAST(UNIX_TIMESTAMP(last_used_at) * 1000 AS SIGNED)
         FROM user_totp WHERE user_id = ?",
    )
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;
    let passkeys = sqlx::query_as(
        "SELECT id, name,
                CAST(UNIX_TIMESTAMP(created_at) * 1000 AS SIGNED) AS created_at_unix_ms,
                CAST(UNIX_TIMESTAMP(last_used_at) * 1000 AS SIGNED) AS last_used_at_unix_ms
         FROM user_webauthn WHERE user_id = ? ORDER BY id",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    let left: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_recovery_codes WHERE user_id = ? AND used_at IS NULL",
    )
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;
    let shared = shared_role(&state.pool, user.id).await?;
    Ok(Json(TwoFactorView {
        totp: totp.map(|(created, used)| TotpView {
            created_at_unix_ms: created,
            last_used_at_unix_ms: used,
        }),
        passkeys,
        recovery_codes_left: u32::try_from(left).unwrap_or(0),
        required_by: required_by(&state.pool, user.id, shared).await?,
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordCheck {
    /// Your password, asked again.
    pub password: String,
}

/// An authenticator app to set up.
#[derive(Serialize, ToSchema)]
pub struct TotpSetup {
    /// The secret in base32, to type into the app.
    pub secret: String,
    /// The `otpauth://` URI the QR code holds.
    pub uri: String,
    /// The QR code, as an SVG image.
    pub qr_svg: String,
}

/// Starts setting up an authenticator app: a new secret, which
/// `POST /auth/two-factor/totp/confirm` turns on within 10 minutes. It
/// replaces your app once confirmed.
#[utoipa::path(post, path = "/auth/two-factor/totp", tag = "auth",
    request_body = PasswordCheck,
    responses((status = 200, body = TotpSetup), (status = "4XX", body = ErrorBody)))]
async fn start_totp(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<PasswordCheck>,
) -> ApiResult<Json<TotpSetup>> {
    check_password(&state, &user, &request.password).await?;
    let mut secret = vec![0u8; totp::SECRET_LEN];
    getrandom::fill(&mut secret)
        .map_err(|error| ApiError::Internal(format!("no randomness: {error}")))?;
    let uri = totp::uri(ISSUER, &user.username, &secret);
    let qr_svg = totp::qr_svg(&uri)
        .ok_or_else(|| ApiError::Internal("cannot draw the QR code".to_owned()))?;
    let setup = TotpSetup {
        secret: totp::base32(&secret),
        uri,
        qr_svg,
    };
    state.two_factor.start_totp(user.id, secret);
    Ok(Json(setup))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeRequest {
    pub code: String,
}

/// A factor set up.
#[derive(Serialize, ToSchema)]
pub struct FactorAdded {
    /// The 10 recovery codes, shown once: when this is your first factor.
    pub recovery_codes: Option<Vec<String>>,
}

pub fn now_step() -> u64 {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    totp::step_at(seconds)
}

/// Turns the app being set up on, with a code it shows. Your other
/// sessions are signed out.
#[utoipa::path(post, path = "/auth/two-factor/totp/confirm", tag = "auth",
    request_body = CodeRequest,
    responses((status = 200, body = FactorAdded), (status = "4XX", body = ErrorBody)))]
async fn confirm_totp(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<CodeRequest>,
) -> ApiResult<Json<FactorAdded>> {
    let Some(secret) = state.two_factor.totp_secret(user.id) else {
        return Err(ApiError::Conflict(
            "no app is being set up, or it took more than 10 minutes: start again".to_owned(),
        ));
    };
    state.passwords.check_lockout(&user.username)?;
    let step = totp::verify(&secret, &request.code, now_step(), None);
    state.passwords.record(&user.username, step.is_some());
    let Some(step) = step else {
        return Err(ApiError::Invalid("the code is wrong".to_owned()));
    };
    sqlx::query(
        "INSERT INTO user_totp (user_id, secret, last_step, last_used_at) VALUES (?, ?, ?, NULL)
         ON DUPLICATE KEY UPDATE secret = VALUES(secret), last_step = VALUES(last_step),
             created_at = NOW(6), last_used_at = NULL",
    )
    .bind(user.id)
    .bind(&secret)
    .bind(step)
    .execute(&state.pool)
    .await?;
    lock(&state.two_factor.totp_setups).remove(&user.id);
    let recovery_codes = first_codes(&state, user.id).await?;
    sign_out_elsewhere(&state.pool, &user).await?;
    tracing::info!(user = %user.username, "authenticator app set up");
    Ok(Json(FactorAdded { recovery_codes }))
}

/// Removes your authenticator app. Refused when it is your last factor
/// and your role requires one.
#[utoipa::path(delete, path = "/auth/two-factor/totp", tag = "auth",
    request_body = PasswordCheck,
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn remove_totp(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<PasswordCheck>,
) -> ApiResult<StatusCode> {
    check_password(&state, &user, &request.password).await?;
    if !factors(&state.pool, user.id)
        .await?
        .contains(&SecondFactor::Totp)
    {
        return Err(ApiError::not_found("authenticator app"));
    }
    let shared = shared_role(&state.pool, user.id).await?;
    refuse_last_removal(&state.pool, &user, shared).await?;
    sqlx::query("DELETE FROM user_totp WHERE user_id = ?")
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    after_removal(&state.pool, &user).await?;
    sign_out_elsewhere(&state.pool, &user).await?;
    tracing::info!(user = %user.username, "authenticator app removed");
    Ok(StatusCode::NO_CONTENT)
}

/// What the browser needs to create or use a passkey, for
/// `navigator.credentials.create()` or `.get()`: binary fields in base64url.
#[derive(Serialize, ToSchema)]
pub struct WebauthnOptions {
    #[serde(rename = "publicKey")]
    #[schema(value_type = Object)]
    pub public_key: Value,
}

impl WebauthnOptions {
    pub fn from(challenge: impl Serialize) -> ApiResult<Self> {
        let mut value = serde_json::to_value(challenge)
            .map_err(|error| ApiError::Internal(format!("passkey options: {error}")))?;
        Ok(Self {
            public_key: value
                .get_mut("publicKey")
                .map(Value::take)
                .unwrap_or(Value::Null),
        })
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewPasskey {
    /// Your password, asked again.
    pub password: String,
    /// What it is: "MacBook (Touch ID)", "Blue security key"…
    pub name: String,
}

fn check_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 255 {
        return Err(ApiError::Invalid(
            "names are 1 to 255 characters".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

/// Starts adding a passkey: the options for `navigator.credentials.create()`.
#[utoipa::path(post, path = "/auth/two-factor/passkeys", tag = "auth",
    request_body = NewPasskey,
    responses((status = 200, body = WebauthnOptions), (status = "4XX", body = ErrorBody)))]
async fn start_passkey(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<NewPasskey>,
) -> ApiResult<Json<WebauthnOptions>> {
    let name = check_name(&request.name)?;
    let webauthn = relying_party(&headers)?;
    check_password(&state, &user, &request.password).await?;
    let existing: Vec<_> = passkeys(&state.pool, user.id)
        .await?
        .iter()
        .map(|(_, passkey)| passkey.cred_id().clone())
        .collect();
    let (challenge, registration) = webauthn
        .start_passkey_registration(
            user_handle(user.id),
            &user.username,
            &user.username,
            Some(existing),
        )
        .map_err(|error| ApiError::Invalid(format!("cannot add a passkey: {error}")))?;
    state.two_factor.start_passkey(user.id, registration, name);
    Ok(Json(WebauthnOptions::from(challenge)?))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WebauthnCredential {
    /// What `navigator.credentials` answered, as JSON (binary fields in
    /// base64url).
    #[schema(value_type = Object)]
    pub credential: Value,
}

/// Adds the passkey the browser created. Your other sessions are signed out.
#[utoipa::path(post, path = "/auth/two-factor/passkeys/finish", tag = "auth",
    request_body = WebauthnCredential,
    responses((status = 200, body = FactorAdded), (status = "4XX", body = ErrorBody)))]
async fn finish_passkey(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebauthnCredential>,
) -> ApiResult<Json<FactorAdded>> {
    let Some((registration, name)) = state.two_factor.take_passkey(user.id) else {
        return Err(ApiError::Conflict(
            "no passkey is being added, or it took more than 10 minutes: start again".to_owned(),
        ));
    };
    let credential: RegisterPublicKeyCredential = serde_json::from_value(request.credential)
        .map_err(|error| ApiError::Invalid(format!("not a passkey: {error}")))?;
    let passkey = relying_party(&headers)?
        .finish_passkey_registration(&credential, &registration)
        .map_err(|error| ApiError::Invalid(format!("the passkey was refused: {error}")))?;
    let json = serde_json::to_string(&passkey)
        .map_err(|error| ApiError::Internal(format!("passkey: {error}")))?;
    sqlx::query(
        "INSERT INTO user_webauthn (user_id, credential_id, name, passkey) VALUES (?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(passkey.cred_id().as_slice())
    .bind(&name)
    .bind(json)
    .execute(&state.pool)
    .await
    .map_err(|error| {
        if crate::error::is_duplicate(&error) {
            ApiError::Conflict("this passkey was added already".to_owned())
        } else {
            error.into()
        }
    })?;
    let recovery_codes = first_codes(&state, user.id).await?;
    sign_out_elsewhere(&state.pool, &user).await?;
    tracing::info!(user = %user.username, %name, "passkey added");
    Ok(Json(FactorAdded { recovery_codes }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasskeyRename {
    pub name: String,
}

/// Renames one of your passkeys.
#[utoipa::path(patch, path = "/auth/two-factor/passkeys/{id}", tag = "auth",
    params(("id" = u64, Path)), request_body = PasskeyRename,
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn rename_passkey(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(request): Json<PasskeyRename>,
) -> ApiResult<StatusCode> {
    let name = check_name(&request.name)?;
    let found: Option<u64> =
        sqlx::query_scalar("SELECT id FROM user_webauthn WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user.id)
            .fetch_optional(&state.pool)
            .await?;
    if found.is_none() {
        return Err(ApiError::not_found("passkey"));
    }
    sqlx::query("UPDATE user_webauthn SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes one of your passkeys. Refused when it is your last factor and
/// your role requires one.
#[utoipa::path(delete, path = "/auth/two-factor/passkeys/{id}", tag = "auth",
    params(("id" = u64, Path)), request_body = PasswordCheck,
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn remove_passkey(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(request): Json<PasswordCheck>,
) -> ApiResult<StatusCode> {
    check_password(&state, &user, &request.password).await?;
    let name: Option<String> =
        sqlx::query_scalar("SELECT name FROM user_webauthn WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user.id)
            .fetch_optional(&state.pool)
            .await?;
    let Some(name) = name else {
        return Err(ApiError::not_found("passkey"));
    };
    let shared = shared_role(&state.pool, user.id).await?;
    refuse_last_removal(&state.pool, &user, shared).await?;
    sqlx::query("DELETE FROM user_webauthn WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    after_removal(&state.pool, &user).await?;
    sign_out_elsewhere(&state.pool, &user).await?;
    tracing::info!(user = %user.username, %name, "passkey removed");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
pub struct RecoveryCodes {
    /// Shown once.
    pub recovery_codes: Vec<String>,
}

/// 10 new recovery codes; the old ones stop working.
#[utoipa::path(post, path = "/auth/two-factor/recovery-codes", tag = "auth",
    request_body = PasswordCheck,
    responses((status = 200, body = RecoveryCodes), (status = "4XX", body = ErrorBody)))]
async fn regenerate_codes(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<PasswordCheck>,
) -> ApiResult<Json<RecoveryCodes>> {
    check_password(&state, &user, &request.password).await?;
    let factors = factors(&state.pool, user.id).await?;
    if !factors.contains(&SecondFactor::Totp) && !factors.contains(&SecondFactor::Webauthn) {
        return Err(ApiError::Conflict(
            "set up an authenticator app or a passkey first".to_owned(),
        ));
    }
    let recovery_codes = replace_recovery_codes(&state, user.id).await?;
    tracing::info!(user = %user.username, "new recovery codes");
    Ok(Json(RecoveryCodes { recovery_codes }))
}

/// Which roles require a second factor.
#[utoipa::path(get, path = "/settings/two-factor", tag = "users",
    responses((status = 200, body = TwoFactorSettings), (status = "4XX", body = ErrorBody)))]
async fn get_two_factor_settings(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<TwoFactorSettings>> {
    Ok(Json(
        settings::load(&state.pool, TwoFactorSettings::SETTINGS_NAME).await?,
    ))
}

/// Requires a second factor for some roles. Admins only.
#[utoipa::path(put, path = "/settings/two-factor", tag = "users",
    request_body = TwoFactorSettings,
    responses((status = 200, body = TwoFactorSettings), (status = "4XX", body = ErrorBody)))]
async fn put_two_factor_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(settings): Json<TwoFactorSettings>,
) -> ApiResult<Json<TwoFactorSettings>> {
    user.require(Role::Admin)?;
    settings::save(&state.pool, TwoFactorSettings::SETTINGS_NAME, &settings).await?;
    tracing::info!(by = %user.username, ?settings, "two-factor requirement changed");
    Ok(Json(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_highest_role_counts() {
        assert_eq!(highest_role(Role::Viewer, &[]), Role::Viewer);
        assert_eq!(
            highest_role(Role::Viewer, &[Role::Viewer, Role::Operator]),
            Role::Operator
        );
        assert_eq!(highest_role(Role::Admin, &[Role::Viewer]), Role::Admin);
        let settings = TwoFactorSettings {
            operator: true,
            ..TwoFactorSettings::default()
        };
        assert!(settings.requires(Role::Operator));
        assert!(!settings.requires(Role::Admin) && !settings.requires(Role::Viewer));
        assert!(!TwoFactorSettings::default().any());
    }

    #[test]
    fn enrolment_reaches_only_the_account() {
        for path in [
            "/api/v1/auth/me",
            "/api/v1/auth/logout",
            "/api/v1/auth/preferences",
            "/api/v1/auth/two-factor",
            "/api/v1/auth/two-factor/totp/confirm",
            "/api/v1/auth/two-factor/passkeys/3",
        ] {
            assert!(allowed_before_enrolment(path), "{path}");
        }
        for path in [
            "/api/v1/queue",
            "/api/v1/auth/password",
            "/api/v1/tokens",
            "/api/v1/auth/two-factorx",
            "/api/v1/settings/two-factor",
            "/api/v1/users",
        ] {
            assert!(!allowed_before_enrolment(path), "{path}");
        }
    }

    #[test]
    fn recovery_codes_are_readable_and_normalized() {
        let codes = new_recovery_codes().unwrap();
        assert_eq!(codes.len(), RECOVERY_CODES);
        for code in &codes {
            assert_eq!(code.len(), 9);
            assert_eq!(&code[4..5], "-");
            assert!(normalize_code(code).is_some(), "{code}");
        }
        assert_ne!(codes[0], codes[1]);
        assert_eq!(normalize_code("7kq2-m9xd").as_deref(), Some("7KQ2M9XD"));
        assert_eq!(normalize_code(" 7KQ2 M9XD ").as_deref(), Some("7KQ2M9XD"));
        assert_eq!(normalize_code("7KQ2-M9X"), None);
        assert_eq!(normalize_code("0KQ2-M9XD"), None);
    }

    #[test]
    fn the_relying_party_is_the_browsers_address() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "heron:8080".parse().unwrap());
        headers.insert(
            header::ORIGIN,
            "https://radio.example:8443".parse().unwrap(),
        );
        assert!(relying_party(&headers).is_ok());
        headers.remove(header::ORIGIN);
        headers.insert("x-forwarded-host", "radio.example".parse().unwrap());
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(relying_party(&headers).is_ok());
        // An IP address cannot have passkeys.
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "http://192.168.1.10:8000".parse().unwrap());
        assert!(matches!(relying_party(&headers), Err(ApiError::Invalid(_))));
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "http://localhost:8000".parse().unwrap());
        assert!(relying_party(&headers).is_ok());
    }
}
