//! macOS Accessibility API 数据获取
//!
//! 通过 AXUIElement API 从富途牛牛 App 窗口直接读取行情数据。
//! 需要用户在 系统偏好设置 → 隐私与安全性 → 辅助功能 中授权。

use anyhow::{Context, Result};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::string::CFString;
use std::ffi::c_void;
use tracing::{debug, info, warn};

use crate::models::QuoteSnapshot;

// macOS Accessibility API 通过 CoreFoundation FFI 调用
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> core_foundation::base::CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: core_foundation::base::CFTypeRef,
        attribute: core_foundation::string::CFStringRef,
        value: *mut core_foundation::base::CFTypeRef,
    ) -> i32;
    fn AXUIElementCopyAttributeNames(
        element: core_foundation::base::CFTypeRef,
        names: *mut core_foundation::base::CFTypeRef,
    ) -> i32;
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(
        options: core_foundation::dictionary::CFDictionaryRef,
    ) -> bool;

    // AXValue extraction (for AXPosition / AXSize)
    fn AXValueGetValue(
        value: core_foundation::base::CFTypeRef,
        typ: u32,
        out: *mut c_void,
    ) -> bool;

    // CoreFoundation 类型检查
    fn CFGetTypeID(cf: core_foundation::base::CFTypeRef) -> core_foundation::base::CFTypeID;
    fn CFStringGetTypeID() -> core_foundation::base::CFTypeID;
    fn CFArrayGetTypeID() -> core_foundation::base::CFTypeID;
    fn CFNumberGetTypeID() -> core_foundation::base::CFTypeID;
    fn CFBooleanGetTypeID() -> core_foundation::base::CFTypeID;
    fn CFCopyDescription(cf: core_foundation::base::CFTypeRef)
        -> core_foundation::string::CFStringRef;
}

// AX 错误码
const AX_ERROR_SUCCESS: i32 = 0;

// AXValue type constants
const K_AX_VALUE_TYPE_CG_POINT: u32 = 1;
const K_AX_VALUE_TYPE_CG_SIZE: u32 = 2;

/// Accessibility API 数据提取器
pub struct AccessibilityReader {
    /// 富途 App 进程 ID
    futu_pid: Option<i32>,
}

impl AccessibilityReader {
    pub fn new() -> Self {
        Self { futu_pid: None }
    }

    /// 检查辅助功能权限
    pub fn check_permission() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    /// 请求辅助功能权限（弹出系统授权对话框）
    pub fn request_permission() -> bool {
        unsafe {
            let key = CFString::new("AXTrustedCheckOptionPrompt");
            let value = CFBoolean::true_value();
            let pairs = [(key, value)];
            let options =
                core_foundation::dictionary::CFDictionary::from_CFType_pairs(&pairs);
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
        }
    }

    /// 查找富途牛牛 App 的进程 ID
    pub fn find_futu_pid() -> Result<i32> {
        use std::process::Command;

        let output = Command::new("pgrep")
            .args(["-f", "Futu"])
            .output()
            .context("Failed to run pgrep")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Ok(pid) = line.trim().parse::<i32>() {
                // 验证确实是富途牛牛
                let check = Command::new("ps")
                    .args(["-p", &pid.to_string(), "-o", "comm="])
                    .output();
                if let Ok(check_output) = check {
                    let comm = String::from_utf8_lossy(&check_output.stdout);
                    if comm.contains("Futu") || comm.contains("Niuniu") {
                        info!("Found Futu app PID: {}", pid);
                        return Ok(pid);
                    }
                }
            }
        }

        // 备选：通过 NSWorkspace 查找
        let output = Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to get unix id of (processes whose name contains "Futu")"#,
            ])
            .output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Ok(pid) = stdout.trim().parse::<i32>() {
                info!("Found Futu app PID via AppleScript: {}", pid);
                return Ok(pid);
            }
        }

        anyhow::bail!("未找到富途牛牛进程。请确认 App 已启动。")
    }

    /// 连接到富途 App（查找 PID + 创建 AX 引用）
    pub fn connect(&mut self) -> Result<()> {
        if !Self::check_permission() {
            warn!("辅助功能权限未授权，正在请求...");
            Self::request_permission();
            anyhow::bail!(
                "需要辅助功能权限。请在 系统偏好设置 → 隐私与安全性 → 辅助功能 中授权 qtrade。"
            );
        }

        let pid = Self::find_futu_pid()?;
        self.futu_pid = Some(pid);
        info!("Connected to Futu app (PID: {})", pid);
        Ok(())
    }

    /// 从 App 窗口读取行情数据
    pub fn read_quotes(&self) -> Result<Vec<QuoteSnapshot>> {
        let pid = self
            .futu_pid
            .context("Not connected. Call connect() first.")?;

        let app_ref = unsafe { AXUIElementCreateApplication(pid) };
        if app_ref.is_null() {
            anyhow::bail!("Failed to create AXUIElement for PID {}", pid);
        }

        let mut quotes = Vec::new();

        // 获取窗口列表
        let windows = get_ax_attribute(app_ref, "AXWindows")?;
        if windows.is_null() {
            warn!("No windows found for Futu app");
            return Ok(quotes);
        }

        // 遍历窗口，查找行情数据
        if let Some(window_array) = as_cf_array(windows) {
            let count = unsafe {
                core_foundation::array::CFArrayGetCount(
                    window_array as *const _,
                )
            };
            debug!("Found {} windows", count);

            for i in 0..count {
                let window = unsafe {
                    core_foundation::array::CFArrayGetValueAtIndex(
                        window_array as *const _,
                        i,
                    )
                };
                if !window.is_null() {
                    if let Ok(mut window_quotes) = extract_quotes_from_element(window) {
                        quotes.append(&mut window_quotes);
                    }
                }
            }
        }

        Ok(quotes)
    }

    /// 获取辅助功能元素树摘要（调试用）
    pub fn dump_element_tree(&self) -> Result<String> {
        let pid = self
            .futu_pid
            .context("Not connected. Call connect() first.")?;

        let app_ref = unsafe { AXUIElementCreateApplication(pid) };
        if app_ref.is_null() {
            anyhow::bail!("Failed to create AXUIElement for PID {}", pid);
        }

        let mut output = String::new();

        // 扫描 AXWindow 的所有直接子元素，找出含价格信息的元素
        output.push_str("=== 扫描 AXWindow 所有子元素 ===\n");
        if let Ok(windows) = get_ax_attribute(app_ref, "AXWindows") {
            if let Some(win_arr) = as_cf_array(windows) {
                let win_count = unsafe {
                    core_foundation::array::CFArrayGetCount(win_arr as *const _)
                };
                for w in 0..win_count.min(3) {
                    let window = unsafe {
                        core_foundation::array::CFArrayGetValueAtIndex(win_arr as *const _, w)
                    };
                    if window.is_null() { continue; }

                    output.push_str(&format!("--- Window {} ---\n", w));

                    // 获取窗口的所有子元素（不限制数量）
                    if let Ok(children) = get_ax_attribute(window, "AXChildren") {
                        if let Some(ch_arr) = as_cf_array(children) {
                            let ch_count = unsafe {
                                core_foundation::array::CFArrayGetCount(ch_arr as *const _)
                            };
                            output.push_str(&format!("Total children: {}\n", ch_count));

                            for i in 0..ch_count {
                                let child = unsafe {
                                    core_foundation::array::CFArrayGetValueAtIndex(
                                        ch_arr as *const _, i,
                                    )
                                };
                                if child.is_null() { continue; }

                                let role = get_ax_attribute(child, "AXRole")
                                    .ok()
                                    .and_then(|v| cftype_to_string(v))
                                    .unwrap_or_default();
                                let value = get_ax_attribute(child, "AXValue")
                                    .ok()
                                    .and_then(|v| cftype_to_string(v));
                                let title = get_ax_attribute(child, "AXTitle")
                                    .ok()
                                    .and_then(|v| cftype_to_string(v));

                                // 只打印有值的元素
                                if value.is_some() || title.is_some() {
                                    output.push_str(&format!(
                                        "  [{}] {} val={:?} title={:?}\n",
                                        i, role, value, title
                                    ));
                                } else if role == "AXGroup" || role == "AXSplitGroup"
                                    || role == "AXScrollArea"
                                {
                                    output.push_str(&format!("  [{}] {}\n", i, role));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(output)
    }
}

/// 获取 AX 元素的属性值
fn get_ax_attribute(
    element: core_foundation::base::CFTypeRef,
    attribute: &str,
) -> Result<core_foundation::base::CFTypeRef> {
    let attr_name = CFString::new(attribute);
    let mut value: core_foundation::base::CFTypeRef = std::ptr::null();

    let result = unsafe {
        AXUIElementCopyAttributeValue(element, attr_name.as_concrete_TypeRef(), &mut value)
    };

    if result != AX_ERROR_SUCCESS {
        anyhow::bail!("AXUIElementCopyAttributeValue failed for '{}': error {}", attribute, result);
    }

    Ok(value)
}

/// 获取 AX 元素的所有属性名
fn get_ax_attribute_names(
    element: core_foundation::base::CFTypeRef,
) -> Result<Vec<String>> {
    let mut names: core_foundation::base::CFTypeRef = std::ptr::null();
    let result = unsafe { AXUIElementCopyAttributeNames(element, &mut names) };

    if result != AX_ERROR_SUCCESS || names.is_null() {
        return Ok(Vec::new());
    }

    let mut result_names = Vec::new();
    if let Some(array) = as_cf_array(names) {
        let count = unsafe {
            core_foundation::array::CFArrayGetCount(array as *const _)
        };
        for i in 0..count {
            let item = unsafe {
                core_foundation::array::CFArrayGetValueAtIndex(array as *const _, i)
            };
            if !item.is_null() {
                if let Some(s) = cftype_to_string(item) {
                    result_names.push(s);
                }
            }
        }
    }

    Ok(result_names)
}

/// 从 AX 元素中提取行情数据
fn extract_quotes_from_element(
    element: core_foundation::base::CFTypeRef,
) -> Result<Vec<QuoteSnapshot>> {
    let mut quotes = Vec::new();

    // 获取元素角色
    let role = get_ax_attribute(element, "AXRole")
        .ok()
        .and_then(|v| cftype_to_string(v))
        .unwrap_or_default();

    // 获取元素值/标题
    let value = get_ax_attribute(element, "AXValue")
        .ok()
        .and_then(|v| cftype_to_string(v));
    let title = get_ax_attribute(element, "AXTitle")
        .ok()
        .and_then(|v| cftype_to_string(v));

    // 如果是表格行，可能包含行情数据
    if role == "AXRow" || role == "AXCell" || role == "AXStaticText" {
        if let Some(text) = value.or(title) {
            if let Some(quote) = crate::data::parser::try_parse_quote_text(&text) {
                quotes.push(quote);
            }
        }
    }

    // 递归遍历子元素
    if let Ok(children) = get_ax_attribute(element, "AXChildren") {
        if !children.is_null() {
            if let Some(children_array) = as_cf_array(children) {
                let count = unsafe {
                    core_foundation::array::CFArrayGetCount(children_array as *const _)
                };
                for i in 0..count.min(200) {
                    // 限制遍历数量
                    let child = unsafe {
                        core_foundation::array::CFArrayGetValueAtIndex(
                            children_array as *const _,
                            i,
                        )
                    };
                    if !child.is_null() {
                        if let Ok(mut child_quotes) = extract_quotes_from_element(child) {
                            quotes.append(&mut child_quotes);
                        }
                    }
                }
            }
        }
    }

    Ok(quotes)
}

/// 安全地将 CFTypeRef 转换为 String
/// 检查实际 CFType 类型，避免对非 CFString 类型强转导致崩溃
fn cftype_to_string(value: core_foundation::base::CFTypeRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    unsafe {
        let type_id = CFGetTypeID(value);

        if type_id == CFStringGetTypeID() {
            let cf_string: CFString = CFString::wrap_under_get_rule(value as *const _);
            Some(cf_string.to_string())
        } else if type_id == CFNumberGetTypeID() || type_id == CFBooleanGetTypeID() {
            // 对数字/布尔类型，用 CFCopyDescription 获取字符串表示
            let desc = CFCopyDescription(value);
            if desc.is_null() {
                return None;
            }
            let cf_string: CFString = CFString::wrap_under_create_rule(desc);
            Some(cf_string.to_string())
        } else {
            // 其他类型（AXUIElement 等）不转为字符串
            None
        }
    }
}

/// 安全地检查 CFTypeRef 是否为 CFArray 并返回
fn as_cf_array(
    value: core_foundation::base::CFTypeRef,
) -> Option<core_foundation::base::CFTypeRef> {
    if value.is_null() {
        return None;
    }
    unsafe {
        if CFGetTypeID(value) == CFArrayGetTypeID() {
            Some(value)
        } else {
            None
        }
    }
}

/// 调试用：打印 AX 元素树
fn dump_element(
    element: core_foundation::base::CFTypeRef,
    depth: usize,
    output: &mut String,
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }

    let indent = "  ".repeat(depth);

    let role = get_ax_attribute(element, "AXRole")
        .ok()
        .and_then(|v| cftype_to_string(v))
        .unwrap_or_else(|| "?".to_string());

    let title = get_ax_attribute(element, "AXTitle")
        .ok()
        .and_then(|v| cftype_to_string(v));

    let value = get_ax_attribute(element, "AXValue")
        .ok()
        .and_then(|v| cftype_to_string(v));

    output.push_str(&format!("{}{}", indent, role));
    if let Some(t) = title {
        output.push_str(&format!(" title=\"{}\"", t));
    }
    if let Some(v) = value {
        output.push_str(&format!(" value=\"{}\"", v));
    }
    output.push('\n');

    // 子元素
    if let Ok(children) = get_ax_attribute(element, "AXChildren") {
        if !children.is_null() {
            if let Some(children_array) = as_cf_array(children) {
                let count = unsafe {
                    core_foundation::array::CFArrayGetCount(children_array as *const _)
                };
                // 深层限制子元素数量以免输出过多
                let max_children = if depth >= 4 { 20 } else { 50 };
                for i in 0..count.min(max_children) {
                    let child = unsafe {
                        core_foundation::array::CFArrayGetValueAtIndex(
                            children_array as *const _,
                            i,
                        )
                    };
                    if !child.is_null() {
                        dump_element(child, depth + 1, output, max_depth);
                    }
                }
                if count > max_children {
                    let indent = "  ".repeat(depth + 1);
                    output.push_str(&format!(
                        "{}... ({} more children)\n",
                        indent,
                        count - max_children
                    ));
                }
            }
        }
    }
}

/// 递归查找所有 AXTable，并尝试读取其行数据
fn find_and_dump_tables(
    element: core_foundation::base::CFTypeRef,
    depth: usize,
    output: &mut String,
) {
    if depth > 6 {
        return;
    }

    let role = get_ax_attribute(element, "AXRole")
        .ok()
        .and_then(|v| cftype_to_string(v))
        .unwrap_or_default();

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
            match get_ax_attribute(element, attr) {
                Ok(val) => {
                    if val.is_null() {
                        output.push_str(&format!("{}  {}: (null)\n", indent, attr));
                        continue;
                    }
                    if let Some(arr) = as_cf_array(val) {
                        let count = unsafe {
                            core_foundation::array::CFArrayGetCount(arr as *const _)
                        };
                        output.push_str(&format!(
                            "{}  {}: array[{}]\n",
                            indent, attr, count
                        ));

                        // 展开前 5 个元素
                        for i in 0..count.min(5) {
                            let item = unsafe {
                                core_foundation::array::CFArrayGetValueAtIndex(
                                    arr as *const _,
                                    i,
                                )
                            };
                            if !item.is_null() {
                                dump_element(item, depth + 2, output, depth + 5);
                            }
                        }
                    } else if let Some(s) = cftype_to_string(val) {
                        output.push_str(&format!("{}  {}: \"{}\"\n", indent, attr, s));
                    } else {
                        output.push_str(&format!("{}  {}: (non-string/array)\n", indent, attr));
                    }
                }
                Err(_) => {
                    output.push_str(&format!("{}  {}: (error)\n", indent, attr));
                }
            }
        }

        // 也列出所有属性名
        if let Ok(names) = get_ax_attribute_names(element) {
            output.push_str(&format!(
                "{}  All attributes: {:?}\n",
                indent, names
            ));
        }

        return;
    }

    // 递归搜索子元素
    if let Ok(children) = get_ax_attribute(element, "AXChildren") {
        if !children.is_null() {
            if let Some(children_array) = as_cf_array(children) {
                let count = unsafe {
                    core_foundation::array::CFArrayGetCount(children_array as *const _)
                };
                for i in 0..count.min(50) {
                    let child = unsafe {
                        core_foundation::array::CFArrayGetValueAtIndex(
                            children_array as *const _,
                            i,
                        )
                    };
                    if !child.is_null() {
                        find_and_dump_tables(child, depth + 1, output);
                    }
                }
            }
        }
    }
}

/// 自选股表格区域（归一化坐标 0.0-1.0，相对于窗口）
#[derive(Debug, Clone, Copy)]
pub struct GridFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 递归搜索 AX 树，匹配 `AXIdentifier` 属性
fn find_element_by_identifier(
    element: core_foundation::base::CFTypeRef,
    identifier: &str,
    depth: usize,
) -> Option<core_foundation::base::CFTypeRef> {
    if depth > 10 || element.is_null() {
        return None;
    }

    // 检查当前元素的 AXIdentifier
    if let Ok(id_val) = get_ax_attribute(element, "AXIdentifier") {
        if let Some(id_str) = cftype_to_string(id_val) {
            if id_str == identifier {
                return Some(element);
            }
        }
    }

    // 递归搜索子元素
    if let Ok(children) = get_ax_attribute(element, "AXChildren") {
        if let Some(children_array) = as_cf_array(children) {
            let count = unsafe {
                core_foundation::array::CFArrayGetCount(children_array as *const _)
            };
            for i in 0..count.min(100) {
                let child = unsafe {
                    core_foundation::array::CFArrayGetValueAtIndex(
                        children_array as *const _,
                        i,
                    )
                };
                if let Some(found) = find_element_by_identifier(child, identifier, depth + 1) {
                    return Some(found);
                }
            }
        }
    }

    None
}

/// 从 AX 元素读取 AXPosition + AXSize，返回 (x, y, w, h) 屏幕坐标
fn get_element_frame(
    element: core_foundation::base::CFTypeRef,
) -> Result<(f64, f64, f64, f64)> {
    // AXPosition → CGPoint {x, y}
    let pos_val = get_ax_attribute(element, "AXPosition")
        .context("Failed to get AXPosition")?;

    let mut point: [f64; 2] = [0.0, 0.0]; // {x, y}
    let ok = unsafe {
        AXValueGetValue(
            pos_val,
            K_AX_VALUE_TYPE_CG_POINT,
            point.as_mut_ptr() as *mut c_void,
        )
    };
    if !ok {
        anyhow::bail!("AXValueGetValue failed for AXPosition");
    }

    // AXSize → CGSize {width, height}
    let size_val = get_ax_attribute(element, "AXSize")
        .context("Failed to get AXSize")?;

    let mut size: [f64; 2] = [0.0, 0.0]; // {width, height}
    let ok = unsafe {
        AXValueGetValue(
            size_val,
            K_AX_VALUE_TYPE_CG_SIZE,
            size.as_mut_ptr() as *mut c_void,
        )
    };
    if !ok {
        anyhow::bail!("AXValueGetValue failed for AXSize");
    }

    Ok((point[0], point[1], size[0], size[1]))
}

/// 查找自选股表格（FTVGridView）的归一化 frame
///
/// 通过 AX 树搜索 identifier 为 `accessibility.futu.FTQWatchStocksViewController` 的元素，
/// 获取其屏幕坐标 frame，转换为窗口相对归一化坐标 (0.0-1.0)。
pub fn find_watchlist_grid_frame(pid: i32) -> Result<GridFrame> {
    let app_ref = unsafe { AXUIElementCreateApplication(pid) };
    if app_ref.is_null() {
        anyhow::bail!("Failed to create AXUIElement for PID {}", pid);
    }

    // 获取窗口列表
    let windows = get_ax_attribute(app_ref, "AXWindows")
        .context("Failed to get AXWindows")?;
    let win_arr = as_cf_array(windows)
        .context("AXWindows is not an array")?;
    let win_count = unsafe {
        core_foundation::array::CFArrayGetCount(win_arr as *const _)
    };

    if win_count == 0 {
        anyhow::bail!("No windows found for PID {}", pid);
    }

    // 搜索每个窗口
    let target_id = "accessibility.futu.FTQWatchStocksViewController";
    for w in 0..win_count.min(5) {
        let window = unsafe {
            core_foundation::array::CFArrayGetValueAtIndex(win_arr as *const _, w)
        };
        if window.is_null() {
            continue;
        }

        // 获取窗口 frame
        let (win_x, win_y, win_w, win_h) = match get_element_frame(window) {
            Ok(f) => f,
            Err(_) => continue,
        };

        if win_w < 100.0 || win_h < 100.0 {
            continue; // 跳过太小的窗口
        }

        // 在此窗口的子树中搜索目标元素
        if let Some(grid_element) = find_element_by_identifier(window, target_id, 0) {
            let (gx, gy, gw, gh) = get_element_frame(grid_element)
                .context("Failed to get grid element frame")?;

            // 转换为窗口相对归一化坐标
            let norm_x = ((gx - win_x) / win_w).clamp(0.0, 1.0);
            let norm_y = ((gy - win_y) / win_h).clamp(0.0, 1.0);
            let norm_w = (gw / win_w).clamp(0.0, 1.0 - norm_x);
            let norm_h = (gh / win_h).clamp(0.0, 1.0 - norm_y);

            debug!(
                "Grid frame: screen({:.0},{:.0},{:.0},{:.0}) window({:.0},{:.0},{:.0},{:.0}) normalized({:.3},{:.3},{:.3},{:.3})",
                gx, gy, gw, gh, win_x, win_y, win_w, win_h, norm_x, norm_y, norm_w, norm_h
            );

            return Ok(GridFrame {
                x: norm_x,
                y: norm_y,
                width: norm_w,
                height: norm_h,
            });
        }
    }

    anyhow::bail!(
        "未找到自选股表格元素 (identifier={})",
        target_id
    )
}
