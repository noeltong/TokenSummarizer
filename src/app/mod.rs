//! Win32 消息常量与托盘/单实例/自启。

pub mod autostart;
pub mod single_instance;
pub mod tray;

use windows::Win32::UI::WindowsAndMessaging::WM_APP;

/// 托盘回调消息（Shell_NotifyIconW uCallbackMessage）。
pub const WM_APP_TRAY: u32 = WM_APP;
/// 后台数据更新 → UI 重绘。
pub const WM_APP_UPDATE: u32 = WM_APP + 1;
/// 唤起已有实例（单实例）/ 通知点击定位。
pub const WM_APP_SHOW: u32 = WM_APP + 3;
/// 子对话框关闭 → 恢复主窗口失焦隐藏。
pub const WM_APP_DIALOG_CLOSED: u32 = WM_APP + 8;
/// 打开"添加服务器"。
pub const WM_APP_OPEN_ADD_SERVER: u32 = WM_APP + 9;
/// 打开"模型别名"对话框。
pub const WM_APP_OPEN_ALIASES: u32 = WM_APP + 10;
/// 打开设置（wParam 忽略）。
pub const WM_APP_OPEN_SETTINGS: u32 = WM_APP + 11;
/// 选择选项卡（wParam = 索引：0 合计，1 本机，2.. 服务器）。
pub const WM_APP_SELECT_TAB: u32 = WM_APP + 12;
