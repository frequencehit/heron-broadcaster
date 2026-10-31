// SPDX-License-Identifier: AGPL-3.0-or-later

//! API errors, answered as JSON: `{"error": "not_found", "message": "…"}`.

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

/// Body of every error response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    /// Machine-readable code: `invalid_request`, `unauthorized`, `forbidden`,
    /// `not_found`, `conflict`, `invalid`, `too_many_requests`,
    /// `engine_unavailable` or `internal`.
    pub error: String,
    /// For people.
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("sign in first")]
    Unauthorized,
    #[error("your role does not allow this")]
    Forbidden,
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    /// The request is well-formed but its content is not acceptable.
    #[error("{0}")]
    Invalid(String),
    #[error("too many attempts, try again in a minute")]
    TooManyRequests,
    #[error("the engine cannot be reached")]
    EngineUnavailable,
    /// Logged; the client only gets a generic message.
    #[error("internal error: {0}")]
    Internal(String),
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn not_found(what: impl std::fmt::Display) -> Self {
        Self::NotFound(format!("{what} not found"))
    }

    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Self::Invalid(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid"),
            Self::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "too_many_requests"),
            Self::EngineUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "engine_unavailable"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.parts();
        let message = match &self {
            Self::Internal(reason) => {
                tracing::error!(%reason, "request failed");
                "internal error, see the API logs".to_owned()
            }
            other => other.to_string(),
        };
        let body = ErrorBody {
            error: code.to_owned(),
            message,
        };
        (status, axum::Json(body)).into_response()
    }
}

/// Answers the client errors axum writes itself as plain text, or with no
/// body (a body that cannot be read, a parameter of the wrong type, an
/// unknown route or method), as [`ErrorBody`]s: clients show their message.
pub async fn json_client_errors(response: Response) -> Response {
    let status = response.status();
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if !status.is_client_error() || is_json {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let text = axum::body::to_bytes(body, 16 * 1024)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_default();
    let code = match status {
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::CONFLICT => "conflict",
        StatusCode::UNPROCESSABLE_ENTITY => "invalid",
        StatusCode::TOO_MANY_REQUESTS => "too_many_requests",
        _ => "invalid_request",
    };
    let message = if text.is_empty() {
        status
            .canonical_reason()
            .unwrap_or("invalid request")
            .to_lowercase()
    } else {
        text
    };
    parts.headers.remove(header::CONTENT_TYPE);
    parts.headers.remove(header::CONTENT_LENGTH);
    let body = ErrorBody {
        error: code.to_owned(),
        message,
    };
    let (answer_parts, answer_body) = axum::Json(body).into_response().into_parts();
    parts.headers.extend(answer_parts.headers);
    Response::from_parts(parts, answer_body)
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(format!("database: {error}"))
    }
}

impl From<tonic::Status> for ApiError {
    fn from(status: tonic::Status) -> Self {
        use tonic::Code;
        match status.code() {
            Code::Unavailable | Code::DeadlineExceeded | Code::Cancelled | Code::Unknown => {
                Self::EngineUnavailable
            }
            Code::InvalidArgument | Code::FailedPrecondition | Code::OutOfRange => {
                Self::Invalid(status.message().to_owned())
            }
            Code::NotFound => Self::NotFound(status.message().to_owned()),
            _ => Self::Internal(format!("engine: {status}")),
        }
    }
}

/// Whether a database error is a duplicate key (a name already used).
pub fn is_duplicate(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|error| error.is_unique_violation())
}

/// Whether a database error is a foreign key still referenced.
pub fn is_still_used(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|error| error.is_foreign_key_violation())
}
