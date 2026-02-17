//! AXUIElement 安全操作 API

use super::error::{AxError, AxResult};
use super::ffi::*;
use super::types::{AxElement, CfType, Rect};
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::string::CFString;

/// 可访问性元素
///
/// 包装 AXUIElementRef，提供类型安全的操作接口
pub struct Element {
    inner: AxElement,
}

impl Element {
    /// 从原始 AXUIElement 创建
    /// 安全性：接管指针所有权，不增加引用计数
    pub unsafe fn from_raw(ptr: CFTypeRef) -> AxResult<Self> {
        AxElement::from_raw(ptr)
            .map(|inner| Self { inner })
            .ok_or(AxError::NullPointer)
    }

    /// 从包装器创建
    pub fn from_wrapper(wrapper: AxElement) -> Self {
        Self { inner: wrapper }
    }

    /// 获取内部元素的克隆（增加引用计数）
    pub fn clone_inner(&self) -> Option<AxElement> {
        unsafe { AxElement::retain(self.inner.as_ptr()) }
    }

    /// 获取内部裸指针
    pub fn as_ptr(&self) -> CFTypeRef {
        self.inner.as_ptr()
    }

    /// 获取属性值
    pub fn attribute(&self, name: &str) -> AxResult<CfType> {
        let attr_name = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();

        let result =
            unsafe { AXUIElementCopyAttributeValue(self.inner.as_ptr(), attr_name.as_concrete_TypeRef(), &mut value) };

        if result != AX_ERROR_SUCCESS {
            return Err(AxError::from_code(result));
        }

        if value.is_null() {
            return Err(AxError::AttributeNotFound(name.to_string()));
        }

        unsafe { CfType::from_raw(value).ok_or(AxError::NullPointer) }
    }

    /// 获取字符串属性
    pub fn string_attribute(&self, name: &str) -> Option<String> {
        self.attribute(name).ok().and_then(|t| t.as_string())
    }

    /// 获取角色
    pub fn role(&self) -> Option<String> {
        self.string_attribute("AXRole")
    }

    /// 获取子角色
    pub fn subrole(&self) -> Option<String> {
        self.string_attribute("AXSubrole")
    }

    /// 获取是否最小化（仅适用于窗口）
    pub fn is_minimized(&self) -> bool {
        match self.attribute("AXMinimized") {
            Ok(CfType::Boolean(b)) => b,
            _ => false,
        }
    }

    /// 设置最小化状态（仅适用于窗口）
    pub fn set_minimized(&self, minimized: bool) -> AxResult<()> {
        self.set_attribute_bool("AXMinimized", minimized)
    }

    /// 获取值
    pub fn value(&self) -> Option<String> {
        self.string_attribute("AXValue")
    }

    /// 获取标题
    pub fn title(&self) -> Option<String> {
        self.string_attribute("AXTitle")
    }

    /// 获取描述
    pub fn description(&self) -> Option<String> {
        self.string_attribute("AXDescription")
    }

    /// 获取标识符
    pub fn identifier(&self) -> Option<String> {
        self.string_attribute("AXIdentifier")
    }

    /// 获取帮助文本
    pub fn help(&self) -> Option<String> {
        self.string_attribute("AXHelp")
    }

    /// 获取子元素列表
    pub fn children(&self) -> AxResult<Vec<Element>> {
        let attr = self.attribute("AXChildren")?;
        let array = attr.as_array().ok_or(AxError::TypeMismatch {
            expected: "array".to_string(),
            actual: "other".to_string(),
        })?;

        let mut elements = Vec::with_capacity(array.len());
        for ptr in array.iter() {
            if let Some(elem) = unsafe { AxElement::retain(ptr) } {
                elements.push(Element::from_wrapper(elem));
            }
        }

        Ok(elements)
    }

    /// 获取直接子元素数量
    pub fn child_count(&self) -> usize {
        match self.attribute("AXChildren") {
            Ok(CfType::Array(arr)) => arr.len(),
            _ => 0,
        }
    }

    /// 获取窗口列表（仅适用于应用元素）
    pub fn windows(&self) -> AxResult<Vec<Element>> {
        let attr = self.attribute("AXWindows")?;
        let array = attr.as_array().ok_or(AxError::TypeMismatch {
            expected: "array".to_string(),
            actual: "other".to_string(),
        })?;

        let mut elements = Vec::with_capacity(array.len());
        for ptr in array.iter() {
            if let Some(elem) = unsafe { AxElement::retain(ptr) } {
                elements.push(Element::from_wrapper(elem));
            }
        }

        Ok(elements)
    }

    /// 获取元素框架 (x, y, width, height)
    pub fn frame(&self) -> AxResult<Rect> {
        // 获取位置
        let pos = self.attribute("AXPosition")?;
        let ax_value = match pos {
            CfType::Value(v) => v,
            _ => {
                return Err(AxError::TypeMismatch {
                    expected: "AXValue".to_string(),
                    actual: "other".to_string(),
                })
            }
        };

        let (x, y) = ax_value
            .as_point()
            .ok_or(AxError::FrameParseFailed("无法解析 AXPosition".to_string()))?;

        // 获取大小
        let size = self.attribute("AXSize")?;
        let ax_value = match size {
            CfType::Value(v) => v,
            _ => {
                return Err(AxError::TypeMismatch {
                    expected: "AXValue".to_string(),
                    actual: "other".to_string(),
                })
            }
        };

        let (width, height) = ax_value
            .as_size()
            .ok_or(AxError::FrameParseFailed("无法解析 AXSize".to_string()))?;

        Ok(Rect::new(x, y, width, height))
    }

    /// 获取元素位置
    pub fn position(&self) -> AxResult<(f64, f64)> {
        let pos = self.attribute("AXPosition")?;
        match pos {
            CfType::Value(v) => v
                .as_point()
                .ok_or(AxError::FrameParseFailed("无法解析 AXPosition".to_string())),
            _ => Err(AxError::TypeMismatch {
                expected: "AXValue".to_string(),
                actual: "other".to_string(),
            }),
        }
    }

    /// 获取元素大小
    pub fn size(&self) -> AxResult<(f64, f64)> {
        let size = self.attribute("AXSize")?;
        match size {
            CfType::Value(v) => v
                .as_size()
                .ok_or(AxError::FrameParseFailed("无法解析 AXSize".to_string())),
            _ => Err(AxError::TypeMismatch {
                expected: "AXValue".to_string(),
                actual: "other".to_string(),
            }),
        }
    }

    /// 获取聚焦的元素
    pub fn focused_element(&self) -> AxResult<Element> {
        let attr = self.attribute("AXFocusedUIElement")?;
        match attr {
            CfType::Element(e) => Ok(Element::from_wrapper(unsafe {
                AxElement::retain(e.as_ptr()).ok_or(AxError::NullPointer)?
            })),
            _ => Err(AxError::TypeMismatch {
                expected: "element".to_string(),
                actual: "other".to_string(),
            }),
        }
    }

    /// 获取选中的子元素
    pub fn selected_children(&self) -> AxResult<Vec<Element>> {
        let attr = self.attribute("AXSelectedChildren")?;
        let array = attr.as_array().ok_or(AxError::TypeMismatch {
            expected: "array".to_string(),
            actual: "other".to_string(),
        })?;

        let mut elements = Vec::with_capacity(array.len());
        for ptr in array.iter() {
            if let Some(elem) = unsafe { AxElement::retain(ptr) } {
                elements.push(Element::from_wrapper(elem));
            }
        }

        Ok(elements)
    }

    /// 获取可见子元素
    pub fn visible_children(&self) -> AxResult<Vec<Element>> {
        let attr = self.attribute("AXVisibleChildren")?;
        let array = attr.as_array().ok_or(AxError::TypeMismatch {
            expected: "array".to_string(),
            actual: "other".to_string(),
        })?;

        let mut elements = Vec::with_capacity(array.len());
        for ptr in array.iter() {
            if let Some(elem) = unsafe { AxElement::retain(ptr) } {
                elements.push(Element::from_wrapper(elem));
            }
        }

        Ok(elements)
    }

    /// 执行操作
    pub fn perform_action(&self, action: &str) -> AxResult<()> {
        let action_name = CFString::new(action);
        let result = unsafe { AXUIElementPerformAction(self.inner.as_ptr(), action_name.as_concrete_TypeRef()) };

        if result != AX_ERROR_SUCCESS {
            return Err(AxError::from_code(result));
        }

        Ok(())
    }

    /// 点击元素（执行 AXPress 操作）
    pub fn click(&self) -> AxResult<()> {
        self.perform_action("AXPress")
    }

    /// 确认元素（执行 AXConfirm 操作）
    pub fn confirm(&self) -> AxResult<()> {
        self.perform_action("AXConfirm")
    }

    /// 取消元素（执行 AXCancel 操作）
    pub fn cancel(&self) -> AxResult<()> {
        self.perform_action("AXCancel")
    }

    /// 递增元素值
    pub fn increment(&self) -> AxResult<()> {
        self.perform_action("AXIncrement")
    }

    /// 递减元素值
    pub fn decrement(&self) -> AxResult<()> {
        self.perform_action("AXDecrement")
    }

    /// 显示菜单
    pub fn show_menu(&self) -> AxResult<()> {
        self.perform_action("AXShowMenu")
    }

    /// 设置字符串值
    pub fn set_string_value(&self, value: &str) -> AxResult<()> {
        let attr_name = CFString::new("AXValue");
        let value_string = CFString::new(value);

        let result = unsafe {
            AXUIElementSetAttributeValue(
                self.inner.as_ptr(),
                attr_name.as_concrete_TypeRef(),
                value_string.as_concrete_TypeRef() as CFTypeRef,
            )
        };

        if result != AX_ERROR_SUCCESS {
            return Err(AxError::from_code(result));
        }

        Ok(())
    }

    /// 设置布尔属性值
    pub fn set_attribute_bool(&self, attribute: &str, value: bool) -> AxResult<()> {
        let attr_name = CFString::new(attribute);
        let cf_value = if value {
            core_foundation::boolean::CFBoolean::true_value()
        } else {
            core_foundation::boolean::CFBoolean::false_value()
        };

        let result = unsafe {
            AXUIElementSetAttributeValue(
                self.inner.as_ptr(),
                attr_name.as_concrete_TypeRef(),
                cf_value.as_concrete_TypeRef() as CFTypeRef,
            )
        };

        if result != AX_ERROR_SUCCESS {
            return Err(AxError::from_code(result));
        }

        Ok(())
    }

    /// 设置聚焦
    pub fn set_focused(&self, focused: bool) -> AxResult<()> {
        let attr_name = CFString::new("AXFocused");
        let value = if focused {
            core_foundation::boolean::CFBoolean::true_value()
        } else {
            core_foundation::boolean::CFBoolean::false_value()
        };

        let result = unsafe {
            AXUIElementSetAttributeValue(
                self.inner.as_ptr(),
                attr_name.as_concrete_TypeRef(),
                value.as_concrete_TypeRef() as CFTypeRef,
            )
        };

        if result != AX_ERROR_SUCCESS {
            return Err(AxError::from_code(result));
        }

        Ok(())
    }

    /// 判断是否为启用状态
    pub fn is_enabled(&self) -> bool {
        match self.attribute("AXEnabled") {
            Ok(CfType::Boolean(b)) => b,
            _ => false,
        }
    }

    /// 递归查找元素
    ///
    /// 在子树中查找匹配条件的元素
    pub fn find<F>(&self, predicate: F, max_depth: usize) -> Option<Element>
    where
        F: Fn(&Element) -> bool,
    {
        self.find_recursive(&predicate, 0, max_depth)
    }

    fn find_recursive<F>(&self, predicate: &F, depth: usize, max_depth: usize) -> Option<Element>
    where
        F: Fn(&Element) -> bool,
    {
        if depth > max_depth {
            return None;
        }

        // 先检查自己
        if predicate(self) {
            // 返回自己的克隆
            return self.clone_inner().map(Element::from_wrapper);
        }

        // 递归检查子元素
        if let Ok(children) = self.children() {
            for child in children {
                if let Some(found) = child.find_recursive(predicate, depth + 1, max_depth) {
                    return Some(found);
                }
            }
        }

        None
    }

    /// 根据角色查找元素
    pub fn find_by_role(&self, role: &str, max_depth: usize) -> Option<Element> {
        let target = role.to_string();
        self.find(|e| e.role().as_ref() == Some(&target), max_depth)
    }

    /// 根据标识符查找元素
    pub fn find_by_identifier(&self, identifier: &str, max_depth: usize) -> Option<Element> {
        let target = identifier.to_string();
        self.find(|e| e.identifier().as_ref() == Some(&target), max_depth)
    }

    /// 根据标题查找元素
    pub fn find_by_title(&self, title: &str, max_depth: usize) -> Option<Element> {
        let target = title.to_string();
        self.find(|e| e.title().as_ref() == Some(&target), max_depth)
    }

    /// 根据标题包含查找元素
    pub fn find_by_title_contains(&self, substring: &str, max_depth: usize) -> Option<Element> {
        let target = substring.to_string();
        self.find(|e| e.title().as_ref().map_or(false, |t| t.contains(&target)), max_depth)
    }

    /// 收集所有匹配的元素
    pub fn find_all<F>(&self, predicate: F, max_depth: usize) -> Vec<Element>
    where
        F: Fn(&Element) -> bool,
    {
        let mut results = Vec::new();
        self.find_all_recursive(&predicate, 0, max_depth, &mut results);
        results
    }

    fn find_all_recursive<F>(&self, predicate: &F, depth: usize, max_depth: usize, results: &mut Vec<Element>)
    where
        F: Fn(&Element) -> bool,
    {
        if depth > max_depth {
            return;
        }

        if predicate(self) {
            if let Some(clone) = self.clone_inner() {
                results.push(Element::from_wrapper(clone));
            }
        }

        if let Ok(children) = self.children() {
            for child in children {
                child.find_all_recursive(predicate, depth + 1, max_depth, results);
            }
        }
    }

    /// 查找所有特定角色的元素
    pub fn find_all_by_role(&self, role: &str, max_depth: usize) -> Vec<Element> {
        let target = role.to_string();
        self.find_all(|e| e.role().as_ref() == Some(&target), max_depth)
    }

    /// 获取元素树摘要（用于调试）
    pub fn dump_tree(&self, max_depth: usize) -> String {
        let mut output = String::new();
        self.dump_recursive(0, max_depth, &mut output);
        output
    }

    fn dump_recursive(&self, depth: usize, max_depth: usize, output: &mut String) {
        if depth > max_depth {
            return;
        }

        let indent = "  ".repeat(depth);
        let role = self.role().unwrap_or_else(|| "?".to_string());
        let title = self.title();
        let value = self.value();
        let id = self.identifier();

        output.push_str(&format!("{}{}", indent, role));
        if let Some(t) = title {
            output.push_str(&format!(" title=\"{}\"", t));
        }
        if let Some(v) = value {
            output.push_str(&format!(" value=\"{}\"", v));
        }
        if let Some(i) = id {
            output.push_str(&format!(" id=\"{}\"", i));
        }
        output.push('\n');

        // 限制子元素数量
        let max_children = if depth >= 4 { 20 } else { 50 };
        if let Ok(children) = self.children() {
            let count = children.len().min(max_children);
            for (_i, child) in children.iter().take(count).enumerate() {
                child.dump_recursive(depth + 1, max_depth, output);
            }
            if children.len() > max_children {
                output.push_str(&format!(
                    "{}  ... ({} more children)\n",
                    indent,
                    children.len() - max_children
                ));
            }
        }
    }
}

impl Clone for Element {
    fn clone(&self) -> Self {
        unsafe {
            AxElement::retain(self.inner.as_ptr())
                .map(Element::from_wrapper)
                .expect("Failed to retain element")
        }
    }
}
