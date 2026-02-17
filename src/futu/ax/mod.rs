//! macOS Accessibility API 安全封装层
//!
//! 提供对 CoreFoundation/AXUIElement 的类型安全 Rust 接口，
//! 所有内存管理通过 RAII 自动处理。
//!
//! # 使用示例
//!
//! ```rust,no_run
//! use crate::futu::ax::{Application, Element};
//!
//! // 查找并连接应用
//! let pid = Application::find_futu_pid()?;
//! let app = Application::new(pid)?;
//!
//! // 遍历窗口
//! for window in app.windows()? {
//!     println!("Window: {:?}", window.title());
//! }
//!
//! // 查找特定元素
//! if let Some(button) = app.element().find_by_role("AXButton", 5) {
//!     button.click()?;
//! }
//! ```

mod app;
mod element;
mod error;
mod ffi;
mod types;

pub mod action;

#[cfg(test)]
mod tests;

// 公开导出
pub use app::Application;
pub use element::Element;
pub use error::AxResult;
pub use types::{AxElement, CfType, Rect};

/// 检查辅助功能权限
pub fn check_permission() -> bool {
    Application::check_permission()
}

/// 请求辅助功能权限
pub fn request_permission() -> bool {
    Application::request_permission()
}

/// 便捷函数：连接到富途牛牛
pub fn connect_futu() -> AxResult<Application> {
    let pid = Application::find_futu_pid()?;
    Application::new(pid)
}
