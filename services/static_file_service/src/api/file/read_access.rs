use crate::api::context::{AppState, AuthorizationService};
use crate::service::dynamodb::model::MetadataObject;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use macro_authorization::{MacroAuthorizationExtractor, UserOrInternal, UserOrInternalCaller};

pub(super) async fn readable_metadata(
    state: &AppState,
    user: &MacroAuthorizationExtractor<AuthorizationService, UserOrInternal>,
    file_id: &str,
) -> Result<MetadataObject, Response> {
    let not_found = || (StatusCode::NOT_FOUND, "file not found").into_response();
    let metadata = state
        .metadata_client
        .get_metadata(file_id)
        .await
        .map_err(|err| {
            tracing::error!(?err, "failed to read file metadata");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })?
        .ok_or_else(not_found)?;
    let reader: &str = user.authorization.user.macro_user_id.as_ref();
    if user.authorization.caller == UserOrInternalCaller::Internal || reader == metadata.owner_id {
        return Ok(metadata);
    }
    let pool = state.file_access_db.as_ref().ok_or_else(not_found)?;
    let id = uuid::Uuid::parse_str(file_id).map_err(|_| not_found())?;
    let visible: bool = sqlx::query_scalar(include_str!("read_channel_file.sql"))
        .bind(id)
        .bind(reader)
        .bind(&metadata.owner_id)
        .fetch_one(pool)
        .await
        .map_err(|err| {
            tracing::error!(?err, "failed to check channel file access");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        })?;
    if visible && metadata.is_uploaded {
        Ok(metadata)
    } else {
        Err(not_found())
    }
}
