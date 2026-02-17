//! macOS Accessibility API 数据获取
//!
//! 通过 AXUIElement API 从富途牛牛 App 窗口直接读取行情数据。
//! 需要用户在 系统偏好设置 → 隐私与安全性 → 辅助功能 中授权。

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::models::QuoteSnapshot;

// 使用新的安全封装层
use crate::futu::ax::{Application, Element, Rect};

/// 自选股表格区域（归一化坐标 0.0-1.0，相对于窗口）
#[derive(Debug, Clone, Copy)]
pub struct GridFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<Rect> for GridFrame {
    fn from(rect: Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

/// Accessibility API 数据提取器
pub struct AccessibilityReader {
    /// 富途 App 连接
    app: Option<Application>,
    /// 富途 App 进程 ID
    futu_pid: Option<i32>,
}

impl AccessibilityReader {
    pub fn new() -> Self {
        Self {
            app: None,
            futu_pid: None,
        }
    }

    /// 检查辅助功能权限
    pub fn check_permission() -> bool {
        Application::check_permission()
    }

    /// 请求辅助功能权限（弹出系统授权对话框）
    pub fn request_permission() -> bool {
        Application::request_permission()
    }

    /// 查找富途牛牛 App 的进程 ID
    pub fn find_futu_pid() -> Result<i32> {
        Application::find_futu_pid().map_err(|e| anyhow::anyhow!("查找富途牛牛进程失败: {}", e))
    }

    /// 连接到富途 App（查找 PID + 创建 AX 引用）
    pub fn connect(&mut self) -> Result<()> {
        if !Self::check_permission() {
            warn!("辅助功能权限未授权，正在请求...");
            Self::request_permission();
            anyhow::bail!("需要辅助功能权限。请在 系统偏好设置 → 隐私与安全性 → 辅助功能 中授权 qtrade。");
        }

        let pid = Self::find_futu_pid()?;
        let app = Application::new(pid).map_err(|e| anyhow::anyhow!("无法连接到富途应用: {}", e))?;

        self.futu_pid = Some(pid);
        self.app = Some(app);
        info!("Connected to Futu app (PID: {})", pid);
        Ok(())
    }

    /// 从 App 窗口读取行情数据
    pub fn read_quotes(&self) -> Result<Vec<QuoteSnapshot>> {
        let app = self.app.as_ref().context("Not connected. Call connect() first.")?;

        let mut quotes = Vec::new();

        // 获取窗口列表
        let windows = app.windows().map_err(|e| anyhow::anyhow!("获取窗口列表失败: {}", e))?;

        debug!("Found {} windows", windows.len());

        // 遍历窗口，查找行情数据
        for (idx, window) in windows.iter().enumerate() {
            debug!("Scanning window {}", idx);
            if let Ok(mut window_quotes) = extract_quotes_from_element(window) {
                quotes.append(&mut window_quotes);
            }
        }

        Ok(quotes)
    }

    /// 获取辅助功能元素树摘要（调试用）
    pub fn dump_element_tree(&self) -> Result<String> {
        let app = self.app.as_ref().context("Not connected. Call connect() first.")?;

        let mut output = String::new();

        // 扫描 AXWindow 的所有直接子元素，找出含价格信息的元素
        output.push_str("=== 扫描 AXWindow 所有子元素 ===\n");

        let windows = app.windows().map_err(|e| anyhow::anyhow!("获取窗口列表失败: {}", e))?;

        for (w, window) in windows.iter().enumerate().take(3) {
            output.push_str(&format!("--- Window {} ---\n", w));

            // 获取窗口的所有子元素（不限制数量）
            match window.children() {
                Ok(children) => {
                    output.push_str(&format!("Total children: {}\n", children.len()));

                    for (i, child) in children.iter().enumerate() {
                        let role = child.role().unwrap_or_default();
                        let value = child.value();
                        let title = child.title();

                        // 只打印有值的元素
                        if value.is_some() || title.is_some() {
                            output.push_str(&format!("  [{}] {} val={:?} title={:?}\n", i, role, value, title));
                        } else if role == "AXGroup" || role == "AXSplitGroup" || role == "AXScrollArea" {
                            output.push_str(&format!("  [{}] {}\n", i, role));
                        }
                    }
                }
                Err(e) => {
                    output.push_str(&format!("获取子元素失败: {}\n", e));
                }
            }
        }

        Ok(output)
    }

    /// 获取应用引用（用于其他模块访问）
    pub fn app(&self) -> Option<&Application> {
        self.app.as_ref()
    }

    /// 获取 PID
    pub fn pid(&self) -> Option<i32> {
        self.futu_pid
    }

    /// 查找自选股表格（FTVGridView）的归一化 frame
    ///
    /// 通过 AX 树搜索 identifier 为 `accessibility.futu.FTQWatchStocksViewController` 的元素，
    /// 获取其屏幕坐标 frame，转换为窗口相对归一化坐标 (0.0-1.0)。
    pub fn find_watchlist_grid_frame(&self) -> Result<GridFrame> {
        let app = self.app.as_ref().context("Not connected. Call connect() first.")?;

        let frame = app
            .find_watchlist_grid_frame()
            .map_err(|e| anyhow::anyhow!("查找自选股表格失败: {}", e))?;

        Ok(GridFrame::from(frame))
    }
}

/// 查找自选股表格的便捷函数（供其他模块使用）
pub fn find_watchlist_grid_frame(pid: i32) -> Result<GridFrame> {
    let app = Application::new(pid).map_err(|e| anyhow::anyhow!("无法连接到应用: {}", e))?;

    let frame = app
        .find_watchlist_grid_frame()
        .map_err(|e| anyhow::anyhow!("查找自选股表格失败: {}", e))?;

    Ok(GridFrame::from(frame))
}

/// 从 AX 元素中提取行情数据
fn extract_quotes_from_element(element: &Element) -> Result<Vec<QuoteSnapshot>> {
    let mut quotes = Vec::new();

    // 获取元素角色
    let role = element.role().unwrap_or_default();

    // 获取元素值/标题
    let value = element.value();
    let title = element.title();

    // 如果是表格行，可能包含行情数据
    if role == "AXRow" || role == "AXCell" || role == "AXStaticText" {
        if let Some(text) = value.or(title) {
            if let Some(quote) = crate::data::parser::try_parse_quote_text(&text) {
                quotes.push(quote);
            }
        }
    }

    // 递归遍历子元素
    if let Ok(children) = element.children() {
        // 限制遍历数量
        for child in children.iter().take(200) {
            if let Ok(mut child_quotes) = extract_quotes_from_element(child) {
                quotes.append(&mut child_quotes);
            }
        }
    }

    Ok(quotes)
}

/// 查找所有表格并打印详细信息（调试用）
#[allow(dead_code)]
pub fn debug_dump_tables(pid: i32) -> Result<String> {
    let app = Application::new(pid)?;
    let mut output = String::new();

    let windows = app.windows()?;
    for (w, window) in windows.iter().enumerate() {
        output.push_str(&format!("=== Window {} ===\n", w));
        find_and_dump_tables(window, 0, &mut output);
    }

    Ok(output)
}

/// 递归搜索 AX 树，匹配表格并打印
fn find_and_dump_tables(element: &Element, depth: usize, output: &mut String) {
    if depth > 6 {
        return;
    }

    let role = element.role().unwrap_or_default();

    if role == "AXTable" {
        let indent = "  ".repeat(depth);
        output.push_str(&format!("{}[Found AXTable at depth {}]\n", indent, depth));

        // 尝试多种方式读取表格数据
        for attr in &[
            "AXRows",
            "AXVisibleRows",
            "AXColumns",
            "AXVisibleColumns",
            "AXVisibleCells",
            "AXHeader",
            "AXChildren",
            "AXDescription",
            "AXRoleDescription",
        ] {
            match element.attribute(attr) {
                Ok(attr_value) => {
                    match attr_value {
                        crate::futu::ax::CfType::Array(arr) => {
                            output.push_str(&format!("{}  {}: array[{}]\n", indent, attr, arr.len()));

                            // 展开前 5 个元素
                            for (i, ptr) in arr.iter().take(5).enumerate() {
                                if let Some(elem) = unsafe { crate::futu::ax::AxElement::retain(ptr) } {
                                    let child = Element::from_wrapper(elem);
                                    output.push_str(&format!("{}    [{}] ", indent, i));
                                    if let Some(r) = child.role() {
                                        output.push_str(&format!("role={} ", r));
                                    }
                                    if let Some(t) = child.title() {
                                        output.push_str(&format!("title=\"{}\" ", t));
                                    }
                                    if let Some(v) = child.value() {
                                        output.push_str(&format!("value=\"{}\" ", v));
                                    }
                                    output.push('\n');
                                }
                            }
                        }
                        crate::futu::ax::CfType::String(s) => {
                            if let Some(s) = s.to_string() {
                                output.push_str(&format!("{}  {}: \"{}\"\n", indent, attr, s));
                            } else {
                                output.push_str(&format!("{}  {}: (string conversion failed)\n", indent, attr));
                            }
                        }
                        _ => {
                            output.push_str(&format!("{}  {}: (other type)\n", indent, attr));
                        }
                    }
                }
                Err(_) => {
                    output.push_str(&format!("{}  {}: (error)\n", indent, attr));
                }
            }
        }

        return;
    }

    // 递归搜索子元素
    if let Ok(children) = element.children() {
        for child in children.iter().take(50) {
            find_and_dump_tables(child, depth + 1, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grid_frame_from_rect() {
        let rect = Rect::new(0.1, 0.2, 0.3, 0.4);
        let grid: GridFrame = rect.into();

        assert_eq!(grid.x, 0.1);
        assert_eq!(grid.y, 0.2);
        assert_eq!(grid.width, 0.3);
        assert_eq!(grid.height, 0.4);
    }

    #[test]
    fn test_accessibility_reader_new() {
        let reader = AccessibilityReader::new();
        assert!(reader.app.is_none());
        assert!(reader.futu_pid.is_none());
    }
}
