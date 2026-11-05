// SPDX-License-Identifier: AGPL-3.0-or-later

//! The second step of signing in: after the password, a code of the app, a
//! recovery code or a passkey turns the pending sign-in into a session.
//! Failures count with the password's lockout.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use webauthn_rs::prelude::PublicKeyCredential;

use super::{CodeRequest, PendingSignIn, WebauthnCredential, WebauthnOptions, normalize_code};
use crate::app::AppState;
use crate::auth::cookie;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::users::User;

/// The cookie of a sign-in waiting for its second factor.
pub const PENDING_COOKIE: &str = "heron_pending";
/// Only the second step's routes get it.
pub const PENDING_PATH: &str = "/api/v1/auth/login";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(login_totp))
        .routes(routes!(login_recovery))
        .routes(routes!(login_webauthn_start))
        .routes(routes!(login_webauthn_finish))
}

/// The pending sign-in of the request, and its cookie's token.
fn pending(state: &AppState, headers: &HeaderMap) -> ApiResult<(PendingSignIn, String)> {
    let token = cookie(headers, PENDING_COOKIE).ok_or(ApiError::Unauthorized)?;
    let sign_in = state
        .two_factor
        .pending(token)
        .ok_or(ApiError::Unauthorized)?;
    state.passwords.check_lockout(&sign_in.username)?;
    Ok((sign_in, token.to_owned()))
}

fn failed(state: &AppState, sign_in: &PendingSignIn, factor: &'static str) -> ApiError {
    state.passwords.record(&sign_in.username, false);
    tracing::info!(username = %sign_in.username, factor, "failed second factor");
    metrics::counter!("heron_api_logins_total", "result" => "failed").increment(1);
    ApiError::Invalid("the code is wrong".to_owned())
}

async fn finish(
    state: &AppState,
    headers: &HeaderMap,
    sign_in: &PendingSignIn,
    token: &str,
) -> ApiResult<Response> {
    state.passwords.record(&sign_in.username, true);
    state.two_factor.finish_sign_in(token);
    crate::users::signed_in(state, headers, sign_in.user_id, &sign_in.username).await
}

/// Finishes signing in with a code of the authenticator app. Each code is
/// accepted once.
#[utoipa::path(post, path = "/auth/login/totp", tag = "auth", security(()),
    request_body = CodeRequest,
    responses((status = 200, body = User),
              (status = 401, body = ErrorBody, description = "No sign-in waiting, or it expired"),
              (status = 422, body = ErrorBody, description = "Wrong code"),
              (status = 429, body = ErrorBody, description = "Too many failures for this username")))]
async fn login_totp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CodeRequest>,
) -> ApiResult<Response> {
    let (sign_in, token) = pending(&state, &headers)?;
    let row: Option<(Vec<u8>, Option<u64>)> =
        sqlx::query_as("SELECT secret, last_step FROM user_totp WHERE user_id = ?")
            .bind(sign_in.user_id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((secret, last_step)) = row else {
        return Err(failed(&state, &sign_in, "totp"));
    };
    let Some(step) = super::totp::verify(&secret, &request.code, super::now_step(), last_step)
    else {
        return Err(failed(&state, &sign_in, "totp"));
    };
    // Taken in the database: two requests with the same code cannot both pass.
    let taken = sqlx::query(
        "UPDATE user_totp SET last_step = ?, last_used_at = NOW(6)
         WHERE user_id = ? AND (last_step IS NULL OR last_step < ?)",
    )
    .bind(step)
    .bind(sign_in.user_id)
    .bind(step)
    .execute(&state.pool)
    .await?;
    if taken.rows_affected() == 0 {
        return Err(failed(&state, &sign_in, "totp"));
    }
    finish(&state, &headers, &sign_in, &token).await
}

/// Finishes signing in with a recovery code, which then stops working.
#[utoipa::path(post, path = "/auth/login/recovery", tag = "auth", security(()),
    request_body = CodeRequest,
    responses((status = 200, body = User),
              (status = 401, body = ErrorBody, description = "No sign-in waiting, or it expired"),
              (status = 422, body = ErrorBody, description = "Wrong code"),
              (status = 429, body = ErrorBody, description = "Too many failures for this username")))]
async fn login_recovery(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CodeRequest>,
) -> ApiResult<Response> {
    let (sign_in, token) = pending(&state, &headers)?;
    let Some(code) = normalize_code(&request.code) else {
        return Err(failed(&state, &sign_in, "recovery"));
    };
    let rows: Vec<(u64, String)> = sqlx::query_as(
        "SELECT id, code_hash FROM user_recovery_codes WHERE user_id = ? AND used_at IS NULL",
    )
    .bind(sign_in.user_id)
    .fetch_all(&state.pool)
    .await?;
    let hashes = rows.iter().map(|(_, hash)| hash.clone()).collect();
    let Some(index) = state.passwords.find_code(hashes, &code).await? else {
        return Err(failed(&state, &sign_in, "recovery"));
    };
    let used = sqlx::query(
        "UPDATE user_recovery_codes SET used_at = NOW(6) WHERE id = ? AND used_at IS NULL",
    )
    .bind(rows[index].0)
    .execute(&state.pool)
    .await?;
    if used.rows_affected() == 0 {
        return Err(failed(&state, &sign_in, "recovery"));
    }
    let left = rows.len() - 1;
    tracing::warn!(username = %sign_in.username, left, "recovery code used");
    finish(&state, &headers, &sign_in, &token).await
}

/// Starts signing in with a passkey: the options for
/// `navigator.credentials.get()`.
#[utoipa::path(post, path = "/auth/login/webauthn/start", tag = "auth", security(()),
    responses((status = 200, body = WebauthnOptions),
              (status = 401, body = ErrorBody, description = "No sign-in waiting, or it expired"),
              (status = "4XX", body = ErrorBody)))]
async fn login_webauthn_start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<WebauthnOptions>> {
    let (sign_in, token) = pending(&state, &headers)?;
    let passkeys: Vec<_> = super::passkeys(&state.pool, sign_in.user_id)
        .await?
        .into_iter()
        .map(|(_, passkey)| passkey)
        .collect();
    if passkeys.is_empty() {
        return Err(ApiError::Conflict("you have no passkey".to_owned()));
    }
    let (challenge, authentication) = super::relying_party(&headers)?
        .start_passkey_authentication(&passkeys)
        .map_err(|error| ApiError::Invalid(format!("cannot use a passkey: {error}")))?;
    state.two_factor.set_challenge(&token, Some(authentication));
    Ok(Json(WebauthnOptions::from(challenge)?))
}

/// Finishes signing in with what the browser's passkey answered.
#[utoipa::path(post, path = "/auth/login/webauthn/finish", tag = "auth", security(()),
    request_body = WebauthnCredential,
    responses((status = 200, body = User),
              (status = 401, body = ErrorBody, description = "No sign-in waiting, or it expired"),
              (status = 422, body = ErrorBody, description = "The passkey was refused"),
              (status = 429, body = ErrorBody, description = "Too many failures for this username")))]
async fn login_webauthn_finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebauthnCredential>,
) -> ApiResult<Response> {
    let (sign_in, token) = pending(&state, &headers)?;
    // One answer per challenge.
    state.two_factor.set_challenge(&token, None);
    let Some(authentication) = sign_in.webauthn.clone() else {
        return Err(ApiError::Conflict(
            "start with POST /auth/login/webauthn/start".to_owned(),
        ));
    };
    let credential: PublicKeyCredential = serde_json::from_value(request.credential)
        .map_err(|error| ApiError::Invalid(format!("not a passkey answer: {error}")))?;
    let result = match super::relying_party(&headers)?
        .finish_passkey_authentication(&credential, &authentication)
    {
        Ok(result) => result,
        Err(error) => {
            tracing::info!(username = %sign_in.username, %error, "passkey refused");
            state.passwords.record(&sign_in.username, false);
            return Err(ApiError::Invalid("the passkey was refused".to_owned()));
        }
    };
    for (id, mut passkey) in super::passkeys(&state.pool, sign_in.user_id).await? {
        let Some(changed) = passkey.update_credential(&result) else {
            continue;
        };
        let json = changed
            .then(|| serde_json::to_string(&passkey))
            .transpose()
            .map_err(|error| ApiError::Internal(format!("passkey: {error}")))?;
        sqlx::query(
            "UPDATE user_webauthn SET last_used_at = NOW(6), passkey = COALESCE(?, passkey)
             WHERE id = ?",
        )
        .bind(json)
        .bind(id)
        .execute(&state.pool)
        .await?;
    }
    finish(&state, &headers, &sign_in, &token).await
}
