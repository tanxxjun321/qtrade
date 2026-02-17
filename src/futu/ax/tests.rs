//! futu::ax 模块单元测试
//!
//! 注意：部分测试需要 macOS Accessibility 权限才能运行。
//! 这些测试被标记为 `#[ignore]`，可以使用 `cargo test -- --ignored` 运行。

use crate::futu::ax::{
    action::Matcher,
    app::Application,
    element::Element,
    error::{AxError, AxResult},
    types::{AxElement, CfArray, CfNumber, CfString, CfType, Rect},
};
use core_foundation::base::{CFRetain, TCFType};
use core_foundation::number::CFNumber as CFNumberType;
use core_foundation::string::CFString as CFCString;

// ===== types.rs 测试 =====

#[cfg(test)]
mod types_tests {
    use super::*;

    #[test]
    fn test_ax_element_from_raw_null() {
        let result = unsafe { AxElement::from_raw(std::ptr::null()) };
        assert!(result.is_none());
    }

    #[test]
    fn test_ax_element_is_null() {
        // 只能通过 from_raw 创建，不能直接用 struct literal
        // 这里我们测试 null 返回 None
        let result = unsafe { AxElement::from_raw(std::ptr::null()) };
        assert!(result.is_none());
    }

    #[test]
    fn test_rect_new() {
        let rect = Rect::new(10.0, 20.0, 100.0, 200.0);
        assert_eq!(rect.x, 10.0);
        assert_eq!(rect.y, 20.0);
        assert_eq!(rect.width, 100.0);
        assert_eq!(rect.height, 200.0);
    }

    #[test]
    fn test_rect_clone_copy() {
        let rect = Rect::new(1.0, 2.0, 3.0, 4.0);
        let rect2 = rect;
        assert_eq!(rect.x, rect2.x);
        assert_eq!(rect.y, rect2.y);
    }

    #[test]
    fn test_cftype_boolean_true() {
        let true_ptr = core_foundation::boolean::CFBoolean::true_value().as_CFTypeRef();

        unsafe {
            let true_type = CfType::from_raw(true_ptr).unwrap();
            assert!(matches!(true_type, CfType::Boolean(true)));
        }
    }

    #[test]
    fn test_cftype_boolean_false() {
        let false_ptr = core_foundation::boolean::CFBoolean::false_value().as_CFTypeRef();

        unsafe {
            let false_type = CfType::from_raw(false_ptr).unwrap();
            assert!(matches!(false_type, CfType::Boolean(false)));
        }
    }

    #[test]
    fn test_cftype_null() {
        unsafe {
            let result = CfType::from_raw(std::ptr::null());
            assert!(result.is_none());
        }
    }

    #[test]
    fn test_cfarray_empty() {
        unsafe {
            // 创建空数组
            let empty_arr: core_foundation::array::CFArray<CFCString> =
                core_foundation::array::CFArray::from_CFTypes(&[]);
            let ptr = empty_arr.as_CFTypeRef();
            let cf_array = CfArray::from_raw(ptr).unwrap();

            assert_eq!(cf_array.len(), 0);
            assert!(cf_array.is_empty());
        }
    }

    #[test]
    fn test_cfarray_with_items() {
        unsafe {
            let items = [
                CFCString::new("item1"),
                CFCString::new("item2"),
                CFCString::new("item3"),
            ];
            let arr = core_foundation::array::CFArray::from_CFTypes(&items);
            let ptr = arr.as_CFTypeRef();
            // 增加引用计数，因为我们创建 CfArray 会负责释放
            CFRetain(ptr);
            let cf_array = CfArray::from_raw(ptr).unwrap();

            assert_eq!(cf_array.len(), 3);
            assert!(!cf_array.is_empty());

            // 测试迭代器
            let collected: Vec<_> = cf_array.iter().collect();
            assert_eq!(collected.len(), 3);
        }
    }

    #[test]
    fn test_cfarray_get_out_of_bounds() {
        unsafe {
            let items = [CFCString::new("only")];
            let arr = core_foundation::array::CFArray::from_CFTypes(&items);
            let ptr = arr.as_CFTypeRef();
            // 增加引用计数，因为我们创建 CfArray 会负责释放
            CFRetain(ptr);
            let cf_array = CfArray::from_raw(ptr).unwrap();

            assert!(cf_array.get(0).is_some());
            assert!(cf_array.get(1).is_none());
            assert!(cf_array.get(100).is_none());
        }
    }

    #[test]
    fn test_cftype_string_roundtrip() {
        unsafe {
            // 创建一个 CFString，并增加引用计数
            let cf_str = CFCString::new("hello world");
            let ptr = cf_str.as_CFTypeRef();
            core_foundation::base::CFRetain(ptr);

            // 现在 ptr 可以被 CfString 包装（它会负责释放）
            let cf_string = CfString::from_raw(ptr).unwrap();
            assert_eq!(cf_string.to_string(), Some("hello world".to_string()));
            // cf_string 在这里 drop，释放引用
        }
    }

    #[test]
    fn test_cftype_as_string_none() {
        // 非字符串类型应该返回 None
        let bool_ptr = core_foundation::boolean::CFBoolean::true_value().as_CFTypeRef();
        unsafe {
            let cf_type = CfType::from_raw(bool_ptr).unwrap();
            assert!(cf_type.as_string().is_none());
        }
    }

    #[test]
    fn test_cfnumber_i64() {
        unsafe {
            let num = CFNumberType::from(42i64);
            let ptr = num.as_CFTypeRef();
            // 增加引用计数，因为我们创建 CfNumber 会负责释放
            CFRetain(ptr);
            let cf_num = CfNumber::from_raw(ptr).unwrap();
            assert_eq!(cf_num.as_i64(), Some(42));
        }
    }

    #[test]
    fn test_cfnumber_f64() {
        unsafe {
            let num = CFNumberType::from(3.14159f64);
            let ptr = num.as_CFTypeRef();
            // 增加引用计数，因为我们创建 CfNumber 会负责释放
            CFRetain(ptr);
            let cf_num = CfNumber::from_raw(ptr).unwrap();
            let val = cf_num.as_f64().unwrap();
            assert!((val - 3.14159).abs() < 0.0001);
        }
    }

    #[test]
    fn test_cfnumber_as_f64_from_i64() {
        // i64 类型的 CFNumber 不能转换为 f64
        unsafe {
            let num = CFNumberType::from(42i64);
            let ptr = num.as_CFTypeRef();
            let cf_num = CfNumber::from_raw(ptr).unwrap();
            // CFNumberGetValue 对类型要求严格，i64 不能直接转 f64
            // 但我们仍然可以获取值
            assert!(cf_num.as_f64().is_none() || cf_num.as_f64() == Some(42.0));
        }
    }
}

// ===== error.rs 测试 =====

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn test_ax_error_from_code_permission_denied() {
        let err = AxError::from_code(-25200);
        assert!(matches!(err, AxError::PermissionDenied));
    }

    #[test]
    fn test_ax_error_from_code_app_not_accessible() {
        let err = AxError::from_code(-25201);
        assert!(matches!(err, AxError::AppNotAccessible));
    }

    #[test]
    fn test_ax_error_from_code_invalid_element() {
        let err = AxError::from_code(-25202);
        assert!(matches!(err, AxError::InvalidElement));
    }

    #[test]
    fn test_ax_error_from_code_attribute_not_found() {
        let err = AxError::from_code(-25203);
        assert!(matches!(err, AxError::AttributeNotFound(_)));
    }

    #[test]
    fn test_ax_error_from_code_api_error() {
        let err = AxError::from_code(-99999);
        assert!(matches!(err, AxError::ApiError(-99999)));
    }

    #[test]
    fn test_ax_error_display_permission() {
        let err = AxError::PermissionDenied;
        let msg = format!("{}", err);
        assert!(msg.contains("权限"));
    }

    #[test]
    fn test_ax_error_display_app_not_accessible() {
        let err = AxError::AppNotAccessible;
        let msg = format!("{}", err);
        assert!(msg.contains("应用") || msg.contains("accessible"));
    }

    #[test]
    fn test_ax_result_ok() {
        let result: AxResult<i32> = Ok(42);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_ax_result_err() {
        let result: AxResult<i32> = Err(AxError::NullPointer);
        assert!(result.is_err());
    }
}

// ===== action.rs 测试 =====

#[cfg(test)]
mod action_tests {
    use super::*;

    #[test]
    fn test_matcher_title() {
        let m = Matcher::Title("Button");
        match m {
            Matcher::Title(s) => assert_eq!(s, "Button"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_matcher_title_contains() {
        let m = Matcher::TitleContains("Submit");
        match m {
            Matcher::TitleContains(s) => assert_eq!(s, "Submit"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_matcher_identifier() {
        let m = Matcher::Identifier("test.id");
        match m {
            Matcher::Identifier(s) => assert_eq!(s, "test.id"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_matcher_description() {
        let m = Matcher::Description("A test button");
        match m {
            Matcher::Description(s) => assert_eq!(s, "A test button"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_matcher_any() {
        let m = Matcher::Any;
        assert!(matches!(m, Matcher::Any));
    }
}

// ===== element.rs 测试 =====

#[cfg(test)]
mod element_tests {
    use super::*;

    #[test]
    fn test_element_from_raw_null() {
        let result = unsafe { Element::from_raw(std::ptr::null()) };
        assert!(result.is_err());
    }

    #[test]
    fn test_element_as_ptr_consistency() {
        // 注意：这个测试假设有一个有效的 AX 元素
        // 在没有 AX 权限的情况下无法运行
    }
}

// ===== 集成测试（需要 AX 权限）=====

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    #[ignore = "需要 AX 权限"]
    fn test_check_permission() {
        // 这个测试检查权限状态，但不失败
        let has_perm = Application::check_permission();
        println!("AX Permission: {}", has_perm);
    }

    #[test]
    #[ignore = "需要 AX 权限"]
    fn test_application_new_current_process() {
        let pid = std::process::id() as i32;
        let result = Application::new(pid);
        assert!(
            result.is_ok(),
            "Failed to create Application: {:?}",
            result.err()
        );

        let app = result.unwrap();
        assert_eq!(app.pid(), pid);
    }

    #[test]
    #[ignore = "需要 AX 权限和富途进程"]
    fn test_find_futu_pid() {
        // 这个测试只在富途运行时有效
        let result = Application::find_futu_pid();
        println!("find_futu_pid result: {:?}", result);
        // 不强制要求成功，因为富途可能没运行
    }

    #[test]
    #[ignore = "需要 AX 权限"]
    fn test_application_windows() {
        let pid = std::process::id() as i32;
        let app = Application::new(pid).expect("Failed to create app");

        let windows = app.windows();
        println!("Windows result: {:?}", windows.is_ok());
    }

    #[test]
    #[ignore = "需要 AX 权限"]
    fn test_element_role() {
        let pid = std::process::id() as i32;
        let app = Application::new(pid).expect("Failed to create app");

        let role = app.element().role();
        println!("App element role: {:?}", role);
    }

    #[test]
    #[ignore = "需要 AX 权限"]
    fn test_element_children() {
        let pid = std::process::id() as i32;
        let app = Application::new(pid).expect("Failed to create app");

        match app.element().children() {
            Ok(children) => {
                println!("Children count: {}", children.len());
            }
            Err(e) => println!("Failed to get children: {}", e),
        }
    }

    #[test]
    #[ignore = "需要 AX 权限"]
    fn test_element_find_by_role() {
        let pid = std::process::id() as i32;
        let app = Application::new(pid).expect("Failed to create app");

        // 查找窗口（可能没有）
        let window = app.element().find_by_role("AXWindow", 5);
        println!("Found window: {}", window.is_some());
    }

    #[test]
    #[ignore = "需要 AX 权限"]
    fn test_application_dump_tree() {
        let pid = std::process::id() as i32;
        let app = Application::new(pid).expect("Failed to create app");

        let dump = app.dump_tree(2);
        assert!(!dump.is_empty());
        // 打印前 500 字符
        println!(
            "Tree dump (first 500 chars):\n{}",
            &dump[..dump.len().min(500)]
        );
    }

    #[test]
    #[ignore = "需要 AX 权限"]
    fn test_application_name() {
        let pid = std::process::id() as i32;
        let app = Application::new(pid).expect("Failed to create app");

        let name = app.name();
        println!("App name: {:?}", name);
    }

    #[test]
    #[ignore = "需要 AX 权限"]
    fn test_element_find_by_title() {
        let pid = std::process::id() as i32;
        let app = Application::new(pid).expect("Failed to create app");

        // 查找标题包含 "test" 的元素（可能没有）
        let found = app.element().find_by_title_contains("test", 3);
        println!("Found by title: {}", found.is_some());
    }
}

// ===== 边界情况和错误处理测试 =====

#[cfg(test)]
mod edge_case_tests {
    use super::*;

    #[test]
    fn test_ax_element_into_raw() {
        unsafe {
            // 创建一个临时的 CFString 作为测试对象
            let cf_str = CFCString::new("test");
            let ptr = cf_str.as_CFTypeRef();

            // 使用 retain 创建 AxElement（增加引用计数）
            let elem = AxElement::retain(ptr).unwrap();
            let raw_ptr = elem.into_raw();

            // 现在 raw_ptr 是有效的，我们需要释放它
            assert!(!raw_ptr.is_null());
            core_foundation::base::CFRelease(raw_ptr);
        }
    }

    #[test]
    fn test_ax_element_retain_null() {
        unsafe {
            let result = AxElement::retain(std::ptr::null());
            assert!(result.is_none());
        }
    }

    #[test]
    fn test_cftype_as_array_none() {
        // 布尔类型不能转换为数组
        unsafe {
            let bool_ptr = core_foundation::boolean::CFBoolean::true_value().as_CFTypeRef();
            let cf_type = CfType::from_raw(bool_ptr).unwrap();
            assert!(cf_type.as_array().is_none());
        }
    }

    #[test]
    fn test_cftype_as_element_none() {
        // 字符串不能转换为元素
        unsafe {
            let str = CFCString::new("test");
            let ptr = str.as_CFTypeRef();
            let cf_type = CfType::from_raw(ptr).unwrap();
            assert!(cf_type.as_element().is_none());
        }
    }

    #[test]
    fn test_cftype_as_f64_none() {
        // 字符串不能转换为数字
        unsafe {
            let str = CFCString::new("123");
            let ptr = str.as_CFTypeRef();
            let cf_type = CfType::from_raw(ptr).unwrap();
            assert!(cf_type.as_f64().is_none());
        }
    }

    #[test]
    fn test_rect_zero() {
        let rect = Rect::new(0.0, 0.0, 0.0, 0.0);
        assert_eq!(rect.x, 0.0);
        assert_eq!(rect.y, 0.0);
        assert_eq!(rect.width, 0.0);
        assert_eq!(rect.height, 0.0);
    }

    #[test]
    fn test_rect_negative() {
        // 负坐标在某些情况下是有效的
        let rect = Rect::new(-10.0, -20.0, 100.0, 200.0);
        assert_eq!(rect.x, -10.0);
        assert_eq!(rect.y, -20.0);
    }
}
