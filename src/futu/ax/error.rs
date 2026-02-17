//! 错误类型定义

use std::fmt;
use std::error::Error;

/// AX 操作错误
#[derive(Debug)]
pub enum AxError {
    ApiError(i32),
    InvalidElement,
    AttributeNotFound(String),
    TypeMismatch { expected: String, actual: String },
    NullPointer,
    PermissionDenied,
    AppNotAccessible,
    WindowNotFound,
    ElementNotFound(String),
    FrameParseFailed(String),
    Other(String),
}

impl fmt::Display for AxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AxError::ApiError(code) => write!(f, "AX API 错误: 代码 {}", code),
            AxError::InvalidElement => write!(f, "无效的元素"),
            AxError::AttributeNotFound(attr) => write!(f, "属性不存在: {}", attr),
            AxError::TypeMismatch { expected, actual } => {
                write!(f, "类型转换失败: 期望 {}, 实际 {}", expected, actual)
            }
            AxError::NullPointer => write!(f, "空指针"),
            AxError::PermissionDenied => {
                write!(f, "权限被拒绝: 请在系统设置中授予辅助功能权限")
            }
            AxError::AppNotAccessible => write!(f, "应用未运行或无法访问"),
            AxError::WindowNotFound => write!(f, "未找到窗口"),
            AxError::ElementNotFound(id) => write!(f, "未找到元素: {}", id),
            AxError::FrameParseFailed(msg) => write!(f, "框架解析失败: {}", msg),
            AxError::Other(msg) => write!(f, "其他错误: {}", msg),
        }
    }
}

impl Error for AxError {}

/// AX 操作结果
pub type AxResult<T> = Result<T, AxError>;

impl AxError {
    /// 根据错误码创建错误
    pub fn from_code(code: i32) -> Self {
        match code {
            AX_ERROR_SUCCESS => unreachable!("Success is not an error"),
            -25200 => AxError::PermissionDenied,
            -25201 => AxError::AppNotAccessible,
            -25202 => AxError::InvalidElement,
            -25203 => AxError::AttributeNotFound("unknown".to_string()),
            -25204 => AxError::Other("参数错误".to_string()),
            _ => AxError::ApiError(code),
        }
    }
}

/// 错误码常量
const AX_ERROR_SUCCESS: i32 = 0;
