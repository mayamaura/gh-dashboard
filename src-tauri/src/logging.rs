//! ログのファイル出力 + サイズベース・ローテーション (T-X.1 / NFR-22)。
//!
//! `tracing-appender` の `RollingFileAppender` は時間ベース (毎日/毎時) の
//! ローテーションしか無く、目安「5MB×5世代」のサイズベースには使えない。
//! 新規の依存クレートを足さず (NFR-10)、`std::io::Write` を実装した最小の
//! ローテーション書き込み器をここに自前で用意する。
//!
//! 保存先は `%LOCALAPPDATA%\gh-dashboard\logs\app.log` (`db::default_db_path`
//! と同じ流儀)。**書き込み失敗でアプリを落とさない** — I/O エラーは黙って
//! 捨てる (panic しない)。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 1 世代の上限 (目安 5MB)。
const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// 保持世代数。`app.log` 込みで 5 (`app.log` + `app.log.1`〜`.4`)。
const MAX_GENERATIONS: u32 = 5;

struct Inner {
    dir: PathBuf,
    file: File,
    written: u64,
}

impl Inner {
    fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join("app.log");
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            written,
        })
    }

    /// `app.log.3` → `app.log.4` … `app.log` → `app.log.1`。5 世代目は破棄する。
    fn rotate(&mut self) -> io::Result<()> {
        for gen in (1..MAX_GENERATIONS).rev() {
            let from = self.dir.join(format!("app.log.{gen}"));
            let to = self.dir.join(format!("app.log.{}", gen + 1));
            if from.exists() {
                let _ = fs::rename(&from, &to);
            }
        }
        let base = self.dir.join("app.log");
        if base.exists() {
            let _ = fs::rename(&base, self.dir.join("app.log.1"));
        }
        self.file = OpenOptions::new().create(true).append(true).open(&base)?;
        self.written = 0;
        Ok(())
    }
}

impl Write for Inner {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written >= MAX_BYTES {
            // ローテーションに失敗しても書き込み自体は続ける (ログでアプリを落とさない)
            let _ = self.rotate();
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// `tracing_subscriber::fmt::MakeWriter` に渡すクローン可能な書き込み器。
///
/// 開けなかった場合は `None` を保持し、以後の書き込みは黙って捨てる
/// (ログ機構自体の不調でアプリを落とさない)。
#[derive(Clone)]
pub struct RotatingFileWriter(Arc<Mutex<Option<Inner>>>);

impl RotatingFileWriter {
    pub fn new(dir: PathBuf) -> Self {
        let inner = match Inner::open(&dir) {
            Ok(i) => Some(i),
            Err(e) => {
                eprintln!("ログファイルを開けません ({}): {e}", dir.display());
                None
            }
        };
        Self(Arc::new(Mutex::new(inner)))
    }
}

impl Write for RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.0.lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_mut() {
            Some(inner) => inner.write(buf),
            None => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut guard = self.0.lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_mut() {
            Some(inner) => inner.flush(),
            None => Ok(()),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RotatingFileWriter {
    type Writer = RotatingFileWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_when_exceeding_the_size_limit() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("ghd_log_test_{}_{}", std::process::id(), unique_suffix()));
        let mut writer = RotatingFileWriter::new(dir.clone());

        // 5MB を明確に超える量を書き、ローテーションが起きることを確認する
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..6 {
            writer.write_all(&chunk).unwrap();
        }
        writer.flush().unwrap();

        assert!(dir.join("app.log").exists());
        assert!(dir.join("app.log.1").exists(), "6MB 書けば 1 回はローテーションする");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_failure_does_not_panic_when_directory_is_unusable() {
        // ファイルをディレクトリの代わりに置いて open を失敗させる
        let mut dir = std::env::temp_dir();
        dir.push(format!("ghd_log_test_bad_{}_{}", std::process::id(), unique_suffix()));
        fs::write(&dir, b"not a directory").unwrap();

        let mut writer = RotatingFileWriter::new(dir.clone());
        // 開けなくても write は panic せず、成功したことにして捨てる
        assert!(writer.write_all(b"hello").is_ok());

        let _ = fs::remove_file(&dir);
    }

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
