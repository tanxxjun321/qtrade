//! AX 写操作 + 元素搜索
//!
//! 基于 macOS Accessibility API 的 UI 自动化操作。
//! 大部分交互通过 AX API 完成；对于 AXPress 不生效的 Qt 控件，
//! 使用 CGEvent 前台坐标点击作为备用。
//!
//! 本模块现基于 `futu::ax` 安全 API 实现，保持向后兼容的函数签名。

use crate::futu::ax::{action, CfType, Element};
use anyhow::{Context, Result};
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::CFString;
use tracing::debug;

// FFI declarations (仅保留 CFRetain，用于指针生命周期管理)
extern "C" {
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
}

// ===== 内部辅助函数 =====

/// 将裸指针包装为 Element（不增加引用计数）
unsafe fn element_from_raw(ptr: CFTypeRef) -> Option<Element> {
    if ptr.is_null() {
        None
    } else {
        Element::from_raw(ptr).ok()
    }
}

/// 将 Element 转换为裸指针（不转移所有权）
fn element_to_raw(element: CFTypeRef) -> CFTypeRef {
    element
}

// ===== 属性获取 API（向后兼容） =====

/// 获取 AX 元素的属性值
pub fn get_attr(element: CFTypeRef, attribute: &str) -> Result<CFTypeRef> {
    let elem = unsafe { element_from_raw(element) }.context("Invalid element pointer")?;

    match elem.attribute(attribute) {
        Ok(cf_type) => {
            // 需要返回裸指针，调用方负责释放
            // 为了防止 CfType 的 Drop 实现释放内部指针，
            // 我们对需要释放的类型调用 CFRetain
            let ptr = match cf_type {
                CfType::String(s) => {
                    let p = s.as_ptr();
                    unsafe { CFRetain(p) };
                    p
                }
                CfType::Array(a) => {
                    let p = a.as_ptr();
                    unsafe { CFRetain(p) };
                    p
                }
                CfType::Number(n) => {
                    let p = n.as_ptr();
                    unsafe { CFRetain(p) };
                    p
                }
                CfType::Element(e) => {
                    let p = e.as_ptr();
                    unsafe { CFRetain(p) };
                    p
                }
                CfType::Value(v) => {
                    let p = v.as_ptr();
                    unsafe { CFRetain(p) };
                    p
                }
                CfType::Boolean(b) => {
                    if b {
                        CFBoolean::true_value().as_CFTypeRef()
                    } else {
                        CFBoolean::false_value().as_CFTypeRef()
                    }
                }
                CfType::Unknown(p) => {
                    unsafe { CFRetain(p) };
                    p
                }
            };
            Ok(ptr)
        }
        Err(e) => anyhow::bail!("AX get '{}' failed: {}", attribute, e),
    }
}

/// 安全地将 CFTypeRef 转换为 String
pub fn cftype_to_string(value: CFTypeRef) -> Option<String> {
    use core_foundation::base::{CFGetTypeID, TCFType};

    if value.is_null() {
        return None;
    }
    unsafe {
        if CFGetTypeID(value) == core_foundation::string::CFStringGetTypeID() {
            let cf_string: CFString = CFString::wrap_under_get_rule(value as *const _);
            Some(cf_string.to_string())
        } else {
            None
        }
    }
}

/// 安全地检查 CFTypeRef 是否为 CFArray
pub fn as_cf_array(value: CFTypeRef) -> Option<CFTypeRef> {
    use core_foundation::array::CFArrayGetTypeID;
    use core_foundation::base::CFGetTypeID;

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

/// 获取元素的 AXRole（不获取所有权，适用于借用指针）
pub fn get_role(element: CFTypeRef) -> Option<String> {
    let elem = unsafe { element_from_raw(element) }?;
    elem.role()
}

/// 获取元素的 AXTitle（不获取所有权，适用于借用指针）
pub fn get_title(element: CFTypeRef) -> Option<String> {
    let elem = unsafe { element_from_raw(element) }?;
    elem.title()
}

/// 获取元素的 AXValue（字符串形式，不获取所有权，适用于借用指针）
pub fn get_value_str(element: CFTypeRef) -> Option<String> {
    let elem = unsafe { element_from_raw(element) }?;
    elem.value()
}

/// 获取元素的 AXDescription（不获取所有权，适用于借用指针）
pub fn get_description(element: CFTypeRef) -> Option<String> {
    let elem = unsafe { element_from_raw(element) }?;
    elem.description()
}

/// 获取元素的 AXIdentifier（不获取所有权，适用于借用指针）
pub fn get_identifier(element: CFTypeRef) -> Option<String> {
    let elem = unsafe { element_from_raw(element) }?;
    elem.identifier()
}

/// 获取元素的父元素
pub fn get_parent(element: CFTypeRef) -> Option<CFTypeRef> {
    let elem = unsafe { element_from_raw(element) }?;
    elem.attribute("AXParent")
        .ok()
        .and_then(|cf_type| {
            if let CfType::Element(e) = cf_type {
                let p = e.as_ptr();
                unsafe { CFRetain(p) };
                Some(p)
            } else {
                None
            }
        })
        .filter(|p| !p.is_null())
}

/// 获取元素的子元素数组
fn get_children(element: CFTypeRef) -> Option<CFTypeRef> {
    let elem = unsafe { element_from_raw(element) }?;
    elem.attribute("AXChildren").ok().and_then(|cf_type| {
        if let CfType::Array(a) = cf_type {
            let p = a.as_ptr();
            unsafe { CFRetain(p) };
            Some(p)
        } else {
            None
        }
    })
}

// ===== 写操作 API =====

/// 对 AX 元素执行操作（如 AXPress、AXRaise）
pub fn perform_action(element: CFTypeRef, action: &str) -> Result<()> {
    let elem = unsafe { element_from_raw(element) }.context("Invalid element pointer")?;

    elem.perform_action(action)
        .map_err(|e| anyhow::anyhow!("AXPerformAction('{}') failed: {}", action, e))
}

/// 设置 AX 元素的文本值（AXValue 属性）
pub fn set_text_field_value(element: CFTypeRef, text: &str) -> Result<()> {
    let elem = unsafe { element_from_raw(element) }.context("Invalid element pointer")?;

    elem.set_string_value(text)
        .map_err(|e| anyhow::anyhow!("AXSetAttributeValue('AXValue', '{}') failed: {}", text, e))
}

/// 聚焦元素（设置 AXFocused = true）
pub fn focus_element(element: CFTypeRef) -> Result<()> {
    set_bool_attr(element, "AXFocused", true)
}

/// 读取布尔属性（如 AXHidden、AXMinimized）
pub fn get_bool_attr(element: CFTypeRef, attribute: &str) -> Option<bool> {
    let elem = unsafe { element_from_raw(element) }?;

    match elem.attribute(attribute) {
        Ok(CfType::Boolean(b)) => Some(b),
        _ => {
            // 尝试原始方式比较
            let value = get_attr(element, attribute).ok()?;
            Some(value == CFBoolean::true_value().as_CFTypeRef())
        }
    }
}

/// 设置布尔属性
pub fn set_bool_attr(element: CFTypeRef, attribute: &str, value: bool) -> Result<()> {
    let elem = unsafe { element_from_raw(element) }.context("Invalid element pointer")?;

    elem.set_attribute_bool(attribute, value)
        .map_err(|e| anyhow::anyhow!("AXSetAttributeValue('{}', {}) failed: {}", attribute, value, e))
}

// ===== 前台坐标点击（CGEventPost to HID） =====

/// 通过前台 CGEvent 坐标点击元素
///
/// 读取 AXPosition + AXSize 计算中心点，通过 CGEventPost(HID) 发送点击。
/// 需要窗口已 raise 到前台。仅用于 AXPress 无效的 Qt 控件。
pub fn click_at_element(element: CFTypeRef) -> Result<()> {
    let elem = unsafe { element_from_raw(element) }.context("Invalid element pointer")?;

    action::click_at_element(&elem).map_err(|e| anyhow::anyhow!("click_at_element failed: {}", e))
}

/// 后台点击：通过 CGEventPostToPSN 将鼠标事件直接发送到指定进程
///
/// 不需要窗口在前台。读取 AXPosition + AXSize 计算中心点，
/// 通过 ProcessSerialNumber 定向投递给目标进程。
pub fn click_at_element_to_pid(element: CFTypeRef, pid: i32) -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct ProcessSerialNumber {
        high: u32,
        low: u32,
    }

    extern "C" {
        fn GetProcessForPID(pid: i32, psn: *mut ProcessSerialNumber) -> i32;
        fn CGEventPostToPSN(psn: *const ProcessSerialNumber, event: core_graphics::sys::CGEventRef);
    }

    // 获取 PSN
    let mut psn = ProcessSerialNumber { high: 0, low: 0 };
    let status = unsafe { GetProcessForPID(pid, &mut psn) };
    if status != 0 {
        anyhow::bail!("GetProcessForPID({}) failed: {}", pid, status);
    }

    // 读取元素位置和大小 - 使用内部辅助获取 frame
    let elem = unsafe { element_from_raw(element) }.context("Invalid element pointer")?;

    let frame = elem.frame().map_err(|e| anyhow::anyhow!("无法读取元素框架: {}", e))?;

    let center = CGPoint::new(frame.x + frame.width / 2.0, frame.y + frame.height / 2.0);
    debug!(
        "click_at_element_to_pid({}): center=({:.0},{:.0})",
        pid, center.x, center.y
    );

    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;

    let mouse_down = CGEvent::new_mouse_event(source.clone(), CGEventType::LeftMouseDown, center, CGMouseButton::Left)
        .map_err(|_| anyhow::anyhow!("Failed to create mouse down event"))?;

    let mouse_up = CGEvent::new_mouse_event(source, CGEventType::LeftMouseUp, center, CGMouseButton::Left)
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
    let elem = unsafe { element_from_raw(element) }.context("Invalid element pointer")?;

    action::set_string_value_via_paste(&elem, value).map_err(|e| anyhow::anyhow!("type_value_via_paste failed: {}", e))
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

/// 将旧版 Matcher 转换为新版 action::Matcher
fn to_new_matcher<'a>(matcher: &'a Matcher<'a>) -> action::Matcher<'a> {
    match matcher {
        Matcher::Title(s) => action::Matcher::Title(s),
        Matcher::TitleContains(s) => action::Matcher::TitleContains(s),
        Matcher::Identifier(s) => action::Matcher::Identifier(s),
        Matcher::Description(s) => action::Matcher::Description(s),
        Matcher::Any => action::Matcher::Any,
    }
}

/// AX 树广度优先搜索
///
/// 从 root 开始，搜索匹配 role + matcher 的元素。
/// max_depth 控制搜索深度。
pub fn find_element(root: CFTypeRef, role: &str, matcher: &Matcher, max_depth: usize) -> Option<CFTypeRef> {
    let root_elem = unsafe { element_from_raw(root) }?;
    let new_matcher = to_new_matcher(matcher);

    action::find_element_with_matcher(&root_elem, role, &new_matcher, max_depth).map(|e| {
        let ptr = e.as_ptr();
        unsafe { CFRetain(ptr) };
        ptr
    })
}

/// 按 AXTitle 匹配搜索元素
pub fn find_element_by_title(root: CFTypeRef, role: &str, title: &str, max_depth: usize) -> Option<CFTypeRef> {
    find_element(root, role, &Matcher::Title(title), max_depth)
}

/// 搜索所有匹配的元素（非递归返回第一个，而是返回所有）
pub fn find_all_elements(root: CFTypeRef, role: &str, matcher: &Matcher, max_depth: usize) -> Vec<CFTypeRef> {
    let root_elem = match unsafe { element_from_raw(root) } {
        Some(e) => e,
        None => return Vec::new(),
    };
    let new_matcher = to_new_matcher(matcher);

    action::find_all_elements_with_matcher(&root_elem, role, &new_matcher, max_depth)
        .iter()
        .map(|e| {
            let ptr = e.as_ptr();
            unsafe { CFRetain(ptr) };
            ptr
        })
        .collect()
}

/// 遍历 AXOutline 树形控件，按路径定位节点
///
/// path 示例: ["港股通", "港股买入"]
/// 搜索 AXOutline 下的 AXRow 元素，逐级匹配 title。
pub fn find_tree_node(root: CFTypeRef, path: &[&str]) -> Option<CFTypeRef> {
    if path.is_empty() {
        return None;
    }

    let root_elem = unsafe { element_from_raw(root) }?;

    // 先找 AXOutline
    let outline = root_elem.find(|e| e.role().as_deref() == Some("AXOutline"), 10)?;

    let mut current_parent = outline;
    let mut found_node = None;

    for (level, &target_title) in path.iter().enumerate() {
        found_node = None;

        // 搜索当前层级的 AXRow 子节点
        let rows: Vec<Element> = if level == 0 {
            // 顶级：搜索 outline 下所有 AXRow
            current_parent.find_all(|e| e.role().as_deref() == Some("AXRow"), 2)
        } else {
            // 子级：搜索当前节点的子 AXRow
            let mut child_rows = Vec::new();
            if let Ok(children) = current_parent.children() {
                for child in &children {
                    if child.role().as_deref() == Some("AXRow") {
                        child_rows.push(child.clone());
                    }
                    // 也搜索 AXGroup 等中间容器
                    if let Ok(sub_children) = child.children() {
                        for sub in &sub_children {
                            if sub.role().as_deref() == Some("AXRow") {
                                child_rows.push(sub.clone());
                            }
                        }
                    }
                }
            }
            child_rows
        };

        for row in &rows {
            // AXRow 的 title 可能在 row 本身或其子 AXStaticText 中
            let row_title = row.title().or_else(|| row.value()).or_else(|| {
                // 搜索子元素中的 AXStaticText
                row.find(|e| e.role().as_deref() == Some("AXStaticText"), 3)
                    .and_then(|st| st.value().or_else(|| st.title()))
            });

            if let Some(ref title) = row_title {
                if title.contains(target_title) {
                    debug!(
                        "Tree node matched: level={} target='{}' found='{}'",
                        level, target_title, title
                    );
                    found_node = Some(row.clone());
                    break;
                }
            }
        }

        match found_node {
            Some(ref node) => current_parent = node.clone(),
            None => {
                debug!("Tree node not found: level={} target='{}'", level, target_title);
                return None;
            }
        }
    }

    found_node.map(|e| {
        let ptr = e.as_ptr();
        unsafe { CFRetain(ptr) };
        ptr
    })
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
    let parent_elem = unsafe { element_from_raw(parent) }?;

    // 搜索所有 AXTextField，检查其 AXDescription 或附近的标签
    let fields = parent_elem.find_all_by_role("AXTextField", 10);

    for field in &fields {
        // 检查 AXDescription 或 AXLabel
        if let Some(desc) = field.description() {
            if desc.contains(label_text) {
                let ptr = field.as_ptr();
                unsafe { CFRetain(ptr) };
                return Some(ptr);
            }
        }
        if let Some(title) = field.title() {
            if title.contains(label_text) {
                let ptr = field.as_ptr();
                unsafe { CFRetain(ptr) };
                return Some(ptr);
            }
        }
        // 检查 AXTitleUIElement 指向的标签
        if let Ok(title_elem_cf) = field.attribute("AXTitleUIElement") {
            if let CfType::Element(title_elem) = title_elem_cf {
                if let Some(title_elem_ref) = unsafe { element_from_raw(title_elem.as_ptr()) } {
                    if let Some(label_val) = title_elem_ref.title().or_else(|| title_elem_ref.value()) {
                        if label_val.contains(label_text) {
                            let ptr = field.as_ptr();
                            unsafe { CFRetain(ptr) };
                            return Some(ptr);
                        }
                    }
                }
            }
        }
    }

    None
}

/// 创建应用的 AX 根元素
///
/// 基于 `crate::futu::ax::Application` 安全 API 实现。
pub fn create_app_element(pid: i32) -> Result<CFTypeRef> {
    let app = crate::futu::ax::Application::new(pid)
        .map_err(|e| anyhow::anyhow!("Failed to create AXUIElement for PID {}: {:?}", pid, e))?;
    // 返回元素指针，增加引用计数确保生命周期
    let ptr = app.element().as_ptr();
    unsafe { CFRetain(ptr) };
    Ok(ptr)
}

/// 激活（raise）窗口
pub fn raise_window(window: CFTypeRef) -> Result<()> {
    perform_action(window, "AXRaise")
}

/// 调试：打印元素及其子元素的简要信息
pub fn dump_element_brief(element: CFTypeRef, max_depth: usize) -> String {
    let elem = match unsafe { element_from_raw(element) } {
        Some(e) => e,
        None => return "<invalid element>".to_string(),
    };

    let mut output = String::new();
    dump_brief_recursive(&elem, 0, max_depth, &mut output);
    output
}

fn dump_brief_recursive(element: &Element, depth: usize, max_depth: usize, output: &mut String) {
    if depth > max_depth {
        return;
    }

    let indent = "  ".repeat(depth);
    let role = element.role().unwrap_or_else(|| "?".to_string());
    let title = element.title();
    let value = element.value();
    let desc = element.description();
    let ident = element.identifier();

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

    if let Ok(children) = element.children() {
        let count = children.len();
        for child in children.iter().take(50) {
            dump_brief_recursive(child, depth + 1, max_depth, output);
        }
        if count > 50 {
            output.push_str(&format!("{}  ... ({} more)\n", indent, count - 50));
        }
    }
}
