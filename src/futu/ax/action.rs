//! AX 特殊操作
//!
//! 包含需要 CGEvent 或剪贴板配合的 UI 操作。

use super::error::{AxError, AxResult};
use super::element::Element;
use tracing::debug;

/// 通过前台 CGEvent 坐标点击元素
///
/// 读取 AXPosition + AXSize 计算中心点，通过 CGEventPost(HID) 发送点击。
/// 需要窗口已 raise 到前台。仅用于 AXPress 无效的 Qt 控件。
pub fn click_at_element(element: &Element) -> AxResult<()> {
    use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;

    // 读取元素位置和大小
    let frame = element.frame()
        .map_err(|e| AxError::Other(format!("无法读取元素框架: {}", e)))?;

    let center = CGPoint::new(
        frame.x + frame.width / 2.0,
        frame.y + frame.height / 2.0,
    );
    debug!("click_at_element: center=({:.0},{:.0})", center.x, center.y);

    // 前台 CGEventPost
    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|_| AxError::Other("Failed to create CGEventSource".to_string()))?;

    let mouse_down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::LeftMouseDown,
        center,
        CGMouseButton::Left,
    )
    .map_err(|_| AxError::Other("Failed to create mouse down event".to_string()))?;

    let mouse_up = CGEvent::new_mouse_event(
        source,
        CGEventType::LeftMouseUp,
        center,
        CGMouseButton::Left,
    )
    .map_err(|_| AxError::Other("Failed to create mouse up event".to_string()))?;

    mouse_down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(50));
    mouse_up.post(CGEventTapLocation::HID);

    Ok(())
}

/// 前台键盘输入：聚焦元素 → 全选 → 粘贴剪贴板内容
///
/// 用于 AXIncrementor 等不接受 AXValue 直接设值的 Qt 控件。
/// 需要窗口已 raise 到前台。
pub fn set_string_value_via_paste(element: &Element, value: &str) -> AxResult<()> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // 设置剪贴板（通过 stdin 传入，避免 shell 注入）
    let mut child = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| AxError::Other(format!("pbcopy spawn failed: {}", e)))?;
    
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(value.as_bytes())
            .map_err(|e| AxError::Other(format!("write to pbcopy failed: {}", e)))?;
    }
    child.wait()
        .map_err(|e| AxError::Other(format!("pbcopy wait failed: {}", e)))?;

    // 聚焦元素
    element.set_focused(true)?;
    std::thread::sleep(std::time::Duration::from_millis(50));

    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|_| AxError::Other("Failed to create CGEventSource".to_string()))?;

    // Cmd+A (全选) — keycode 0 = 'a'
    let select_all_down = CGEvent::new_keyboard_event(source.clone(), 0, true)
        .map_err(|_| AxError::Other("Failed to create key event".to_string()))?;
    select_all_down.set_flags(CGEventFlags::CGEventFlagCommand);
    let select_all_up = CGEvent::new_keyboard_event(source.clone(), 0, false)
        .map_err(|_| AxError::Other("Failed to create key event".to_string()))?;
    select_all_up.set_flags(CGEventFlags::CGEventFlagCommand);

    select_all_down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(30));
    select_all_up.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Cmd+V (粘贴) — keycode 9 = 'v'
    let paste_down = CGEvent::new_keyboard_event(source.clone(), 9, true)
        .map_err(|_| AxError::Other("Failed to create key event".to_string()))?;
    paste_down.set_flags(CGEventFlags::CGEventFlagCommand);
    let paste_up = CGEvent::new_keyboard_event(source, 9, false)
        .map_err(|_| AxError::Other("Failed to create key event".to_string()))?;
    paste_up.set_flags(CGEventFlags::CGEventFlagCommand);

    paste_down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(30));
    paste_up.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(100));

    debug!("已通过 Cmd+V 输入: {}", value);
    Ok(())
}

/// 在 Incrementor 中查找并输入值
///
/// Incrementor 是 Qt 的特殊控件，包含一个 TextField 子元素。
/// 先尝试直接设值，失败则使用粘贴方式。
pub fn set_incrementor_value(incrementor: &Element, value: &str) -> AxResult<()> {
    // 查找子 TextField
    if let Some(text_field) = incrementor.find_by_role("AXTextField", 3) {
        // 尝试直接设值
        text_field.set_focused(true)?;
        if text_field.set_string_value(value).is_ok() {
            // 验证
            std::thread::sleep(std::time::Duration::from_millis(50));
            let readback = text_field.value().unwrap_or_default();
            let inc_readback = incrementor.value().unwrap_or_default();
            if readback == value || inc_readback == value {
                return Ok(());
            }
        }
    }

    // 降级到粘贴方式
    set_string_value_via_paste(incrementor, value)
}

/// 获取元素的文本内容
///
/// 尝试获取 AXValue，如果不存在则尝试 AXTitle
pub fn get_element_text(element: &Element) -> Option<String> {
    element.value().or_else(|| element.title())
}

/// 元素匹配器
#[derive(Debug)]
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

/// 使用 Matcher 查找元素
pub fn find_element_with_matcher(
    root: &Element,
    role: &str,
    matcher: &Matcher,
    max_depth: usize,
) -> Option<Element> {
    root.find(|e| {
        if e.role().as_deref() != Some(role) {
            return false;
        }
        match matcher {
            Matcher::Title(expected) => e.title().as_deref() == Some(*expected),
            Matcher::TitleContains(substr) => e.title().map_or(false, |t| t.contains(*substr)),
            Matcher::Identifier(expected) => e.identifier().as_deref() == Some(*expected),
            Matcher::Description(expected) => e.description().as_deref() == Some(*expected),
            Matcher::Any => true,
        }
    }, max_depth)
}

/// 使用 Matcher 查找所有匹配元素
pub fn find_all_elements_with_matcher(
    root: &Element,
    role: &str,
    matcher: &Matcher,
    max_depth: usize,
) -> Vec<Element> {
    root.find_all(|e| {
        if e.role().as_deref() != Some(role) {
            return false;
        }
        match matcher {
            Matcher::Title(expected) => e.title().as_deref() == Some(*expected),
            Matcher::TitleContains(substr) => e.title().map_or(false, |t| t.contains(*substr)),
            Matcher::Identifier(expected) => e.identifier().as_deref() == Some(*expected),
            Matcher::Description(expected) => e.description().as_deref() == Some(*expected),
            Matcher::Any => true,
        }
    }, max_depth)
}
