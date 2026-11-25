use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pdm_core::PdmError;

/// Error wrapper for axum handlers. Carries the domain error and maps it to an HTTP status.
#[derive(Debug)]
pub struct AppError(pub PdmError);

impl From<PdmError> for AppError {
    fn from(e: PdmError) -> Self {
        Self(e)
    }
}

/// The unique constraint a database error broke, if it was one.
fn unique_violation(e: &sqlx::Error) -> Option<&str> {
    e.as_database_error()
        .filter(|d| d.is_unique_violation())
        .and_then(|d| d.constraint())
}

impl AppError {
    /// The status, code and message sent for this error. Two database errors that are the
    /// caller's, not the server's, are answered as such; they were 500s:
    /// - a query that expected a row and found none (e.g. releasing an NC program that isn't
    ///   there): 404;
    /// - a project name the organization already uses (creating or renaming): 409.
    fn parts(&self) -> (StatusCode, &'static str, String) {
        match &self.0 {
            PdmError::Db(sqlx::Error::RowNotFound) => {
                (StatusCode::NOT_FOUND, "not_found", "not found".to_string())
            }
            PdmError::Db(e) if unique_violation(e) == Some("projects_org_id_name_key") => (
                StatusCode::CONFLICT,
                "conflict",
                "conflict: a project with this name already exists".to_string(),
            ),
            e => (
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                e.code(),
                e.to_string(),
            ),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.parts();
        let body = serde_json::json!({
            "error": {
                "code": code,
                "message": message,
            }
        });
        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_row_is_not_found_and_other_errors_keep_their_status() {
        let (status, code, _) = AppError(PdmError::Db(sqlx::Error::RowNotFound)).parts();
        assert_eq!((status, code), (StatusCode::NOT_FOUND, "not_found"));
        let (status, code, _) = AppError(PdmError::Db(sqlx::Error::PoolTimedOut)).parts();
        assert_eq!((status, code), (StatusCode::INTERNAL_SERVER_ERROR, "db_error"));
        let (status, code, message) = AppError(PdmError::Conflict("x".into())).parts();
        assert_eq!((status, code, message.as_str()), (StatusCode::CONFLICT, "conflict", "conflict: x"));
    }
}
