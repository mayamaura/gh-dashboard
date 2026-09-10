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
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

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
}

pub use imp::JobHandle;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_can_be_created_and_dropped() {
        // 作って落とすだけ。配下にプロセスが無いので何も終了しない。
        let job = JobHandle::create().expect("Job Object を作れること");
        drop(job);
    }
}
