#![allow(unsafe_op_in_unsafe_fn)]
//! TokenSummarizer — 常驻 Windows 托盘的多 SSH 服务器 Agent Token 用量监控。
//!
//! 架构与 gpu_observer_rust（C# GPUObserver 的 Rust 镜像）保持一致：
//! Win32 + GDI+ 自绘 UI，逻辑像素 @96dpi，全部经 `S(v)=round(v*dpi/96)` 缩放。

pub mod app;
pub mod config;
pub mod i18n;
pub mod model;
pub mod monitoring;
pub mod parsing;
pub mod ssh;
pub mod store;
pub mod ui;
pub mod util;
