use crate::api::context::AppState;
use crate::api::context::AuthorizationService;
use crate::model::api::*;
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use macro_authorization::{MacroAuthorizationExtractor, UserOrInternal};

#[derive(serde::Deserialize)]
pub struct Params {
    pub file_id: String,
}

#[utoipa::path(
    get,
    path = "/api/file/metadata/{file_id}",
    params(
    ("file_id" = String, Path, description = "File ID")
    ),
    responses(
    (status = 200, body=GetFileMetadataResponse),
    (status = 401, body=String),
    (status = 404, body=String),
    (status = 500, body=String)
    )
)]
#[tracing::instrument(
    skip(state, user),
    fields(actor = %user.acting_entity())
)]
pub async fn handle_get_metadata(
    State(state): State<AppState>,
    user: MacroAuthorizationExtractor<AuthorizationService, UserOrInternal>,
    Path(Params { file_id }): Path<Params>,
) -> Result<Response, Response> {
    let metadata = super::read_access::readable_metadata(&state, &user, &file_id).await?;
    let response = GetFileMetadataResponse::from(metadata);
    Ok((StatusCode::OK, Json(response)).into_response())
}
