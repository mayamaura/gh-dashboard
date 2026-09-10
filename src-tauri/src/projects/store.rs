//! 永続化するのは「スキャン対象フォルダ」と「手動調整」の 2 つだけ (DR-02 / INV-5)。
//!
//! すべて同期関数。呼び出し側 (`commands.rs`) が `spawn_blocking` で包む (NFR-20)。
//!
//! 対応要求: FR-P-30〜32 / DR-01〜03

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension};

use crate::error::{AppError, AppResult};
use crate::util::path_key;

/// プロジェクトごとの手動調整 (FR-P-30)。
///
/// `Project.override_values` としてフロントにもそのまま渡す — フォームの初期値に
/// 「今保存されている上書き値」が要るため。これが無いと、表示名だけ直して保存した
/// ときに `working_dir_override` が黙って消える (行全体の置き換え upsert のため)。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectOverride {
    pub display_name: Option<String>,
    pub command_override: Option<String>,
    pub working_dir_override: Option<String>,
    pub sort_order: Option<i64>,
    pub hidden: bool,
    pub archived: bool,
}

/// 登録順のスキャン対象フォルダ一覧。
pub fn scan_folders(conn: &Connection) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM project_scan_folders ORDER BY id ASC")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// スキャン対象フォルダを追加する。
///
/// - 存在しないディレクトリは拒否する (FR-P-32 と同種の「実在検証」)
/// - 重複は UNIQUE 制約違反を分かりやすいメッセージに変換する (IR-03)
/// - 保存するのは `path_key` で正規化したパス
pub fn scan_folder_add(conn: &Connection, path: &str, now_ms: i64) -> AppResult<()> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::invalid("path", "パスを入力してください"));
    }
    if !std::path::Path::new(trimmed).is_dir() {
        return Err(AppError::invalid(
            "path",
            format!("指定したディレクトリが存在しません: {trimmed}"),
        ));
    }
    let Some(key) = path_key::path_key(trimmed) else {
        return Err(AppError::invalid("path", "パスを解釈できません"));
    };

    // 重複は path_key で判定する (`D:\Foo` と `d:/foo/` は同じフォルダ)。
    // 保存する文字列はディスク上の実際の大文字小文字 (表示に使う) なので、
    // UNIQUE 制約だけでは大文字小文字違いの二重登録を防げない。
    if scan_folders(conn)?
        .iter()
        .any(|existing| path_key::path_key(existing).as_deref() == Some(key.as_str()))
    {
        return Err(AppError::invalid(
            "path",
            format!("既に登録されています: {trimmed}"),
        ));
    }

    let stored = display_path(trimmed);
    let result = conn.execute(
        "INSERT INTO project_scan_folders (path, created_at) VALUES (?1, ?2)",
        rusqlite::params![stored, now_ms],
    );

    match result {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Err(AppError::invalid(
                "path",
                format!("既に登録されています: {trimmed}"),
            ))
        }
        Err(e) => Err(e.into()),
    }
}

/// 表示用のパス。**ディスク上の実際の大文字小文字**に揃える。
///
/// `path_key` は小文字化した比較用のキーであって、ユーザーに見せる文字列ではない
/// (`d:\masah\projects` と出ると実物と違って見える)。`canonicalize` は Windows で
/// `\\?\` 接頭辞を付けて返すので落とす。失敗したら入力をそのまま使う (エラーにしない)。
fn display_path(input: &str) -> String {
    match std::fs::canonicalize(input) {
        Ok(p) => {
            let s = p.to_string_lossy();
            s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
        }
        Err(_) => input.to_string(),
    }
}

/// スキャン対象フォルダを削除する。**無ければ何もしない** (エラーにしない)。
pub fn scan_folder_remove(conn: &Connection, path: &str) -> AppResult<()> {
    let Some(key) = path_key::path_key(path) else {
        // 解釈できないパスの削除要求は「そもそも登録されていない」と同義
        return Ok(());
    };
    // 保存値は表示用の実パスなので、path_key で突き合わせて消す
    for existing in scan_folders(conn)? {
        if path_key::path_key(&existing).as_deref() == Some(key.as_str()) {
            conn.execute(
                "DELETE FROM project_scan_folders WHERE path = ?1",
                rusqlite::params![existing],
            )?;
        }
    }
    Ok(())
}

/// 手動調整の一覧を `path_key` をキーに読む。
pub fn overrides(conn: &Connection) -> AppResult<HashMap<String, ProjectOverride>> {
    let mut stmt = conn.prepare(
        "SELECT path_key, display_name, command_override, working_dir_override, \
         sort_order, hidden, archived FROM project_overrides",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            ProjectOverride {
                display_name: row.get(1)?,
                command_override: row.get(2)?,
                working_dir_override: row.get(3)?,
                sort_order: row.get(4)?,
                hidden: row.get::<_, i64>(5)? != 0,
                archived: row.get::<_, i64>(6)? != 0,
            },
        ))
    })?;
    let mut out = HashMap::new();
    for r in rows {
        let (key, ov) = r?;
        out.insert(key, ov);
    }
    Ok(out)
}

/// 空文字・空白のみは `None` として扱う。
fn normalize_blank(s: Option<String>) -> Option<String> {
    s.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// 手動調整を保存する。**行全体を置き換える upsert** — リクエストの `None` はクリア (FR-P-31)。
///
/// `fields.hidden` / `fields.archived` は呼び出し側が `None` を `false` に
/// 変換済みの値を渡す (引数過多を避けるため `ProjectOverride` にまとめて受け取る)。
/// `working_dir_override` の実在検証は呼び出し側 (`commands.rs`) の責務 (FR-P-32)。
pub fn override_upsert(
    conn: &Connection,
    path_key: &str,
    fields: &ProjectOverride,
    now_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO project_overrides \
         (path_key, display_name, command_override, working_dir_override, sort_order, hidden, archived, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(path_key) DO UPDATE SET \
           display_name = excluded.display_name, \
           command_override = excluded.command_override, \
           working_dir_override = excluded.working_dir_override, \
           sort_order = excluded.sort_order, \
           hidden = excluded.hidden, \
           archived = excluded.archived, \
           updated_at = excluded.updated_at",
        rusqlite::params![
            path_key,
            normalize_blank(fields.display_name.clone()),
            normalize_blank(fields.command_override.clone()),
            normalize_blank(fields.working_dir_override.clone()),
            fields.sort_order,
            fields.hidden,
            fields.archived,
            now_ms,
        ],
    )?;
    Ok(())
}

/// 単一の `path_key` の手動調整を読む (working_dir_override の実在検証に使う)。
pub fn override_of(conn: &Connection, path_key: &str) -> AppResult<Option<ProjectOverride>> {
    let row = conn
        .query_row(
            "SELECT display_name, command_override, working_dir_override, \
             sort_order, hidden, archived FROM project_overrides WHERE path_key = ?1",
            [path_key],
            |row| {
                Ok(ProjectOverride {
                    display_name: row.get(0)?,
                    command_override: row.get(1)?,
                    working_dir_override: row.get(2)?,
                    sort_order: row.get(3)?,
                    hidden: row.get::<_, i64>(4)? != 0,
                    archived: row.get::<_, i64>(5)? != 0,
                })
            },
        )
        .optional()?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn add_dedup_remove_flow() {
        let conn = open_in_memory().unwrap();
        let dir = std::env::temp_dir();
        let dir_str = dir.to_string_lossy().to_string();

        scan_folder_add(&conn, &dir_str, NOW).unwrap();
        assert_eq!(scan_folders(&conn).unwrap().len(), 1);

        // 重複は分かりやすいメッセージで失敗する (IR-03)
        let err = scan_folder_add(&conn, &dir_str, NOW).unwrap_err();
        match err {
            AppError::InvalidInput { message, .. } => {
                assert!(message.contains("既に登録されています"), "{message}");
            }
            other => panic!("想定外のエラー: {other:?}"),
        }

        scan_folder_remove(&conn, &dir_str).unwrap();
        assert_eq!(scan_folders(&conn).unwrap().len(), 0);

        // 無いものの削除はエラーにしない
        scan_folder_remove(&conn, &dir_str).unwrap();
    }

    /// 大文字小文字・区切り違いは同じフォルダとして重複扱いし、保存値は実パスの
    /// 大文字小文字を保つ (表示用)。`d:\masah\projects` と小文字で出ないこと。
    #[test]
    fn add_keeps_disk_casing_and_dedupes_by_key() {
        let conn = open_in_memory().unwrap();
        let base = std::env::temp_dir().join(format!("gh-dashboard-store-{}", std::process::id()));
        let dir = base.join("MixedCase");
        std::fs::create_dir_all(&dir).unwrap();

        // 小文字 + スラッシュ + 末尾区切りで登録しても…
        let lowered = dir.to_string_lossy().to_lowercase().replace('\\', "/") + "/";
        scan_folder_add(&conn, &lowered, NOW).unwrap();

        // …保存値はディスク上の大文字小文字 (`MixedCase`) で、`\\?\` 接頭辞も付かない
        let stored = scan_folders(&conn).unwrap();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].ends_with("MixedCase"), "{}", stored[0]);
        assert!(!stored[0].starts_with(r"\\?\"), "{}", stored[0]);

        // 別表記の同じフォルダは重複として拒否される
        let err = scan_folder_add(&conn, &dir.to_string_lossy(), NOW).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput { .. }));

        // 削除も別表記で効く
        scan_folder_remove(&conn, &lowered).unwrap();
        assert!(scan_folders(&conn).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn add_rejects_nonexistent_path() {
        let conn = open_in_memory().unwrap();
        let err = scan_folder_add(&conn, "D:\\this-path-should-not-exist-xyz", NOW).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput { .. }));
    }

    #[test]
    fn override_upsert_replaces_entire_row() {
        let conn = open_in_memory().unwrap();
        override_upsert(
            &conn,
            "d:\\proj",
            &ProjectOverride {
                display_name: Some("表示名".to_string()),
                command_override: Some("npm run dev".to_string()),
                working_dir_override: Some("frontend".to_string()),
                sort_order: Some(3),
                hidden: true,
                archived: false,
            },
            NOW,
        )
        .unwrap();

        let ov = override_of(&conn, "d:\\proj").unwrap().unwrap();
        assert_eq!(ov.display_name.as_deref(), Some("表示名"));
        assert!(ov.hidden);

        // 2 回目に display_name を None で送ると消える (行全体を置き換える)
        override_upsert(&conn, "d:\\proj", &ProjectOverride::default(), NOW + 1).unwrap();
        let ov = override_of(&conn, "d:\\proj").unwrap().unwrap();
        assert_eq!(ov.display_name, None);
        assert_eq!(ov.command_override, None);
        assert_eq!(ov.sort_order, None);
        assert!(!ov.hidden, "hidden の None は false になる");
        assert!(!ov.archived);
    }

    #[test]
    fn blank_strings_are_treated_as_none() {
        let conn = open_in_memory().unwrap();
        override_upsert(
            &conn,
            "d:\\proj",
            &ProjectOverride {
                display_name: Some("   ".to_string()),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        let ov = override_of(&conn, "d:\\proj").unwrap().unwrap();
        assert_eq!(ov.display_name, None, "空白のみは None として保存される");
    }

    #[test]
    fn overrides_reads_all_rows_keyed_by_path_key() {
        let conn = open_in_memory().unwrap();
        override_upsert(
            &conn,
            "d:\\a",
            &ProjectOverride {
                display_name: Some("A".to_string()),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        override_upsert(
            &conn,
            "d:\\b",
            &ProjectOverride {
                display_name: Some("B".to_string()),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        let map = overrides(&conn).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map["d:\\a"].display_name.as_deref(), Some("A"));
    }
}
