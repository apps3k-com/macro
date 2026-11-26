use axum::extract::{Path, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use macro_authorization::{MacroAuthorizationExtractor, UserOrInternal};

use crate::api::context::AppState;
use crate::api::context::AuthorizationService;

#[derive(serde::Deserialize)]
pub struct Params {
    pub file_id: String,
}

#[utoipa::path(
    get,
    path = "/api/file/{file_id}/presigned-url",
    params(
        ("file_id" = String, Path, description = "File ID")
    ),
    responses(
        (status = 200, body=String, description = "Presigned URL for the file"),
        (status = 401, body=String),
        (status = 404, body=String),
        (status = 500, body=String)
    )
)]
#[tracing::instrument(
    skip(state, user),
    fields(actor = %user.acting_entity())
)]
pub async fn handle_get_presigned_url(
    State(state): State<AppState>,
    user: MacroAuthorizationExtractor<AuthorizationService, UserOrInternal>,
    Path(Params { file_id }): Path<Params>,
) -> Result<Response, Response> {
    Ok(signed_read(&state, &user, &file_id, Method::GET)
        .await?
        .into_response())
}

pub async fn content(
    State(state): State<AppState>,
    user: MacroAuthorizationExtractor<AuthorizationService, UserOrInternal>,
    Path(Params { file_id }): Path<Params>,
    method: Method,
) -> Result<Response, Response> {
    let url = signed_read(&state, &user, &file_id, method).await?;
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Redirect::temporary(&url),
    )
        .into_response())
}

async fn signed_read(
    state: &AppState,
    user: &MacroAuthorizationExtractor<AuthorizationService, UserOrInternal>,
    file_id: &str,
    method: Method,
) -> Result<String, Response> {
    // First get metadata to ensure file exists and is uploaded
    let metadata = super::read_access::readable_metadata(state, user, file_id).await?;
    // Check if file is uploaded
    if !metadata.is_uploaded {
        return Err((StatusCode::NOT_FOUND, "file not yet uploaded").into_response());
    }

    let s3_key = s3_key::StaticFileKey::new(file_id).to_key();
    state
        .storage_client
        .get_presigned_url(s3_key, method == Method::HEAD)
        .await
        .map_err(|err| {
            tracing::error!(error=?err, "error getting presigned URL from S3");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get presigned URL",
            )
                .into_response()
        })
}
