//! Windows Job Object — **`unsafe` を書いてよい唯一のファイル** (INV-8 / FR-P-68)。
//!
//! dev サーバーの子プロセスをここで作った Job に割り当てておくと、
//! `KILL_ON_JOB_CLOSE` により**アプリが異常終了してもプロセスツリーごと OS が
//! 回収する** (FR-P-61)。アプリ側の明示的な停止処理が取りこぼしても残らない。
//!
//! 他のファイルからはこの型のメソッドだけを使うこと。`windows-sys` を
//! 直接 import しない。
//!
//! 対応要求: FR-P-61 / FR-P-68 / ADR-0011

#![allow(unsafe_code)]

#[cfg(windows)]
mod imp {
    use std::io;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
        PROCESS_TERMINATE,
    };

    /// `GetExitCodeProcess` が「まだ動いている」ときに返す値 (`STILL_ACTIVE`)。
    const STILL_RUNNING: u32 = 259;

    /// PID が生きているか (FR-C-71 / FR-C-42)。
    ///
    /// **2 秒ポーリング経路から呼ばれる。** プロセスを起動しない / ネットワークを
    /// 使わない (INV-4)。`OpenProcess` + `GetExitCodeProcess` の 2 回のシステム
    /// コールだけ。
    ///
    /// - 開けない = そのプロセスは存在しない → `false`
    /// - 開けたが終了コードが確定している → `false` (ハンドルが残る「ゾンビ」を
    ///   生存扱いしない)
    ///
    /// 既知の限界: 終了コードがちょうど 259 のプロセスは生存に見える。
    /// ponytail: 判別するには `SYNCHRONIZE` 権限での `WaitForSingleObject` が要る。
    /// IDE の接続表示 (FR-C-71) にその精度は不要なのでここまでにする。
    pub fn is_process_alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        // SAFETY: 参照権限だけを要求して開き、必ず閉じる。失敗は null で返る。
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return false;
        }
        let mut code: u32 = 0;
        // SAFETY: 直前に開いた有効なハンドルと、スタック上の u32 を渡す。
        let ok = unsafe { GetExitCodeProcess(process, &mut code) };
        // SAFETY: 上で開いたハンドル。以降使わない。
        unsafe { CloseHandle(process) };

        ok != 0 && code == STILL_RUNNING
    }

    /// プロセスツリーの道連れ終了を担う Job。アプリで 1 つだけ作る。
    pub struct JobHandle {
        handle: HANDLE,
    }

    // HANDLE は生ポインタだが、この型は所有権を 1 箇所に閉じ込めており、
    // 中で行う操作 (Assign / Close) はスレッドセーフ。
    unsafe impl Send for JobHandle {}
    unsafe impl Sync for JobHandle {}

    impl JobHandle {
        /// Job を作り、`KILL_ON_JOB_CLOSE` を設定する。
        pub fn create() -> io::Result<Self> {
            // SAFETY: 名前なし・既定のセキュリティ属性で Job を作るだけ。
            // 戻り値が null のときはエラーとして扱う。
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            // SAFETY: info は上で確保した有効な構造体で、サイズも実体と一致させている。
            let ok = unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                let err = io::Error::last_os_error();
                // SAFETY: 直前に作って有効なハンドル。
                unsafe { CloseHandle(handle) };
                return Err(err);
            }

            Ok(Self { handle })
        }

        /// 起動済みプロセスを Job に割り当てる。
        ///
        /// 子プロセスをさらに起こす dev サーバー (`npm run dev` → node → vite) でも、
        /// 子孫はすべてこの Job に属する。
        pub fn assign(&self, pid: u32) -> io::Result<()> {
            // SAFETY: 必要最小限の権限だけを要求して開き、必ず閉じる。
            let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
            if process.is_null() {
                return Err(io::Error::last_os_error());
            }

            // SAFETY: 両ハンドルともこの関数内で有効性を確認済み。
            let ok = unsafe { AssignProcessToJobObject(self.handle, process) };
            // SAFETY: 上で開いたハンドル。以降使わない。
            unsafe { CloseHandle(process) };

            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            // ここで閉じると KILL_ON_JOB_CLOSE により配下のプロセスが終了する。
            // アプリが異常終了した場合も OS がハンドルを閉じるので同じ効果になる。
            // SAFETY: create() で作った有効なハンドルを 1 度だけ閉じる。
            unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;

    /// Windows 以外では何もしない実体。v1 は Windows 専用 (要求 2.3)。
    pub struct JobHandle;

    impl JobHandle {
        pub fn create() -> io::Result<Self> {
            Ok(Self)
        }

        pub fn assign(&self, _pid: u32) -> io::Result<()> {
            Ok(())
        }
    }

    /// Windows 以外では判定できない。**生きていると偽らない** (NFR-43)。
    pub fn is_process_alive(_pid: u32) -> bool {
        false
    }
}

pub use imp::{is_process_alive, JobHandle};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_can_be_created_and_dropped() {
        // 作って落とすだけ。配下にプロセスが無いので何も終了しない。
        let job = JobHandle::create().expect("Job Object を作れること");
        drop(job);
    }

    /// FR-C-71: 生きている PID と、まず存在しない PID を取り違えない
    #[cfg(windows)]
    #[test]
    fn own_process_is_alive_and_bogus_pid_is_not() {
        assert!(is_process_alive(std::process::id()));
        assert!(!is_process_alive(0), "PID 0 は開けても意味がない");
        // 予約領域の外の PID。割り当てられている可能性は無視できるほど低い
        assert!(!is_process_alive(0xFFFF_FFF0));
    }
}
