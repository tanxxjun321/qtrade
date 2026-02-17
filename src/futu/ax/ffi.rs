//! macOS Accessibility API FFI 声明
//!
//! 此模块包含所有 unsafe 的 FFI 声明，仅供内部使用。
//! 外部代码应通过 safe 模块访问。

use core_foundation::base::{CFTypeID, CFTypeRef};
use core_foundation::dictionary::CFDictionaryRef;
use core_foundation::string::CFStringRef;
use std::ffi::c_void;

/// AX 错误码
pub const AX_ERROR_SUCCESS: i32 = 0;

/// AXValue 类型常量
pub const K_AX_VALUE_TYPE_CG_POINT: u32 = 1;
pub const K_AX_VALUE_TYPE_CG_SIZE: u32 = 2;
pub const K_AX_VALUE_TYPE_CG_RECT: u32 = 3;

extern "C" {
    /// 创建应用的 AXUIElement
    pub fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;

    /// 复制属性值
    pub fn AXUIElementCopyAttributeValue(element: CFTypeRef, attribute: CFStringRef, value: *mut CFTypeRef) -> i32;

    /// 复制所有属性名
    pub fn AXUIElementCopyAttributeNames(element: CFTypeRef, names: *mut CFTypeRef) -> i32;

    /// 设置属性值
    pub fn AXUIElementSetAttributeValue(element: CFTypeRef, attribute: CFStringRef, value: CFTypeRef) -> i32;

    /// 执行操作
    pub fn AXUIElementPerformAction(element: CFTypeRef, action: CFStringRef) -> i32;

    /// 检查权限
    pub fn AXIsProcessTrusted() -> bool;

    /// 请求权限
    pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;

    /// 从 AXValue 提取值
    pub fn AXValueGetValue(value: CFTypeRef, typ: u32, out: *mut c_void) -> bool;

    /// 创建 AXValue
    pub fn AXValueCreate(typ: u32, value: *const c_void) -> CFTypeRef;

    // CoreFoundation 类型检查
    pub fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    pub fn CFStringGetTypeID() -> CFTypeID;
    pub fn CFArrayGetTypeID() -> CFTypeID;
    pub fn CFNumberGetTypeID() -> CFTypeID;
    pub fn CFBooleanGetTypeID() -> CFTypeID;
    pub fn CFDataGetTypeID() -> CFTypeID;
    pub fn AXUIElementGetTypeID() -> CFTypeID;
    pub fn AXValueGetTypeID() -> CFTypeID;

    /// 复制描述
    pub fn CFCopyDescription(cf: CFTypeRef) -> CFStringRef;

    /// 释放 CF 对象
    pub fn CFRelease(cf: CFTypeRef);

    /// 保留 CF 对象
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;

    // CFArray 操作
    pub fn CFArrayGetCount(array: CFTypeRef) -> isize;
    pub fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;

    // CFString 操作
    pub fn CFStringGetCString(string: CFStringRef, buffer: *mut u8, buffer_size: isize, encoding: u32) -> bool;

    // CFNumber 操作
    pub fn CFNumberGetValue(number: CFTypeRef, typ: CFNumberType, value: *mut c_void) -> bool;
}

/// CFNumber 类型
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub enum CFNumberType {
    Int8 = 1,
    Int16 = 2,
    Int32 = 3,
    Int64 = 4,
    Float32 = 5,
    Float64 = 6,
    Char = 7,
    Short = 8,
    Int = 9,
    Long = 10,
    LongLong = 11,
    Float = 12,
    Double = 13,
}

/// UTF-8 编码
pub const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;
