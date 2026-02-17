//! 应用程序级别的 AX 操作

use super::element::Element;
use super::error::{AxError, AxResult};
use super::ffi::*;
use super::types::{AxElement, Rect};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use std::process::Command;
use tracing::{debug, info, warn};

/// 应用程序表示
///
/// 包装特定 PID 的应用程序的 AXUIElement
pub struct Application {
    element: Element,
    pid: i32,
}

impl Application {
    /// 从 PID 创建应用程序元素
    pub fn new(pid: i32) -> AxResult<Self> {
        let app_ref = unsafe { AXUIElementCreateApplication(pid) };
        if app_ref.is_null() {
            return Err(AxError::AppNotAccessible);
        }

        let element = unsafe { Element::from_raw(app_ref)? };
        Ok(Self { element, pid })
    }

    /// 获取进程 ID
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// 获取应用元素
    pub fn element(&self) -> &Element {
        &self.element
    }

    /// 获取应用名称
    pub fn name(&self) -> Option<String> {
        self.element.title()
    }

    /// 获取所有窗口
    pub fn windows(&self) -> AxResult<Vec<Element>> {
        self.element.windows()
    }

    /// 获取主窗口
    pub fn main_window(&self) -> AxResult<Element> {
        match self.element.attribute("AXMainWindow") {
            Ok(super::types::CfType::Element(e)) => {
                // 创建新的 Element，增加引用计数
                unsafe {
                    AxElement::retain(e.as_ptr())
                        .map(Element::from_wrapper)
                        .ok_or(AxError::NullPointer)
                }
            }
            Ok(_) => Err(AxError::TypeMismatch {
                expected: "element".to_string(),
                actual: "other".to_string(),
            }),
            Err(e) => Err(e),
        }
    }

    /// 获取聚焦窗口
    pub fn focused_window(&self) -> AxResult<Element> {
        match self.element.attribute("AXFocusedWindow") {
            Ok(super::types::CfType::Element(e)) => unsafe {
                AxElement::retain(e.as_ptr())
                    .map(Element::from_wrapper)
                    .ok_or(AxError::NullPointer)
            },
            Ok(_) => Err(AxError::TypeMismatch {
                expected: "element".to_string(),
                actual: "other".to_string(),
            }),
            Err(e) => Err(e),
        }
    }

    /// 判断应用是否隐藏
    pub fn is_hidden(&self) -> bool {
        match self.element.attribute("AXHidden") {
            Ok(super::types::CfType::Boolean(b)) => b,
            _ => false,
        }
    }

    /// 判断应用是否为主应用
    pub fn is_frontmost(&self) -> bool {
        match self.element.attribute("AXFrontmost") {
            Ok(super::types::CfType::Boolean(b)) => b,
            _ => false,
        }
    }

    /// 激活应用（前置）
    pub fn activate(&self) -> AxResult<()> {
        self.element.perform_action("AXRaise")
    }

    /// 隐藏应用
    pub fn hide(&self) -> AxResult<()> {
        // macOS AX API 没有直接隐藏的方法，使用 AppleScript
        let script = format!(
            r#"tell application "System Events" to set visible of (processes whose unix id is {}) to false"#,
            self.pid
        );
        let output = Command::new("osascript").args(["-e", &script]).output();

        match output {
            Ok(result) if result.status.success() => Ok(()),
            Ok(result) => Err(AxError::Other(String::from_utf8_lossy(&result.stderr).to_string())),
            Err(e) => Err(AxError::Other(e.to_string())),
        }
    }

    /// 显示应用
    pub fn show(&self) -> AxResult<()> {
        let script = format!(
            r#"tell application "System Events" to set visible of (processes whose unix id is {}) to true"#,
            self.pid
        );
        let output = Command::new("osascript").args(["-e", &script]).output();

        match output {
            Ok(result) if result.status.success() => Ok(()),
            Ok(result) => Err(AxError::Other(String::from_utf8_lossy(&result.stderr).to_string())),
            Err(e) => Err(AxError::Other(e.to_string())),
        }
    }

    /// 终止应用
    pub fn terminate(&self) -> AxResult<()> {
        self.element.perform_action("AXCancel")
    }

    /// 在窗口中查找特定标识符的元素
    ///
    /// 从所有窗口中搜索，返回第一个匹配的元素
    pub fn find_element_by_identifier(&self, identifier: &str) -> AxResult<Element> {
        let windows = self.windows()?;

        for window in windows {
            if let Some(found) = window.find_by_identifier(identifier, 10) {
                return Ok(found);
            }
        }

        Err(AxError::ElementNotFound(identifier.to_string()))
    }

    /// 在指定窗口中查找特定标识符的元素
    pub fn find_element_by_identifier_in_window(&self, window_idx: usize, identifier: &str) -> AxResult<Element> {
        let windows = self.windows()?;

        if window_idx >= windows.len() {
            return Err(AxError::WindowNotFound);
        }

        windows[window_idx]
            .find_by_identifier(identifier, 10)
            .ok_or_else(|| AxError::ElementNotFound(identifier.to_string()))
    }

    /// 查找特定角色的所有元素
    pub fn find_elements_by_role(&self, role: &str) -> Vec<Element> {
        let mut results = Vec::new();

        if let Ok(windows) = self.windows() {
            for window in windows {
                let mut found = window.find_all_by_role(role, 10);
                results.append(&mut found);
            }
        }

        results
    }

    /// 获取应用树的调试信息
    pub fn dump_tree(&self, max_depth: usize) -> String {
        let mut output = format!("Application (PID: {})\n", self.pid);

        if let Ok(windows) = self.windows() {
            output.push_str(&format!("Windows: {}\n", windows.len()));
            for (i, window) in windows.iter().enumerate().take(5) {
                output.push_str(&format!("\n--- Window {} ---\n", i));
                output.push_str(&window.dump_tree(max_depth));
            }
        } else {
            output.push_str("Failed to get windows\n");
        }

        output
    }

    /// 检查辅助功能权限
    pub fn check_permission() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    /// 请求辅助功能权限（弹出系统对话框）
    pub fn request_permission() -> bool {
        unsafe {
            let key = CFString::new("AXTrustedCheckOptionPrompt");
            let value = CFBoolean::true_value();
            let pairs = [(key, value)];
            let options = CFDictionary::from_CFType_pairs(&pairs);
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
        }
    }

    /// 查找应用进程 ID
    ///
    /// 通过进程名匹配查找 PID
    pub fn find_pid_by_name(name: &str) -> AxResult<i32> {
        // 方法 1: 使用 pgrep
        let output = Command::new("pgrep")
            .args(["-f", name])
            .output()
            .map_err(|e| AxError::Other(format!("Failed to run pgrep: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Ok(pid) = line.trim().parse::<i32>() {
                // 验证确实是目标应用
                if let Ok(check) = Command::new("ps")
                    .args(["-p", &pid.to_string(), "-o", "comm="])
                    .output()
                {
                    let comm = String::from_utf8_lossy(&check.stdout);
                    if comm.contains(name) || comm.to_lowercase().contains(&name.to_lowercase()) {
                        info!("Found app '{}' with PID: {}", name, pid);
                        return Ok(pid);
                    }
                }
            }
        }

        // 方法 2: 使用 AppleScript
        let script = format!(
            r#"tell application "System Events" to get unix id of (processes whose name contains "{}")"#,
            name
        );
        let output = Command::new("osascript").args(["-e", &script]).output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Ok(pid) = stdout.trim().parse::<i32>() {
                info!("Found app '{}' with PID {} via AppleScript", name, pid);
                return Ok(pid);
            }
        }

        Err(AxError::Other(format!("未找到应用 '{}' 的进程", name)))
    }

    /// 查找富途牛牛 PID（便捷方法）
    pub fn find_futu_pid() -> AxResult<i32> {
        Self::find_pid_by_name("Futu").or_else(|_| Self::find_pid_by_name("Niuniu"))
    }
}

impl Clone for Application {
    fn clone(&self) -> Self {
        // 创建新的应用元素引用
        let element = unsafe {
            AxElement::retain(self.element.as_ptr())
                .map(Element::from_wrapper)
                .expect("Failed to retain application element")
        };
        Self { element, pid: self.pid }
    }
}

/// 应用级辅助函数
impl Application {
    /// 获取第一个窗口
    pub fn first_window(&self) -> AxResult<Element> {
        let windows = self.windows()?;
        windows.into_iter().next().ok_or(AxError::WindowNotFound)
    }

    /// 查找包含特定文本的对话框
    ///
    /// 搜索所有窗口，查找包含指定文本的对话框/面板
    pub fn find_dialog_containing(&self, text: &str) -> Option<Element> {
        if let Ok(windows) = self.windows() {
            for window in windows {
                // 检查窗口本身
                if let Some(title) = window.title() {
                    if title.contains(text) {
                        return Some(window);
                    }
                }
                // 检查子元素
                let elements = window.find_all(|_| true, 5);
                for elem in elements {
                    if let Some(value) = elem.value() {
                        if value.contains(text) {
                            return Some(window);
                        }
                    }
                    if let Some(title) = elem.title() {
                        if title.contains(text) {
                            return Some(window);
                        }
                    }
                }
            }
        }
        None
    }

    /// 获取布尔属性
    pub fn bool_attribute(&self, name: &str) -> Option<bool> {
        match self.element.attribute(name) {
            Ok(super::types::CfType::Boolean(b)) => Some(b),
            Ok(_) => None,
            Err(_) => None,
        }
    }

    /// 设置隐藏状态
    pub fn set_hidden(&self, hidden: bool) -> AxResult<()> {
        self.element.set_attribute_bool("AXHidden", hidden)
    }
}

/// 窗口操作辅助函数
impl Application {
    /// 获取窗口的归一化框架
    ///
    /// 返回相对于窗口的归一化坐标 (0.0-1.0)
    pub fn get_window_normalized_frame(&self, window_idx: usize) -> AxResult<Rect> {
        let windows = self.windows()?;

        if window_idx >= windows.len() {
            return Err(AxError::WindowNotFound);
        }

        let window = &windows[window_idx];
        window.frame().map(|f| Rect::new(f.x, f.y, f.width, f.height))
    }

    /// 计算子元素相对于窗口的归一化框架
    pub fn get_element_normalized_frame(&self, window_idx: usize, element: &Element) -> AxResult<Rect> {
        let windows = self.windows()?;

        if window_idx >= windows.len() {
            return Err(AxError::WindowNotFound);
        }

        let window = &windows[window_idx];
        let win_frame = window.frame()?;
        let elem_frame = element.frame()?;

        if win_frame.width < 1.0 || win_frame.height < 1.0 {
            return Err(AxError::FrameParseFailed("Invalid window size".to_string()));
        }

        let norm_x = ((elem_frame.x - win_frame.x) / win_frame.width).clamp(0.0, 1.0);
        let norm_y = ((elem_frame.y - win_frame.y) / win_frame.height).clamp(0.0, 1.0);
        let norm_w = (elem_frame.width / win_frame.width).clamp(0.0, 1.0 - norm_x);
        let norm_h = (elem_frame.height / win_frame.height).clamp(0.0, 1.0 - norm_y);

        Ok(Rect::new(norm_x, norm_y, norm_w, norm_h))
    }

    /// 查找自选股表格框架（富途特定）
    ///
    /// 返回表格相对于窗口的归一化坐标
    pub fn find_watchlist_grid_frame(&self) -> AxResult<Rect> {
        const TARGET_ID: &str = "accessibility.futu.FTQWatchStocksViewController";

        let windows = self.windows()?;

        for (_w, window) in windows.iter().enumerate().take(5) {
            // 获取窗口框架
            let win_frame = match window.frame() {
                Ok(f) => f,
                Err(_) => continue,
            };

            if win_frame.width < 100.0 || win_frame.height < 100.0 {
                continue;
            }

            // 搜索目标元素
            if let Some(grid_element) = window.find_by_identifier(TARGET_ID, 10) {
                match grid_element.frame() {
                    Ok(grid_frame) => {
                        let norm_x = ((grid_frame.x - win_frame.x) / win_frame.width).clamp(0.0, 1.0);
                        let norm_y = ((grid_frame.y - win_frame.y) / win_frame.height).clamp(0.0, 1.0);
                        let norm_w = (grid_frame.width / win_frame.width).clamp(0.0, 1.0 - norm_x);
                        let norm_h = (grid_frame.height / win_frame.height).clamp(0.0, 1.0 - norm_y);

                        debug!(
                            "Grid frame: screen({:.0},{:.0},{:.0},{:.0}) window({:.0},{:.0},{:.0},{:.0}) normalized({:.3},{:.3},{:.3},{:.3})",
                            grid_frame.x, grid_frame.y, grid_frame.width, grid_frame.height,
                            win_frame.x, win_frame.y, win_frame.width, win_frame.height,
                            norm_x, norm_y, norm_w, norm_h
                        );

                        return Ok(Rect::new(norm_x, norm_y, norm_w, norm_h));
                    }
                    Err(e) => {
                        warn!("Failed to get grid element frame: {:?}", e);
                        continue;
                    }
                }
            }
        }

        Err(AxError::ElementNotFound(TARGET_ID.to_string()))
    }
}
