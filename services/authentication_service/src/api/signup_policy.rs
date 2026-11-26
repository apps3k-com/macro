#[cfg(test)]
mod test;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use model::response::ErrorResponse;

const SIGNUP_FORBIDDEN_MESSAGE: &str = "signup is not allowed";

/// Map an expected signup-policy rejection to a generic API response.
pub(crate) fn signup_forbidden_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ErrorResponse {
            message: SIGNUP_FORBIDDEN_MESSAGE.into(),
        }),
    )
        .into_response()
}

/// Require an outstanding invitation before issuing a first login or accepting
/// the identity provider's user-creation webhook. Existing logins skip this gate.
pub(crate) async fn require_invited_signup(
    ctx: &crate::api::context::ApiContext,
    email: &str,
) -> Result<(), Response> {
    use macro_user_id::user_id::MacroUserIdStr;
    use teams::domain::team_repo::TeamService;

    if !ctx.require_team_invite {
        return Ok(());
    }
    let user = MacroUserIdStr::try_from_email(email).map_err(|_| signup_forbidden_response())?;
    let invites = ctx
        .teams_service
        .get_user_invites(&user)
        .await
        .map_err(|error| {
            tracing::error!(?error, "checking signup invitation failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        })?;
    if invites.is_empty() {
        return Err(signup_forbidden_response());
    }
    Ok(())
}
