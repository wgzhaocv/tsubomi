//! 共有の入力バリデーション。リソースの display_name や CLI トークン名など、
//! ユーザが付ける「表示名」に使う(databases / tokens が共有)。

use crate::error::{AppError, AppResult};

/// 表示名を検証し、trim 済みを返す:空でない / `max_len` 文字以内 / 制御文字なし。
pub fn name(raw: &str, max_len: usize) -> AppResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("名前が空です".into()));
    }
    if trimmed.chars().count() > max_len {
        return Err(AppError::BadRequest(format!("名前は{max_len}文字以内です")));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(AppError::BadRequest("名前に制御文字を含めません".into()));
    }
    Ok(trimmed.to_owned())
}
