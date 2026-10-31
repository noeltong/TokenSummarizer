//! UI 层：GDI+ 封装、主题、绘制助手、图表、气泡主窗口与对话框。

pub mod alias_edit;
pub mod button;
pub mod chart;
pub mod content;
pub mod dialog;
pub mod drawing;
pub mod gdip;
pub mod server_edit;
pub mod settings;
pub mod tab_strip;
pub mod theme;
#[path = "main_window.rs"]
pub mod window;
