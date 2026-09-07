//! OS 固有処理の入口。
//!
//! `unsafe` / FFI は `win_job` だけに閉じる (INV-8 / FR-P-68 / ADR-0011)。

pub mod win_job;
