use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("見つかりません")]
    NotFound,
    #[error("認証が必要です")]
    Unauthorized,
    #[error("権限がありません")]
    Forbidden,
    /// 403 だが理由を文案で伝える版(機能がこの環境で無効、等の**ポリシー拒否**)。固定文言の
    /// `Forbidden` と違い次の一手を載せられる。CLI 契約では 403 = `forbidden`(端末扱い = AI は
    /// 入力を直しても無駄なので再試行しない。`validation`(400)だと誤って再試行する)。
    #[error("{0}")]
    ForbiddenMsg(String),
    #[error("{0}")]
    BadRequest(String),
    /// 409。重複(同名リソースなど)。500 に潰さず、原因が分かる 4xx で返す。
    #[error("{0}")]
    Conflict(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// valkey(cache)の接続 / ACL 操作の失敗。infra 障害なので 500 群。
    #[error(transparent)]
    Redis(#[from] redis::RedisError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden | Self::ForbiddenMsg(_) => StatusCode::FORBIDDEN,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Sqlx(_)
            | Self::Reqwest(_)
            | Self::Json(_)
            | Self::Io(_)
            | Self::Redis(_)
            | Self::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!(error = ?self, "internal error");
            (status, "内部エラー").into_response()
        } else {
            (status, self.to_string()).into_response()
        }
    }
}
