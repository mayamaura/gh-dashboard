//! バージョン付きマイグレーション (DR-04)。
//!
//! `user_version` を段数として使い、**上げるだけ**の前進マイグレーションにする。
//! ここに書いてよいテーブルは [データモデル](../../../docs/data-model.html) に
//! 載っているものだけ。**導出データのテーブルを足さない** (DR-02 / INV-5)。

use rusqlite::Connection;

/// 1 段のマイグレーション。`sql` は同一トランザクションで適用される。
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    /// 破壊的変更を含む段は適用前にバックアップを残す (DR-04)
    pub destructive: bool,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial",
    destructive: false,
    sql: include_str!("schema/0001_initial.sql"),
}];

/// 現在のスキーマバージョン。
pub fn current_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// 未適用のマイグレーションを順に適用する。
///
/// 破壊的な段が含まれる場合、呼び出し側は先にバックアップを取ること (DR-04)。
pub fn apply_all(conn: &mut Connection) -> rusqlite::Result<i64> {
    let mut version = current_version(conn)?;
    for m in MIGRATIONS {
        if m.version <= version {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        tx.pragma_update(None, "user_version", m.version)?;
        tx.commit()?;
        tracing::info!(version = m.version, name = m.name, "マイグレーションを適用");
        version = m.version;
    }
    Ok(version)
}

/// 適用予定の段に破壊的変更が含まれるか (バックアップ要否の判定)。
pub fn has_pending_destructive(from_version: i64) -> bool {
    MIGRATIONS
        .iter()
        .any(|m| m.version > from_version && m.destructive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    fn migrated() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_all(&mut conn).unwrap();
        conn
    }

    #[test]
    fn creates_all_tables() {
        let conn = migrated();
        let names = table_names(&conn);
        for expected in [
            "index_files",
            "project_overrides",
            "project_scan_folders",
            "quota_events",
            "quota_samples",
            "sessions",
            "settings",
            "subagent_runs",
            "turn_index",
        ] {
            assert!(names.contains(&expected.to_string()), "{expected} が無い");
        }
        assert_eq!(current_version(&conn).unwrap(), 1);
    }

    /// DR-02 / INV-5: 導出データのテーブルを作らない
    #[test]
    fn no_derived_data_tables() {
        let conn = migrated();
        let names = table_names(&conn);
        for forbidden in ["projects", "git_status", "dev_logs", "copilot_usage", "turn_bodies"] {
            assert!(
                !names.contains(&forbidden.to_string()),
                "{forbidden} は導出データ。永続化してはいけない (DR-02)"
            );
        }
    }

    /// DR-06 / INV-2: 認証情報を保存する列を作らない
    #[test]
    fn no_credential_columns() {
        let conn = migrated();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for t in tables {
            let mut cols = conn.prepare(&format!("PRAGMA table_info({t})")).unwrap();
            let names: Vec<String> = cols
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            for n in names {
                let lower = n.to_lowercase();
                // トークン "数" の列 (input_tokens 等) は正当なので、
                // 認証情報を思わせる語だけを弾く。
                for bad in [
                    "secret",
                    "password",
                    "credential",
                    "header",
                    "auth",
                    "api_key",
                    "apikey",
                    "access_token",
                    "refresh_token",
                    "bearer",
                ] {
                    assert!(!lower.contains(bad), "{t}.{n} は認証情報を含みうる (DR-06)");
                }
            }
        }
    }

    /// FR-C-06 / FR-C-02: 本文を保存する列が turn_index に無い
    #[test]
    fn turn_index_has_no_body_column() {
        let conn = migrated();
        let mut cols = conn.prepare("PRAGMA table_info(turn_index)").unwrap();
        let names: Vec<String> = cols
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(names.contains(&"byte_offset".to_string()));
        assert!(names.contains(&"preview".to_string()));
        assert!(
            !names.iter().any(|n| n == "body" || n == "content" || n == "text"),
            "本文を DB に複製してはいけない (FR-C-02 / INV-6)"
        );
    }

    /// FR-C-07: (file_path, byte_offset) の UNIQUE が二重適用の安全装置
    #[test]
    fn duplicate_turn_insert_is_ignored() {
        let conn = migrated();
        let sql = "INSERT OR IGNORE INTO turn_index (file_path, byte_offset, byte_length) VALUES (?1, ?2, ?3)";
        conn.execute(sql, rusqlite::params!["a.jsonl", 100i64, 20i64])
            .unwrap();
        let changed = conn
            .execute(sql, rusqlite::params!["a.jsonl", 100i64, 20i64])
            .unwrap();
        assert_eq!(changed, 0, "重複挿入は無視されること");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_index", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn apply_all_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(apply_all(&mut conn).unwrap(), 1);
        assert_eq!(apply_all(&mut conn).unwrap(), 1, "2 回目で壊れない");
    }

    #[test]
    fn migration_versions_are_unique_and_ascending() {
        let mut prev = 0;
        for m in MIGRATIONS {
            assert!(m.version > prev, "バージョンは昇順かつ重複しないこと");
            prev = m.version;
        }
    }
}
