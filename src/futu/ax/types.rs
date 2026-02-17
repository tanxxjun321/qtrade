//! CoreFoundation 类型 RAII 包装器
//!
//! 为 CoreFoundation 对象提供自动内存管理

use super::ffi::*;
use core_foundation::base::{CFTypeID, CFTypeRef, TCFType};
use std::ffi::c_void;

/// AXUIElement 包装器
pub struct AxElement(CFTypeRef);

impl AxElement {
    /// 从裸指针创建，不增加引用计数
    /// 安全性：调用者必须确保指针有效，且所有权转移给本对象
    pub unsafe fn from_raw(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    /// 从裸指针创建，增加引用计数
    /// 安全性：调用者必须确保指针有效
    pub unsafe fn retain(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            CFRetain(ptr);
            Some(Self(ptr))
        }
    }

    /// 获取内部裸指针
    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }

    /// 提取内部裸指针，不释放所有权
    /// 调用后，调用者负责管理返回的指针的引用计数
    pub fn into_raw(mut self) -> CFTypeRef {
        let ptr = self.0;
        self.0 = std::ptr::null();
        ptr
    }

    /// 检查是否为空
    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    /// 获取类型 ID
    pub fn type_id(&self) -> CFTypeID {
        unsafe { CFGetTypeID(self.0) }
    }

    /// 检查是否为 AXUIElement 类型
    pub fn is_element(&self) -> bool {
        unsafe { self.type_id() == AXUIElementGetTypeID() }
    }
}

impl Drop for AxElement {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
            self.0 = std::ptr::null();
        }
    }
}

// CFTypeRef 实际上是线程安全的（引用计数），标记为 Send + Sync
unsafe impl Send for AxElement {}
unsafe impl Sync for AxElement {}

// 禁止 Clone，避免引用计数问题
// 如果需要共享，使用 Arc<Element>

/// AXValue 包装器
pub struct AxValue(CFTypeRef);

impl AxValue {
    /// 从裸指针创建，不增加引用计数
    pub unsafe fn from_raw(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    /// 获取内部裸指针
    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }

    /// 提取 CGPoint {x, y}
    pub fn as_point(&self) -> Option<(f64, f64)> {
        let mut point: [f64; 2] = [0.0, 0.0];
        let ok = unsafe {
            AXValueGetValue(self.0, K_AX_VALUE_TYPE_CG_POINT, point.as_mut_ptr() as *mut c_void)
        };
        if ok {
            Some((point[0], point[1]))
        } else {
            None
        }
    }

    /// 提取 CGSize {width, height}
    pub fn as_size(&self) -> Option<(f64, f64)> {
        let mut size: [f64; 2] = [0.0, 0.0];
        let ok = unsafe {
            AXValueGetValue(self.0, K_AX_VALUE_TYPE_CG_SIZE, size.as_mut_ptr() as *mut c_void)
        };
        if ok {
            Some((size[0], size[1]))
        } else {
            None
        }
    }

    /// 提取 CGRect {x, y, width, height}
    pub fn as_rect(&self) -> Option<(f64, f64, f64, f64)> {
        let mut rect: [f64; 4] = [0.0, 0.0, 0.0, 0.0];
        let ok = unsafe {
            AXValueGetValue(self.0, K_AX_VALUE_TYPE_CG_RECT, rect.as_mut_ptr() as *mut c_void)
        };
        if ok {
            Some((rect[0], rect[1], rect[2], rect[3]))
        } else {
            None
        }
    }
}

impl Drop for AxValue {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
            self.0 = std::ptr::null();
        }
    }
}

/// CFArray 包装器
pub struct CfArray(CFTypeRef);

impl CfArray {
    /// 从裸指针创建，不增加引用计数
    pub unsafe fn from_raw(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    /// 获取内部裸指针
    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }

    /// 检查是否为有效的 CFArray
    pub fn is_valid(&self) -> bool {
        unsafe { CFGetTypeID(self.0) == CFArrayGetTypeID() }
    }

    /// 获取元素数量
    pub fn len(&self) -> usize {
        let count = unsafe { CFArrayGetCount(self.0) };
        count.max(0) as usize
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 获取指定索引的元素（不增加引用计数）
    pub fn get(&self, index: usize) -> Option<CFTypeRef> {
        if index >= self.len() {
            return None;
        }
        let ptr = unsafe { CFArrayGetValueAtIndex(self.0, index as isize) };
        if ptr.is_null() {
            None
        } else {
            Some(ptr)
        }
    }

    /// 遍历数组
    pub fn iter(&self) -> CfArrayIter {
        CfArrayIter {
            array: self,
            index: 0,
            count: self.len(),
        }
    }
}

impl Drop for CfArray {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
            self.0 = std::ptr::null();
        }
    }
}

/// CFArray 迭代器
pub struct CfArrayIter<'a> {
    array: &'a CfArray,
    index: usize,
    count: usize,
}

impl<'a> Iterator for CfArrayIter<'a> {
    type Item = CFTypeRef;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.count {
            return None;
        }
        let item = self.array.get(self.index);
        self.index += 1;
        item
    }
}

/// CFString 包装器
pub struct CfString(CFTypeRef);

impl CfString {
    /// 从裸指针创建，不增加引用计数
    pub unsafe fn from_raw(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    /// 获取内部裸指针
    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }

    /// 转换为 Rust String
    pub fn to_string(&self) -> Option<String> {
        const BUFFER_SIZE: isize = 4096;
        let mut buffer: Vec<u8> = vec![0; BUFFER_SIZE as usize];

        let ok = unsafe {
            CFStringGetCString(
                self.0 as _,
                buffer.as_mut_ptr(),
                BUFFER_SIZE,
                K_CF_STRING_ENCODING_UTF8,
            )
        };

        if ok {
            // 找到 null 终止符
            let len = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
            buffer.truncate(len);
            String::from_utf8(buffer).ok()
        } else {
            None
        }
    }
}

impl Drop for CfString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
            self.0 = std::ptr::null();
        }
    }
}

/// CFNumber 包装器
pub struct CfNumber(CFTypeRef);

impl CfNumber {
    /// 从裸指针创建，不增加引用计数
    pub unsafe fn from_raw(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    /// 获取内部裸指针
    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }

    /// 转换为 i64
    pub fn as_i64(&self) -> Option<i64> {
        let mut value: i64 = 0;
        let ok = unsafe {
            CFNumberGetValue(self.0, CFNumberType::Int64, &mut value as *mut _ as *mut c_void)
        };
        if ok {
            Some(value)
        } else {
            None
        }
    }

    /// 转换为 f64
    pub fn as_f64(&self) -> Option<f64> {
        let mut value: f64 = 0.0;
        let ok = unsafe {
            CFNumberGetValue(self.0, CFNumberType::Float64, &mut value as *mut _ as *mut c_void)
        };
        if ok {
            Some(value)
        } else {
            None
        }
    }
}

impl Drop for CfNumber {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
            self.0 = std::ptr::null();
        }
    }
}

/// 通用的 CFType 包装器
pub enum CfType {
    String(CfString),
    Array(CfArray),
    Number(CfNumber),
    Element(AxElement),
    Value(AxValue),
    Boolean(bool),
    Unknown(CFTypeRef),
}

impl CfType {
    /// 从裸指针创建，自动识别类型
    /// 安全性：调用者必须确保指针有效
    pub unsafe fn from_raw(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            return None;
        }

        let type_id = CFGetTypeID(ptr);

        if type_id == CFStringGetTypeID() {
            CfString::from_raw(ptr).map(CfType::String)
        } else if type_id == CFArrayGetTypeID() {
            CfArray::from_raw(ptr).map(CfType::Array)
        } else if type_id == CFNumberGetTypeID() {
            CfNumber::from_raw(ptr).map(CfType::Number)
        } else if type_id == AXUIElementGetTypeID() {
            AxElement::from_raw(ptr).map(CfType::Element)
        } else if type_id == AXValueGetTypeID() {
            AxValue::from_raw(ptr).map(CfType::Value)
        } else if type_id == CFBooleanGetTypeID() {
            // CFBoolean 不需要包装，直接提取值
            let true_ptr = core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef() as CFTypeRef;
            Some(CfType::Boolean(ptr == true_ptr))
        } else {
            Some(CfType::Unknown(ptr))
        }
    }

    /// 尝试转换为字符串
    pub fn as_string(&self) -> Option<String> {
        match self {
            CfType::String(s) => s.to_string(),
            _ => None,
        }
    }

    /// 尝试转换为数组
    pub fn as_array(&self) -> Option<&CfArray> {
        match self {
            CfType::Array(a) => Some(a),
            _ => None,
        }
    }

    /// 尝试转换为数字
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            CfType::Number(n) => n.as_f64(),
            _ => None,
        }
    }

    /// 尝试转换为元素
    pub fn as_element(&self) -> Option<&AxElement> {
        match self {
            CfType::Element(e) => Some(e),
            _ => None,
        }
    }
}

/// 矩形结构
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }
}
