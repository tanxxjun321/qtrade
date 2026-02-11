//! AX 写操作 + 元素搜索
//!
//! 基于 macOS Accessibility API 的 UI 自动化操作。
//! 所有交互通过 AX API 完成，零 CGEvent，不干扰用户键鼠操作。

use anyhow::{Context, Result};
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::CFString;
use tracing::{debug, info};

// 额外的 AX FFI 绑定（写操作）
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: core_foundation::string::CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementPerformAction(element: CFTypeRef, action: core_foundation::string::CFStringRef) -> i32;
    fn AXUIElementSetAttributeValue(
        element: CFTypeRef,
        attribute: core_foundation::string::CFStringRef,
        value: CFTypeRef,
    ) -> i32;
    fn CFGetTypeID(cf: CFTypeRef) -> core_foundation::base::CFTypeID;
    fn CFStringGetTypeID() -> core_foundation::base::CFTypeID;
    fn CFArrayGetTypeID() -> core_foundation::base::CFTypeID;
}

const AX_ERROR_SUCCESS: i32 = 0;

/// 获取 AX 元素的属性值
fn get_attr(element: CFTypeRef, attribute: &str) -> Result<CFTypeRef> {
    let attr_name = CFString::new(attribute);
    let mut value: CFTypeRef = std::ptr::null();
    let result = unsafe {
        AXUIElementCopyAttributeValue(element, attr_name.as_concrete_TypeRef(), &mut value)
    };
    if result != AX_ERROR_SUCCESS {
        anyhow::bail!("AX get '{}' failed: error {}", attribute, result);
    }
    Ok(value)
}

/// 安全地将 CFTypeRef 转换为 String
fn cftype_to_string(value: CFTypeRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    unsafe {
        if CFGetTypeID(value) == CFStringGetTypeID() {
            let cf_string: CFString = CFString::wrap_under_get_rule(value as *const _);
            Some(cf_string.to_string())
        } else {
            None
        }
    }
}

/// 安全地检查 CFTypeRef 是否为 CFArray
fn as_cf_array(value: CFTypeRef) -> Option<CFTypeRef> {
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

/// 获取 CFArray 的长度
fn cf_array_count(array: CFTypeRef) -> isize {
    unsafe { core_foundation::array::CFArrayGetCount(array as *const _) }
}

/// 获取 CFArray 的第 i 个元素
fn cf_array_get(array: CFTypeRef, index: isize) -> CFTypeRef {
    unsafe { core_foundation::array::CFArrayGetValueAtIndex(array as *const _, index) }
}

/// 获取元素的 AXRole
pub fn get_role(element: CFTypeRef) -> Option<String> {
    get_attr(element, "AXRole").ok().and_then(|v| cftype_to_string(v))
}

/// 获取元素的 AXTitle
pub fn get_title(element: CFTypeRef) -> Option<String> {
    get_attr(element, "AXTitle").ok().and_then(|v| cftype_to_string(v))
}

/// 获取元素的 AXValue（字符串形式）
pub fn get_value_str(element: CFTypeRef) -> Option<String> {
    get_attr(element, "AXValue").ok().and_then(|v| cftype_to_string(v))
}

/// 获取元素的 AXDescription
pub fn get_description(element: CFTypeRef) -> Option<String> {
    get_attr(element, "AXDescription").ok().and_then(|v| cftype_to_string(v))
}

/// 获取元素的 AXIdentifier
pub fn get_identifier(element: CFTypeRef) -> Option<String> {
    get_attr(element, "AXIdentifier").ok().and_then(|v| cftype_to_string(v))
}

/// 获取元素的子元素数组
fn get_children(element: CFTypeRef) -> Option<CFTypeRef> {
    get_attr(element, "AXChildren").ok().and_then(|v| as_cf_array(v))
}

// ===== 写操作 API =====

/// 对 AX 元素执行操作（如 AXPress、AXRaise）
pub fn perform_action(element: CFTypeRef, action: &str) -> Result<()> {
    let action_name = CFString::new(action);
    let result = unsafe { AXUIElementPerformAction(element, action_name.as_concrete_TypeRef()) };
    if result != AX_ERROR_SUCCESS {
        anyhow::bail!("AXPerformAction('{}') failed: error {}", action, result);
    }
    Ok(())
}

/// 设置 AX 元素的文本值（AXValue 属性）
pub fn set_text_field_value(element: CFTypeRef, text: &str) -> Result<()> {
    let attr_name = CFString::new("AXValue");
    let cf_text = CFString::new(text);
    let result = unsafe {
        AXUIElementSetAttributeValue(
            element,
            attr_name.as_concrete_TypeRef(),
            cf_text.as_CFTypeRef(),
        )
    };
    if result != AX_ERROR_SUCCESS {
        anyhow::bail!("AXSetAttributeValue('AXValue', '{}') failed: error {}", text, result);
    }
    Ok(())
}

/// 聚焦元素（设置 AXFocused = true）
pub fn focus_element(element: CFTypeRef) -> Result<()> {
    let attr_name = CFString::new("AXFocused");
    let cf_true = CFBoolean::true_value();
    let result = unsafe {
        AXUIElementSetAttributeValue(
            element,
            attr_name.as_concrete_TypeRef(),
            cf_true.as_CFTypeRef(),
        )
    };
    if result != AX_ERROR_SUCCESS {
        anyhow::bail!("AXSetAttributeValue('AXFocused', true) failed: error {}", result);
    }
    Ok(())
}

// ===== 元素搜索 API =====

/// 元素匹配器
pub enum Matcher<'a> {
    /// 按 AXTitle 匹配
    Title(&'a str),
    /// 按 AXTitle 包含匹配
    TitleContains(&'a str),
    /// 按 AXIdentifier 匹配
    Identifier(&'a str),
    /// 按 AXDescription 匹配
    Description(&'a str),
    /// 任意匹配（仅匹配 role）
    Any,
}

/// AX 树广度优先搜索
///
/// 从 root 开始，搜索匹配 role + matcher 的元素。
/// max_depth 控制搜索深度。
pub fn find_element(
    root: CFTypeRef,
    role: &str,
    matcher: &Matcher,
    max_depth: usize,
) -> Option<CFTypeRef> {
    find_element_recursive(root, role, matcher, 0, max_depth)
}

fn find_element_recursive(
    element: CFTypeRef,
    target_role: &str,
    matcher: &Matcher,
    depth: usize,
    max_depth: usize,
) -> Option<CFTypeRef> {
    if element.is_null() || depth > max_depth {
        return None;
    }

    // 检查当前元素
    if let Some(role) = get_role(element) {
        if role == target_role && matches_element(element, matcher) {
            return Some(element);
        }
    }

    // 递归搜索子元素
    if let Some(children) = get_children(element) {
        let count = cf_array_count(children);
        for i in 0..count.min(200) {
            let child = cf_array_get(children, i);
            if let Some(found) =
                find_element_recursive(child, target_role, matcher, depth + 1, max_depth)
            {
                return Some(found);
            }
        }
    }

    None
}

fn matches_element(element: CFTypeRef, matcher: &Matcher) -> bool {
    match matcher {
        Matcher::Title(expected) => get_title(element).as_deref() == Some(expected),
        Matcher::TitleContains(substr) => {
            get_title(element).map_or(false, |t| t.contains(substr))
        }
        Matcher::Identifier(expected) => get_identifier(element).as_deref() == Some(expected),
        Matcher::Description(expected) => get_description(element).as_deref() == Some(expected),
        Matcher::Any => true,
    }
}

/// 按 AXTitle 匹配搜索元素
pub fn find_element_by_title(
    root: CFTypeRef,
    role: &str,
    title: &str,
    max_depth: usize,
) -> Option<CFTypeRef> {
    find_element(root, role, &Matcher::Title(title), max_depth)
}

/// 搜索所有匹配的元素（非递归返回第一个，而是返回所有）
pub fn find_all_elements(
    root: CFTypeRef,
    role: &str,
    matcher: &Matcher,
    max_depth: usize,
) -> Vec<CFTypeRef> {
    let mut results = Vec::new();
    find_all_recursive(root, role, matcher, 0, max_depth, &mut results);
    results
}

fn find_all_recursive(
    element: CFTypeRef,
    target_role: &str,
    matcher: &Matcher,
    depth: usize,
    max_depth: usize,
    results: &mut Vec<CFTypeRef>,
) {
    if element.is_null() || depth > max_depth {
        return;
    }

    if let Some(role) = get_role(element) {
        if role == target_role && matches_element(element, matcher) {
            results.push(element);
        }
    }

    if let Some(children) = get_children(element) {
        let count = cf_array_count(children);
        for i in 0..count.min(200) {
            let child = cf_array_get(children, i);
            find_all_recursive(child, target_role, matcher, depth + 1, max_depth, results);
        }
    }
}

/// 遍历 AXOutline 树形控件，按路径定位节点
///
/// path 示例: ["港股通", "港股买入"]
/// 搜索 AXOutline 下的 AXRow 元素，逐级匹配 title。
pub fn find_tree_node(root: CFTypeRef, path: &[&str]) -> Option<CFTypeRef> {
    if path.is_empty() {
        return None;
    }

    // 先找 AXOutline
    let outline = find_element(root, "AXOutline", &Matcher::Any, 10)?;

    let mut current_parent = outline;
    let mut found_node = None;

    for (level, &target_title) in path.iter().enumerate() {
        found_node = None;

        // 搜索当前层级的 AXRow 子节点
        let rows = if level == 0 {
            // 顶级：搜索 outline 下所有 AXRow
            find_all_elements(current_parent, "AXRow", &Matcher::Any, 2)
        } else {
            // 子级：搜索当前节点的子 AXRow
            if let Some(children) = get_children(current_parent) {
                let count = cf_array_count(children);
                let mut child_rows = Vec::new();
                for i in 0..count {
                    let child = cf_array_get(children, i);
                    if get_role(child).as_deref() == Some("AXRow") {
                        child_rows.push(child);
                    }
                    // 也搜索 AXGroup 等中间容器
                    if let Some(sub_children) = get_children(child) {
                        let sub_count = cf_array_count(sub_children);
                        for j in 0..sub_count {
                            let sub = cf_array_get(sub_children, j);
                            if get_role(sub).as_deref() == Some("AXRow") {
                                child_rows.push(sub);
                            }
                        }
                    }
                }
                child_rows
            } else {
                Vec::new()
            }
        };

        for row in &rows {
            // AXRow 的 title 可能在 row 本身或其子 AXStaticText 中
            let row_title = get_title(*row)
                .or_else(|| get_value_str(*row))
                .or_else(|| {
                    // 搜索子元素中的 AXStaticText
                    find_element(*row, "AXStaticText", &Matcher::Any, 3)
                        .and_then(|st| get_value_str(st).or_else(|| get_title(st)))
                });

            if let Some(ref title) = row_title {
                if title.contains(target_title) {
                    debug!("Tree node matched: level={} target='{}' found='{}'", level, target_title, title);
                    found_node = Some(*row);
                    break;
                }
            }
        }

        match found_node {
            Some(node) => current_parent = node,
            None => {
                debug!("Tree node not found: level={} target='{}'", level, target_title);
                return None;
            }
        }
    }

    found_node
}

/// 轮询等待元素出现
///
/// 每 200ms 轮询一次，超时返回 None。
pub async fn wait_for_element(
    root: CFTypeRef,
    role: &str,
    matcher: &Matcher<'_>,
    timeout_ms: u64,
) -> Option<CFTypeRef> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);
    let interval = std::time::Duration::from_millis(200);

    // 需要把 AX 搜索放在非 async 上下文中
    // 因为 CFTypeRef 不是 Send，我们在同一线程上轮询
    loop {
        if let Some(found) = find_element(root, role, matcher, 15) {
            return Some(found);
        }

        if start.elapsed() >= timeout {
            return None;
        }

        tokio::time::sleep(interval).await;
    }
}

/// 查找输入框：通过邻近的 AXStaticText 标签定位
///
/// 在 parent 下搜索所有 AXTextField，找到其附近包含 label_text 的那个。
pub fn find_text_field_by_label(parent: CFTypeRef, label_text: &str) -> Option<CFTypeRef> {
    // 搜索所有 AXTextField，检查其 AXDescription 或附近的标签
    let fields = find_all_elements(parent, "AXTextField", &Matcher::Any, 10);
    for field in &fields {
        // 检查 AXDescription 或 AXLabel
        if let Some(desc) = get_description(*field) {
            if desc.contains(label_text) {
                return Some(*field);
            }
        }
        if let Some(title) = get_title(*field) {
            if title.contains(label_text) {
                return Some(*field);
            }
        }
        // 检查 AXTitleUIElement 指向的标签
        if let Ok(title_elem) = get_attr(*field, "AXTitleUIElement") {
            if !title_elem.is_null() {
                if let Some(label_val) = get_title(title_elem).or_else(|| get_value_str(title_elem)) {
                    if label_val.contains(label_text) {
                        return Some(*field);
                    }
                }
            }
        }
    }

    None
}

/// 查找交易客户端（财富通V5.0体验版）的进程 ID
///
/// 通过 pgrep 搜索 "cft5"，再用 ps 验证进程名。
pub fn find_trading_app_pid() -> Result<i32> {
    use std::process::Command;

    let output = Command::new("pgrep")
        .args(["-f", "cft5"])
        .output()
        .context("Failed to run pgrep")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Ok(pid) = line.trim().parse::<i32>() {
            // 验证确实是财富通
            let check = Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "comm="])
                .output();
            if let Ok(check_output) = check {
                let comm = String::from_utf8_lossy(&check_output.stdout);
                if comm.contains("cft5") || comm.contains("CFT") || comm.contains("财富通") {
                    info!("Found trading app PID: {} ({})", pid, comm.trim());
                    return Ok(pid);
                }
            }
        }
    }

    // 备选：通过 AppleScript 查找
    let output = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to get unix id of (processes whose name contains "cft5")"#,
        ])
        .output();

    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Ok(pid) = stdout.trim().parse::<i32>() {
            info!("Found trading app PID via AppleScript: {}", pid);
            return Ok(pid);
        }
    }

    anyhow::bail!("未找到财富通进程。请确认 财富通V5.0体验版 已启动。")
}

/// 创建应用的 AX 根元素
pub fn create_app_element(pid: i32) -> Result<CFTypeRef> {
    let app_ref = unsafe { AXUIElementCreateApplication(pid) };
    if app_ref.is_null() {
        anyhow::bail!("Failed to create AXUIElement for PID {}", pid);
    }
    Ok(app_ref)
}

/// 获取应用的第一个窗口
pub fn get_main_window(app: CFTypeRef) -> Result<CFTypeRef> {
    let windows = get_attr(app, "AXWindows").context("Failed to get AXWindows")?;
    let win_arr = as_cf_array(windows).context("AXWindows is not an array")?;
    let count = cf_array_count(win_arr);
    if count == 0 {
        anyhow::bail!("No windows found");
    }
    let window = cf_array_get(win_arr, 0);
    if window.is_null() {
        anyhow::bail!("First window is null");
    }
    Ok(window)
}

/// 激活（raise）窗口
pub fn raise_window(window: CFTypeRef) -> Result<()> {
    perform_action(window, "AXRaise")
}

/// 调试：打印元素及其子元素的简要信息
pub fn dump_element_brief(element: CFTypeRef, max_depth: usize) -> String {
    let mut output = String::new();
    dump_brief_recursive(element, 0, max_depth, &mut output);
    output
}

fn dump_brief_recursive(element: CFTypeRef, depth: usize, max_depth: usize, output: &mut String) {
    if element.is_null() || depth > max_depth {
        return;
    }

    let indent = "  ".repeat(depth);
    let role = get_role(element).unwrap_or_else(|| "?".to_string());
    let title = get_title(element);
    let value = get_value_str(element);
    let desc = get_description(element);
    let ident = get_identifier(element);

    output.push_str(&format!("{}{}", indent, role));
    if let Some(t) = title {
        output.push_str(&format!(" title=\"{}\"", t));
    }
    if let Some(v) = value {
        let v_short = if v.len() > 50 { &v[..50] } else { &v };
        output.push_str(&format!(" value=\"{}\"", v_short));
    }
    if let Some(d) = desc {
        output.push_str(&format!(" desc=\"{}\"", d));
    }
    if let Some(id) = ident {
        output.push_str(&format!(" id=\"{}\"", id));
    }
    output.push('\n');

    if let Some(children) = get_children(element) {
        let count = cf_array_count(children);
        for i in 0..count.min(50) {
            let child = cf_array_get(children, i);
            dump_brief_recursive(child, depth + 1, max_depth, output);
        }
        if count > 50 {
            output.push_str(&format!("{}  ... ({} more)\n", indent, count - 50));
        }
    }
}
