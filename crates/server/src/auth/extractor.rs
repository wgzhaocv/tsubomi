use crate::auth::AuthCtx;
use crate::error::AppError;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

impl<S> FromRequestParts<S> for AuthCtx
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthCtx>()
            .cloned()
            .ok_or(AppError::Unauthorized)
    }
}
