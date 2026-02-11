//! AX 写操作 + 元素搜索
//!
//! 基于 macOS Accessibility API 的 UI 自动化操作。
//! 大部分交互通过 AX API 完成；对于 AXPress 不生效的 Qt 控件，
//! 使用 CGEvent 前台坐标点击作为备用。

use anyhow::{Context, Result};
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::CFString;
use tracing::debug;

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
pub fn get_attr(element: CFTypeRef, attribute: &str) -> Result<CFTypeRef> {
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
pub fn cftype_to_string(value: CFTypeRef) -> Option<String> {
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
pub fn as_cf_array(value: CFTypeRef) -> Option<CFTypeRef> {
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
pub fn cf_array_count(array: CFTypeRef) -> isize {
    unsafe { core_foundation::array::CFArrayGetCount(array as *const _) }
}

/// 获取 CFArray 的第 i 个元素
pub fn cf_array_get(array: CFTypeRef, index: isize) -> CFTypeRef {
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

/// 获取元素的父元素
pub fn get_parent(element: CFTypeRef) -> Option<CFTypeRef> {
    get_attr(element, "AXParent").ok().filter(|v| !v.is_null())
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
    set_bool_attr(element, "AXFocused", true)
}

/// 读取布尔属性（如 AXHidden、AXMinimized）
pub fn get_bool_attr(element: CFTypeRef, attribute: &str) -> Option<bool> {
    let value = get_attr(element, attribute).ok()?;
    Some(value == CFBoolean::true_value().as_CFTypeRef())
}

/// 设置布尔属性
pub fn set_bool_attr(element: CFTypeRef, attribute: &str, value: bool) -> Result<()> {
    let attr_name = CFString::new(attribute);
    let cf_val = if value {
        CFBoolean::true_value()
    } else {
        CFBoolean::false_value()
    };
    let result = unsafe {
        AXUIElementSetAttributeValue(
            element,
            attr_name.as_concrete_TypeRef(),
            cf_val.as_CFTypeRef(),
        )
    };
    if result != AX_ERROR_SUCCESS {
        anyhow::bail!(
            "AXSetAttributeValue('{}', {}) failed: error {}",
            attribute, value, result
        );
    }
    Ok(())
}


// ===== 前台坐标点击（CGEventPost to HID）=====

/// 通过前台 CGEvent 坐标点击元素
///
/// 读取 AXPosition + AXSize 计算中心点，通过 CGEventPost(HID) 发送点击。
/// 需要窗口已 raise 到前台。仅用于 AXPress 无效的 Qt 控件。
pub fn click_at_element(element: CFTypeRef) -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::{CGPoint, CGSize};
    use std::ffi::c_void;

    extern "C" {
        fn AXValueGetValue(value: CFTypeRef, typ: u32, out: *mut c_void) -> bool;
    }

    // 读取元素位置和大小
    let pos_ref = get_attr(element, "AXPosition")
        .context("无法读取 AXPosition")?;
    let mut position = CGPoint::new(0.0, 0.0);
    if !unsafe {
        AXValueGetValue(pos_ref, 1, &mut position as *mut _ as *mut c_void)
    } {
        anyhow::bail!("AXValueGetValue(AXPosition) failed");
    }

    let size_ref = get_attr(element, "AXSize")
        .context("无法读取 AXSize")?;
    let mut size = CGSize::new(0.0, 0.0);
    if !unsafe {
        AXValueGetValue(size_ref, 2, &mut size as *mut _ as *mut c_void)
    } {
        anyhow::bail!("AXValueGetValue(AXSize) failed");
    }

    let center = CGPoint::new(
        position.x + size.width / 2.0,
        position.y + size.height / 2.0,
    );
    debug!("click_at_element: center=({:.0},{:.0})", center.x, center.y);

    // 前台 CGEventPost
    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;

    let mouse_down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::LeftMouseDown,
        center,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create mouse down event"))?;

    let mouse_up = CGEvent::new_mouse_event(
        source,
        CGEventType::LeftMouseUp,
        center,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create mouse up event"))?;

    mouse_down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(50));
    mouse_up.post(CGEventTapLocation::HID);

    Ok(())
}

/// 后台点击：通过 CGEventPostToPSN 将鼠标事件直接发送到指定进程
///
/// 不需要窗口在前台。读取 AXPosition + AXSize 计算中心点，
/// 通过 ProcessSerialNumber 定向投递给目标进程。
pub fn click_at_element_to_pid(element: CFTypeRef, pid: i32) -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::{CGPoint, CGSize};
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct ProcessSerialNumber {
        high: u32,
        low: u32,
    }

    extern "C" {
        fn AXValueGetValue(value: CFTypeRef, typ: u32, out: *mut c_void) -> bool;
        fn GetProcessForPID(pid: i32, psn: *mut ProcessSerialNumber) -> i32;
        fn CGEventPostToPSN(psn: *const ProcessSerialNumber, event: core_graphics::sys::CGEventRef);
    }

    // 获取 PSN
    let mut psn = ProcessSerialNumber { high: 0, low: 0 };
    let status = unsafe { GetProcessForPID(pid, &mut psn) };
    if status != 0 {
        anyhow::bail!("GetProcessForPID({}) failed: {}", pid, status);
    }

    // 读取元素位置和大小
    let pos_ref = get_attr(element, "AXPosition")
        .context("无法读取 AXPosition")?;
    let mut position = CGPoint::new(0.0, 0.0);
    if !unsafe {
        AXValueGetValue(pos_ref, 1, &mut position as *mut _ as *mut c_void)
    } {
        anyhow::bail!("AXValueGetValue(AXPosition) failed");
    }

    let size_ref = get_attr(element, "AXSize")
        .context("无法读取 AXSize")?;
    let mut size = CGSize::new(0.0, 0.0);
    if !unsafe {
        AXValueGetValue(size_ref, 2, &mut size as *mut _ as *mut c_void)
    } {
        anyhow::bail!("AXValueGetValue(AXSize) failed");
    }

    let center = CGPoint::new(
        position.x + size.width / 2.0,
        position.y + size.height / 2.0,
    );
    debug!("click_at_element_to_pid({}): center=({:.0},{:.0})", pid, center.x, center.y);

    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;

    let mouse_down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::LeftMouseDown,
        center,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create mouse down event"))?;

    let mouse_up = CGEvent::new_mouse_event(
        source,
        CGEventType::LeftMouseUp,
        center,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create mouse up event"))?;

    use foreign_types::ForeignType;
    unsafe {
        CGEventPostToPSN(&psn, mouse_down.as_ptr());
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    unsafe {
        CGEventPostToPSN(&psn, mouse_up.as_ptr());
    }

    Ok(())
}

/// 前台键盘输入：聚焦元素 → 全选 → 粘贴剪贴板内容
///
/// 用于 AXIncrementor 等不接受 AXValue 直接设值的 Qt 控件。
/// 需要窗口已 raise 到前台。
pub fn type_value_via_paste(element: CFTypeRef, value: &str) -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // 设置剪贴板（通过 stdin 传入，避免 shell 注入）
    let mut child = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("pbcopy spawn failed")?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(value.as_bytes()).context("write to pbcopy failed")?;
    }
    child.wait().context("pbcopy wait failed")?;

    // 聚焦元素
    focus_element(element)?;
    std::thread::sleep(std::time::Duration::from_millis(50));

    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;

    // Cmd+A (全选) — keycode 0 = 'a'
    let select_all_down = CGEvent::new_keyboard_event(source.clone(), 0, true)
        .map_err(|_| anyhow::anyhow!("Failed to create key event"))?;
    select_all_down.set_flags(CGEventFlags::CGEventFlagCommand);
    let select_all_up = CGEvent::new_keyboard_event(source.clone(), 0, false)
        .map_err(|_| anyhow::anyhow!("Failed to create key event"))?;
    select_all_up.set_flags(CGEventFlags::CGEventFlagCommand);

    select_all_down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(30));
    select_all_up.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Cmd+V (粘贴) — keycode 9 = 'v'
    let paste_down = CGEvent::new_keyboard_event(source.clone(), 9, true)
        .map_err(|_| anyhow::anyhow!("Failed to create key event"))?;
    paste_down.set_flags(CGEventFlags::CGEventFlagCommand);
    let paste_up = CGEvent::new_keyboard_event(source, 9, false)
        .map_err(|_| anyhow::anyhow!("Failed to create key event"))?;
    paste_up.set_flags(CGEventFlags::CGEventFlagCommand);

    paste_down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(30));
    paste_up.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(100));

    debug!("已通过 Cmd+V 输入: {}", value);
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
/// 创建应用的 AX 根元素
pub fn create_app_element(pid: i32) -> Result<CFTypeRef> {
    let app_ref = unsafe { AXUIElementCreateApplication(pid) };
    if app_ref.is_null() {
        anyhow::bail!("Failed to create AXUIElement for PID {}", pid);
    }
    Ok(app_ref)
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
