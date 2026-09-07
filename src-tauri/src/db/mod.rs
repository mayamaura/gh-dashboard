//! ローカル DB。
//!
//! **UI スレッドから触らない** (NFR-20)。すべての呼び出しは `spawn_blocking` の
//! 中で行う。ここに置くのはスキーマと接続の世話だけで、集計クエリは各機能側に置く。
//!
//! 対応要求: DR-01 / DR-04 / DR-05 / NFR-20

pub mod migrations;

use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// DB ファイルの置き場所。`%LOCALAPPDATA%\gh-dashboard\app.db`
pub fn default_db_path() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("gh-dashboard").join("app.db"))
}

/// 接続を開き、必要ならマイグレーションを適用する。
///
/// 破壊的な段が控えている場合は、適用前にバックアップを残す (DR-04)。
pub fn open(path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut conn = Connection::open(path)?;
    configure(&conn)?;

    let from = migrations::current_version(&conn)?;
    if migrations::has_pending_destructive(from) && path.exists() {
        let backup = path.with_extension(format!("bak-{from}"));
        if let Err(e) = std::fs::copy(path, &backup) {
            tracing::warn!(?backup, error = %e, "適用前バックアップに失敗");
        } else {
            tracing::info!(?backup, "破壊的マイグレーションの前にバックアップを取得");
        }
    }

    let to = migrations::apply_all(&mut conn)?;
    if from != to {
        tracing::info!(from, to, "スキーマを更新");
    }
    Ok(conn)
}

/// テスト用のインメモリ接続。
pub fn open_in_memory() -> anyhow::Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrations::apply_all(&mut conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

/// 他アプリが所有する SQLite を読むときの入口 (DR-05)。
///
/// **書き込みロックを取らない。** 読み取り専用で開けなければコピーしてから読む。
/// `session-store.db` を読むかどうかは OQ-05 の結論待ち。
pub fn open_foreign_readonly(path: &Path) -> anyhow::Result<Connection> {
    use rusqlite::OpenFlags;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Ok(Connection::open_with_flags(path, flags)?)
}

/// 保持期間を過ぎた `quota_samples` を間引く (DR-07 / FR-C-94)。
///
/// **このテーブルへの書き込みを実装しないなら、この関数も呼ばないこと。**
/// 間引きだけが動く未使用テーブルを残さない。
pub const QUOTA_SAMPLE_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

pub fn prune_quota_samples(conn: &Connection, now_ms: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM quota_samples WHERE observed_at < ?1",
        [now_ms - QUOTA_SAMPLE_RETENTION_MS],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn in_memory_db_is_migrated() {
        let conn = open_in_memory().unwrap();
        assert_eq!(migrations::current_version(&conn).unwrap(), 1);
    }

    #[test]
    fn prune_removes_only_old_samples() {
        let conn = open_in_memory().unwrap();
        let insert = "INSERT INTO quota_samples (received_at, observed_at, source, quota_kind) VALUES (?1, ?2, 'sdk', 'monthly_credits')";
        // 31 日前 / 1 日前
        conn.execute(insert, [NOW, NOW - 31 * 24 * 3_600_000]).unwrap();
        conn.execute(insert, [NOW, NOW - 24 * 3_600_000]).unwrap();

        let removed = prune_quota_samples(&conn, NOW).unwrap();
        assert_eq!(removed, 1);

        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM quota_samples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1, "保持期間内の行を消してはいけない");
    }
}
