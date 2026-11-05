// SPDX-License-Identifier: AGPL-3.0-or-later

//! Local users, sessions and API tokens. See `docs/api.md#authentication`.
//!
//! - The web app signs in and gets a session cookie (`heron_session`,
//!   HttpOnly, SameSite=Strict). Requests that change something with a
//!   cookie must also carry an `X-Requested-With` header, which pages on
//!   other sites cannot add without the API's consent.
//! - Scripts use API tokens: `Authorization: Bearer heron_…`.
//! - Only hashes of session and API tokens are stored. Passwords are hashed
//!   with Argon2id.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, header};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use tokio::sync::Semaphore;
use utoipa::ToSchema;

use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

pub const SESSION_COOKIE: &str = "heron_session";
pub const SESSION_DAYS: u32 = 30;
const API_TOKEN_PREFIX: &str = "heron_";
pub const MIN_PASSWORD_LENGTH: usize = 8;

/// What a user may do. Each role can do everything the previous one can.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Sees everything except users and secrets.
    Viewer,
    /// Runs the station: queue, transport, FX, tracks and categories.
    Operator,
    /// Also settings, encoders, outputs and users.
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Admin => "admin",
        }
    }

    pub fn parse(role: &str) -> Option<Self> {
        match role {
            "viewer" => Some(Self::Viewer),
            "operator" => Some(Self::Operator),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(role: &str) -> Result<Self, Self::Err> {
        Self::parse(role).ok_or_else(|| format!("unknown role {role:?} (viewer, operator, admin)"))
    }
}

/// How the current request was authenticated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Credential {
    Session { token_hash: [u8; 32] },
    ApiToken { id: u64 },
}

/// The user making the request. Extracting it answers 401 when the
/// request carries no valid session or token.
#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub id: u64,
    pub username: String,
    pub role: Role,
    pub credential: Credential,
}

impl CurrentUser {
    /// Answers 403 unless the user has at least `role`.
    pub fn require(&self, role: Role) -> ApiResult<()> {
        if self.role >= role {
            Ok(())
        } else {
            Err(ApiError::Forbidden)
        }
    }
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        if let Some(token) = bearer_token(&parts.headers) {
            let user = authenticate_token(&state.pool, token).await?;
            return on_station(&state.pool, user, parts).await;
        }
        let Some(token) = cookie(&parts.headers, SESSION_COOKIE) else {
            return Err(ApiError::Unauthorized);
        };
        let user = authenticate_session(&state.pool, token).await?;
        let safe = matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS);
        if !safe && !parts.headers.contains_key("x-requested-with") {
            return Err(ApiError::BadRequest(
                "requests signed in with a cookie need an X-Requested-With header".to_owned(),
            ));
        }
        on_station(&state.pool, user, parts).await
    }
}

/// On a station's route, the user's role is their role on that station; a
/// station they have `none` on is not found.
async fn on_station(
    pool: &MySqlPool,
    mut user: CurrentUser,
    parts: &Parts,
) -> ApiResult<CurrentUser> {
    let Some(crate::stations::StationId(station_id)) = parts.extensions.get().copied() else {
        return Ok(user);
    };
    if user.role == Role::Admin {
        return Ok(user);
    }
    match station_role(pool, user.id, station_id).await? {
        Some(role) => {
            user.role = role;
            Ok(user)
        }
        None => Err(ApiError::NotFound(format!(
            "station {station_id} not found"
        ))),
    }
}

/// A user's role on a station; `None` for `none`, a shared admin's aside.
pub async fn station_role(
    pool: &MySqlPool,
    user_id: u64,
    station_id: u64,
) -> ApiResult<Option<Role>> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT CAST(role AS CHAR) FROM user_station_roles WHERE user_id = ? AND station_id = ?",
    )
    .bind(user_id)
    .bind(station_id)
    .fetch_optional(pool)
    .await?;
    Ok(role.as_deref().and_then(Role::parse))
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|token| !token.is_empty())
}

/// The value of a cookie sent by the browser.
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

pub fn token_hash(token: &str) -> [u8; 32] {
    *blake3::hash(token.as_bytes()).as_bytes()
}

/// A new random token (256 bits) and its hash.
fn new_token(prefix: &str) -> ApiResult<(String, [u8; 32])> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| ApiError::Internal(format!("no randomness: {error}")))?;
    let token = format!(
        "{prefix}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    );
    let hash = token_hash(&token);
    Ok((token, hash))
}

async fn authenticate_session(pool: &MySqlPool, token: &str) -> ApiResult<CurrentUser> {
    let hash = token_hash(token);
    let row: Option<(u64, String, String, bool)> = sqlx::query_as(
        "SELECT u.id, u.username, CAST(u.role AS CHAR),
                s.last_seen_at < NOW(6) - INTERVAL 1 MINUTE
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ? AND s.expires_at > NOW(6) AND u.enabled",
    )
    .bind(hash.as_slice())
    .fetch_optional(pool)
    .await?;
    let Some((id, username, role, stale)) = row else {
        return Err(ApiError::Unauthorized);
    };
    if stale {
        // Sliding expiry, written at most once a minute.
        sqlx::query(
            "UPDATE sessions SET last_seen_at = NOW(6),
                    expires_at = NOW(6) + INTERVAL ? DAY
             WHERE token_hash = ?",
        )
        .bind(SESSION_DAYS)
        .bind(hash.as_slice())
        .execute(pool)
        .await?;
    }
    Ok(CurrentUser {
        id,
        username,
        role: Role::parse(&role).unwrap_or(Role::Viewer),
        credential: Credential::Session { token_hash: hash },
    })
}

async fn authenticate_token(pool: &MySqlPool, token: &str) -> ApiResult<CurrentUser> {
    let row: Option<(u64, u64, String, String, bool)> = sqlx::query_as(
        "SELECT t.id, u.id, u.username, CAST(u.role AS CHAR),
                t.last_used_at IS NULL OR t.last_used_at < NOW(6) - INTERVAL 1 MINUTE
         FROM api_tokens t JOIN users u ON u.id = t.user_id
         WHERE t.token_hash = ? AND t.revoked_at IS NULL
           AND (t.expires_at IS NULL OR t.expires_at > NOW(6)) AND u.enabled",
    )
    .bind(token_hash(token).as_slice())
    .fetch_optional(pool)
    .await?;
    let Some((token_id, id, username, role, stale)) = row else {
        return Err(ApiError::Unauthorized);
    };
    if stale {
        sqlx::query("UPDATE api_tokens SET last_used_at = NOW(6) WHERE id = ?")
            .bind(token_id)
            .execute(pool)
            .await?;
    }
    Ok(CurrentUser {
        id,
        username,
        role: Role::parse(&role).unwrap_or(Role::Viewer),
        credential: Credential::ApiToken { id: token_id },
    })
}

/// Opens a session and returns its token, for the cookie.
pub async fn create_session(pool: &MySqlPool, user_id: u64) -> ApiResult<String> {
    let (token, hash) = new_token("")?;
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at)
         VALUES (?, ?, NOW(6) + INTERVAL ? DAY)",
    )
    .bind(hash.as_slice())
    .bind(user_id)
    .bind(SESSION_DAYS)
    .execute(pool)
    .await?;
    sqlx::query("UPDATE users SET last_login_at = NOW(6) WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(token)
}

/// Creates an API token for a user and returns it: it is never shown again.
pub async fn create_api_token(
    pool: &MySqlPool,
    user_id: u64,
    name: &str,
    expires_in_days: Option<u32>,
) -> ApiResult<(u64, String)> {
    let (token, hash) = new_token(API_TOKEN_PREFIX)?;
    let id = sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, expires_at)
         VALUES (?, ?, ?, IF(? IS NULL, NULL, NOW(6) + INTERVAL ? DAY))",
    )
    .bind(user_id)
    .bind(name)
    .bind(hash.as_slice())
    .bind(expires_in_days)
    .bind(expires_in_days)
    .execute(pool)
    .await?
    .last_insert_id();
    Ok((id, token))
}

/// The `Set-Cookie` header opening a session (or closing it, with `None`).
pub fn session_cookie(token: Option<&str>, secure: bool) -> HeaderValue {
    let (value, max_age) = match token {
        Some(token) => (token, u64::from(SESSION_DAYS) * 86_400),
        None => ("", 0),
    };
    let secure = if secure { "; Secure" } else { "" };
    let cookie = format!(
        "{SESSION_COOKIE}={value}; Path=/api; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    );
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// Whether the browser reached the API over HTTPS (through the proxy).
pub fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|proto| proto.eq_ignore_ascii_case("https"))
}

/// Deletes expired sessions. Run now and then.
pub async fn purge_expired_sessions(pool: &MySqlPool) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM sessions WHERE expires_at <= NOW(6)")
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// Password hashing, limited to one at a time: Argon2 is meant to be slow,
/// and on a Raspberry Pi the engine needs the other cores.
pub struct Passwords {
    permits: Semaphore,
    failures: Mutex<HashMap<String, (u32, Instant)>>,
}

/// Failed sign-ins for one username before it is locked for a minute.
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);

/// Verified when the username does not exist, so the answer takes as long.
static DUMMY_HASH: LazyLock<String> =
    LazyLock::new(|| hash_blocking("not a real password").unwrap_or_default());

fn hash_blocking(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|error| error.to_string())
}

fn verify_blocking(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

impl Default for Passwords {
    fn default() -> Self {
        Self {
            permits: Semaphore::new(1),
            failures: Mutex::new(HashMap::new()),
        }
    }
}

impl Passwords {
    pub async fn hash(&self, password: &str) -> ApiResult<String> {
        check_password(password)?;
        let _permit = self.permits.acquire().await.map_err(internal)?;
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || hash_blocking(&password))
            .await
            .map_err(internal)?
            .map_err(ApiError::Internal)
    }

    /// Checks a password against a stored hash (`None`: unknown user).
    pub async fn verify(&self, hash: Option<String>, password: &str) -> ApiResult<bool> {
        let _permit = self.permits.acquire().await.map_err(internal)?;
        let known = hash.is_some();
        let hash = hash.unwrap_or_else(|| DUMMY_HASH.clone());
        let password = password.to_owned();
        let valid = tokio::task::spawn_blocking(move || verify_blocking(&hash, &password))
            .await
            .map_err(internal)?;
        Ok(known && valid)
    }

    /// Answers 429 while a username is locked after too many failures.
    pub fn check_lockout(&self, username: &str) -> ApiResult<()> {
        let failures = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match failures.get(username) {
            Some((count, last)) if *count >= MAX_FAILURES && last.elapsed() < LOCKOUT => {
                Err(ApiError::TooManyRequests)
            }
            _ => Ok(()),
        }
    }

    pub fn record(&self, username: &str, success: bool) {
        let mut failures = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if success {
            failures.remove(username);
            return;
        }
        if failures.len() > 10_000 {
            failures.retain(|_, (_, last)| last.elapsed() < LOCKOUT);
        }
        let entry = failures
            .entry(username.to_owned())
            .or_insert((0, Instant::now()));
        if entry.1.elapsed() >= LOCKOUT {
            entry.0 = 0;
        }
        entry.0 += 1;
        entry.1 = Instant::now();
    }
}

pub fn check_password(password: &str) -> ApiResult<()> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(ApiError::Invalid(format!(
            "passwords need at least {MIN_PASSWORD_LENGTH} characters"
        )));
    }
    Ok(())
}

pub fn check_username(username: &str) -> ApiResult<()> {
    let valid = !username.is_empty()
        && username.len() <= 64
        && username
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'));
    if valid {
        Ok(())
    } else {
        Err(ApiError::Invalid(
            "usernames are 1 to 64 letters, digits, '.', '_', '-' or '@'".to_owned(),
        ))
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered() {
        assert!(Role::Admin > Role::Operator && Role::Operator > Role::Viewer);
        assert_eq!(Role::parse("operator"), Some(Role::Operator));
        assert_eq!(Role::parse("root"), None);
    }

    #[test]
    fn reads_cookies_and_bearer_tokens() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; heron_session=abc; other=1"),
        );
        assert_eq!(cookie(&headers, SESSION_COOKIE), Some("abc"));
        assert_eq!(cookie(&headers, "missing"), None);
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer heron_xyz"),
        );
        assert_eq!(bearer_token(&headers), Some("heron_xyz"));
    }

    #[test]
    fn passwords_hash_and_verify() {
        let hash = hash_blocking("correct horse").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_blocking(&hash, "correct horse"));
        assert!(!verify_blocking(&hash, "wrong horse"));
        assert!(!verify_blocking("not a hash", "correct horse"));
    }

    #[test]
    fn tokens_are_random_and_hashed() {
        let (a, hash) = new_token(API_TOKEN_PREFIX).unwrap();
        let (b, _) = new_token(API_TOKEN_PREFIX).unwrap();
        assert!(a.starts_with("heron_") && a.len() > 40);
        assert_ne!(a, b);
        assert_eq!(token_hash(&a), hash);
    }

    #[test]
    fn locks_out_after_repeated_failures() {
        let passwords = Passwords::default();
        for _ in 0..MAX_FAILURES {
            passwords.check_lockout("eve").unwrap();
            passwords.record("eve", false);
        }
        assert!(matches!(
            passwords.check_lockout("eve"),
            Err(ApiError::TooManyRequests)
        ));
        passwords.check_lockout("alice").unwrap();
        passwords.record("eve", true);
        passwords.check_lockout("eve").unwrap();
    }

    #[test]
    fn session_cookies() {
        let open = session_cookie(Some("t0k"), true);
        let open = open.to_str().unwrap();
        assert!(open.starts_with("heron_session=t0k;"));
        assert!(open.contains("HttpOnly") && open.contains("SameSite=Strict"));
        assert!(open.ends_with("; Secure"));
        let closed = session_cookie(None, false);
        assert!(closed.to_str().unwrap().contains("Max-Age=0"));
    }
}
