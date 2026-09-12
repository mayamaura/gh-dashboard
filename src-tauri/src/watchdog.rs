//! UI (メイン) スレッドの応答性 watchdog (NFR-21)。
//!
//! # 何を監視していて、何を監視していないか
//!
//! 監視対象は **ネイティブのイベントループ (メインスレッド) が回っているか** だけ。
//! 段階 4〜7 の重い処理 — 差分インデックス / ライブ監視の DB 読み / 利用枠取得 —
//! は `spawn_blocking` で別スレッドプールに逃がしてあるので、そちらが何秒詰まっても
//! メインスレッドは応答し続ける。つまりここが警告を出すのは
//! 「UI スレッドで DB / 外部プロセス / ロック / ネットワークを触った」(INV-10 違反)
//! か、WebView2 側がメインスレッドを掴んだときだけ。**遅い処理の検出器ではない。**
//!
//! # なぜ `RunEvent::MainEventsCleared` の heartbeat ではないのか
//!
//! 素直な設計は「`MainEventsCleared` のハンドラで最終生存時刻を更新し、別スレッドが
//! 3 秒ごとに古さを見る」だが、**これは常に誤検知する**。
//! `tauri-runtime-wry` はイベントループを `ControlFlow::Wait` で回すため
//! (tauri-runtime-wry 2.11 `src/lib.rs`)、入力も OS イベントも無いアイドル時は
//! ループが `GetMessage` で寝る。常駐ダッシュボードでは「数分間 `MainEventsCleared`
//! が発火しない」のが正常な状態であり、最終発火時刻ではアイドルとハングを区別できない。
//!
//! そこで ping 方式を採る。`AppHandle::run_on_main_thread` は別スレッドから呼ぶと
//! `EventLoopProxy::send_event` でイベントループを叩き起こすので、
//! **寝ているだけなら即座に応答が返る**。返らない = 本当に詰まっている。
//! 副作用としてイベントループを 3 秒ごとに 1 回起こすが、処理は
//! 「チャネルに `()` を送る」だけで、ポーリング経路に何かを足すわけでもない (INV-4)。

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tauri::AppHandle;

/// ping と ping の間隔。
const PING_INTERVAL: Duration = Duration::from_secs(3);

/// この時間だけ応答が無ければ「応答なし」として記録する (NFR-21 の目安 3 秒)。
const HANG_THRESHOLD: Duration = Duration::from_secs(3);

/// watchdog スレッドを起動する。失敗しても本体は止めない。
pub fn spawn(handle: AppHandle) {
    if let Err(error) = thread::Builder::new()
        .name("ui-watchdog".into())
        .spawn(move || watch(handle))
    {
        tracing::warn!(%error, "UI 応答性 watchdog を起動できません");
    }
}

/// 3 秒ごとにメインスレッドへ ping を投げ、3 秒以内に戻らなければ記録する。
///
/// ハング中は「3 秒待つ → 3 秒 sleep」で約 6 秒に 1 行になるため、
/// 別途レート制限を持たなくてもログは溢れない。
fn watch(handle: AppHandle) {
    // ハングが始まった時刻。応答が戻ったら None に戻す。
    let mut hung_since: Option<Instant> = None;

    loop {
        thread::sleep(PING_INTERVAL);

        let (pong_tx, pong_rx) = mpsc::channel();
        let sent_at = Instant::now();

        // イベントループが閉じた後は Err になる。アプリ終了なので監視も畳む。
        if handle
            .run_on_main_thread(move || {
                let _ = pong_tx.send(());
            })
            .is_err()
        {
            return;
        }

        match pong_rx.recv_timeout(HANG_THRESHOLD) {
            Ok(()) => {
                if let Some(since) = hung_since.take() {
                    tracing::warn!(
                        unresponsive_ms = since.elapsed().as_millis(),
                        "UI (メイン) スレッドが応答を再開しました"
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // 応答が来なかった ping の closure は生き残るが、受信側を捨てるので
                // 遅れて実行されても送信が失敗するだけで害はない。
                let since = *hung_since.get_or_insert(sent_at);
                tracing::warn!(
                    unresponsive_ms = since.elapsed().as_millis(),
                    threshold_ms = HANG_THRESHOLD.as_millis(),
                    "UI (メイン) スレッドが応答しません (UI スレッドでの DB / IO / ロックを疑う: INV-10)"
                );
            }
            // 送信側 closure が実行されずに捨てられた = イベントループ終了。
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}
