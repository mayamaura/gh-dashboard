//! IPC 境界のエラー型。
//!
//! **`Result<T, String>` を返さない。** UI が分岐できる形で返し、ユーザーが次に
//! 何をすればよいかを `hint` / `how_to_fix` に載せる。埋められないなら `None` に
//! する — **適当な文言で埋めない**。
//!
//! 対応要求: FR-P-73 / FR-C-83 / NFR-24

use serde::Serialize;

#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppError {
    #[error("{what} が見つかりません")]
    NotFound { what: String },

    #[error("{field}: {message}")]
    InvalidInput { field: String, message: String },

    #[error("ファイル操作に失敗しました: {message}")]
    Io { message: String },

    #[error("データベース操作に失敗しました: {message}")]
    Db { message: String },

    /// 外部ツールの起動失敗。`hint` に「PATH に無い」等の具体的な対処を書く (FR-P-73)
    #[error("{tool} の起動に失敗しました: {message}")]
    External {
        tool: String,
        message: String,
        hint: Option<String>,
    },

    /// 取得不可。**エラーではあるが異常ではない**ケースに使う (FR-C-83)
    #[error("{reason}")]
    Unavailable {
        reason: String,
        how_to_fix: Option<String>,
    },
}

impl AppError {
    pub fn not_found(what: impl Into<String>) -> Self {
        Self::NotFound { what: what.into() }
    }

    pub fn invalid(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidInput {
            field: field.into(),
            message: message.into(),
        }
    }

    pub fn external(tool: impl Into<String>, message: impl Into<String>) -> Self {
        Self::External {
            tool: tool.into(),
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, text: impl Into<String>) -> Self {
        if let Self::External { hint, .. } = &mut self {
            *hint = Some(text.into());
        }
        self
    }

    pub fn unavailable(reason: impl Into<String>, how_to_fix: Option<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
            how_to_fix,
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            message: e.to_string(),
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db {
            message: e.to_string(),
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::Io {
            message: e.to_string(),
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_with_kind_tag() {
        let json = serde_json::to_string(&AppError::not_found("プロジェクト")).unwrap();
        assert!(json.contains("\"kind\":\"not_found\""));
    }

    #[test]
    fn hint_is_none_unless_set() {
        let e = AppError::external("code", "spawn failed");
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"hint\":null"), "埋められないなら null のまま");

        let e = AppError::external("code", "spawn failed")
            .with_hint("VS Code が PATH にありません");
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("PATH"));
    }
}
