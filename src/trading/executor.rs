//! 交易执行器 — 通过 AX API 驱动财富通V5.0完成港股委托
//!
//! 大部分操作通过后台 AX API 完成（表单填写、按钮点击、弹窗验证）。
//! 导航和 AXIncrementor 输入需要短暂激活窗口（前台 CGEvent 点击/粘贴）。
//! 安全不变量：验价通过后才点击确认，任何步骤失败自动清理弹窗。

use anyhow::{Context, Result};
use core_foundation::base::CFTypeRef;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::futu::ax_action;
use crate::futu::accessibility::AccessibilityReader;

/// 交易方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl std::fmt::Display for OrderSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "买入"),
            OrderSide::Sell => write!(f, "卖出"),
        }
    }
}

/// 委托请求
#[derive(Debug, Clone)]
pub struct OrderRequest {
    /// 股票代码（如 "00700"）
    pub stock_code: String,
    /// 委托价格 (HKD)
    pub price: f64,
    /// 委托数量（股）
    pub quantity: u32,
    /// 买入/卖出
    pub side: OrderSide,
}

/// 委托结果
#[derive(Debug, Clone, Serialize)]
pub struct OrderResult {
    pub success: bool,
    pub message: String,
    pub verified_price: Option<f64>,
    pub verified_code: Option<String>,
    pub timestamp: String,
}

/// 交易执行器
pub struct TradingExecutor {
    app_pid: i32,
}

impl TradingExecutor {
    /// 创建执行器，自动查找财富通进程
    pub fn new() -> Result<Self> {
        // 检查 AX 权限
        if !AccessibilityReader::check_permission() {
            anyhow::bail!(
                "辅助功能权限未授权。请在 系统设置 → 隐私与安全性 → 辅助功能 中授权 qtrade。"
            );
        }

        // 查找财富通进程
        let pid = find_cft5_pid()
            .context("未找到财富通进程。请确认 财富通V5.0体验版 已启动。")?;

        // 验证能获取窗口
        let app = ax_action::create_app_element(pid)?;
        let _window = get_cft5_main_window(app)
            .context("无法获取财富通窗口，请确认 App 已打开")?;

        info!("TradingExecutor initialized, PID={}", pid);
        Ok(Self { app_pid: pid })
    }

    /// 获取交易客户端 PID
    pub fn pid(&self) -> i32 {
        self.app_pid
    }

    /// 执行委托
    pub async fn execute_order(&self, req: &OrderRequest) -> Result<OrderResult> {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        info!(
            "开始执行委托: {} {} {} 股 @ {:.3} HKD",
            req.side, req.stock_code, req.quantity, req.price
        );

        // 所有 AX 操作需要在 spawn_blocking 中执行（CFTypeRef 不跨线程）
        let pid = self.app_pid;
        let stock_code = req.stock_code.clone();
        let price = req.price;
        let quantity = req.quantity;
        let side = req.side;

        let result = tokio::task::spawn_blocking(move || {
            execute_order_sync(pid, &stock_code, price, quantity, side)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking failed: {}", e))?;

        match result {
            Ok(msg) => {
                info!("委托成功: {}", msg);
                Ok(OrderResult {
                    success: true,
                    message: msg,
                    verified_price: Some(price),
                    verified_code: Some(req.stock_code.clone()),
                    timestamp,
                })
            }
            Err(e) => {
                warn!("委托失败: {}", e);
                Ok(OrderResult {
                    success: false,
                    message: format!("{}", e),
                    verified_price: None,
                    verified_code: None,
                    timestamp,
                })
            }
        }
    }
}

/// 同步执行委托流程（在 spawn_blocking 中运行）
fn execute_order_sync(
    pid: i32,
    stock_code: &str,
    price: f64,
    quantity: u32,
    side: OrderSide,
) -> Result<String> {
    let app = ax_action::create_app_element(pid)?;
    let window = get_cft5_main_window(app)
        .context("无法获取交易窗口")?;

    // 导航到目标面板（唯一需要前台的步骤）
    navigate_to_panel(window, side)?;

    // T1: 输入证券代码
    // 布局：AXStaticText value="证券代码" → 下一个 AXTextField
    let code_field = find_field_after_label(window, "证券代码", "AXTextField")
        .context("未找到证券代码输入框")?;
    ax_action::focus_element(code_field)?;
    ax_action::set_text_field_value(code_field, stock_code)
        .context("输入证券代码失败")?;
    let readback = ax_action::get_value_str(code_field).unwrap_or_default();
    debug!("已输入代码: {} (readback={})", stock_code, readback);

    // 等待代码解析（服务器查询股票信息）
    std::thread::sleep(std::time::Duration::from_millis(800));

    // T2: 输入价格
    // 布局：AXStaticText value="买入价格"/"卖出价格" → 下一个 AXIncrementor
    let price_label = match side {
        OrderSide::Buy => "买入价格",
        OrderSide::Sell => "卖出价格",
    };
    let price_str = format!("{:.3}", price);
    let price_inc = find_field_after_label(window, price_label, "AXIncrementor")
        .context(format!("未找到 '{}' 输入框", price_label))?;
    set_incrementor_value(price_inc, &price_str)
        .context("输入价格失败")?;
    let price_readback = ax_action::get_value_str(price_inc).unwrap_or_default();
    debug!("已输入价格: {} (readback={})", price_str, price_readback);

    // T3: 输入数量
    // 布局：AXStaticText value="买入数量"/"卖出数量" → 下一个 AXIncrementor
    let qty_label = match side {
        OrderSide::Buy => "买入数量",
        OrderSide::Sell => "卖出数量",
    };
    let qty_str = quantity.to_string();
    let qty_field = find_field_after_label(window, qty_label, "AXIncrementor")
        .context(format!("未找到 '{}' 输入框", qty_label))?;
    set_incrementor_value(qty_field, &qty_str)
        .context("输入数量失败")?;
    let qty_readback = ax_action::get_value_str(qty_field).unwrap_or_default();
    debug!("已输入数量: {} (readback={})", qty_str, qty_readback);

    // T4: 点击买入/卖出按钮
    // AXButton title="买入(B)" / "卖出(S)"
    let submit_label = match side {
        OrderSide::Buy => "买入",
        OrderSide::Sell => "卖出",
    };
    let submit_btn = ax_action::find_element(
        window,
        "AXButton",
        &ax_action::Matcher::TitleContains(submit_label),
        10,
    )
    .context(format!("未找到 '{}' 按钮", submit_label))?;

    let btn_title = ax_action::get_title(submit_btn).unwrap_or_default();
    debug!("找到提交按钮: \"{}\"", btn_title);
    // 先尝试 AXPress，失败则用前台坐标点击
    if ax_action::perform_action(submit_btn, "AXPress").is_err() {
        debug!("AXPress 失败，使用前台坐标点击");
        let _ = ax_action::raise_window(window);
        std::thread::sleep(std::time::Duration::from_millis(100));
        ax_action::click_at_element(submit_btn)
            .context(format!("点击 '{}' 按钮失败", submit_label))?;
    }

    // T5: 等待确认弹窗（直接轮询所有窗口，弹窗是独立 AXWindow）
    let confirm_dialog = wait_for_confirm_dialog(pid, 3000)
        .context("等待确认弹窗超时 (3s)")?;
    debug!("确认弹窗已出现");

    // T6: AX 验价 — 从弹窗 AXStaticText 中提取并比对代码和价格
    let verified = verify_dialog_content(confirm_dialog, stock_code, price)?;
    if !verified {
        // 价格不匹配，取消
        try_cancel_dialog(confirm_dialog);
        anyhow::bail!("AX 验价失败：弹窗内容与预期不符");
    }
    debug!("验价通过");

    // T7: 点击确认按钮
    let confirm_btn = find_confirm_button(confirm_dialog)
        .context("未找到确认按钮")?;
    ax_action::perform_action(confirm_btn, "AXPress")
        .context("点击确认按钮失败")?;

    // T8: 检查是否出现错误弹窗（如 "委托失败"/"系统正在初始化" 等）
    std::thread::sleep(std::time::Duration::from_millis(800));
    if let Some(error_msg) = check_error_dialog(pid) {
        anyhow::bail!("委托被拒绝: {}", error_msg);
    }

    info!(
        "委托已提交: {} {} {} 股 @ {} HKD",
        side, stock_code, quantity, price_str
    );

    Ok(format!(
        "{} {} {} 股 @ {} HKD 委托已提交",
        side, stock_code, quantity, price_str
    ))
}

/// 导航到交易面板（港股买入/港股卖出）
///
/// 优先后台：CGEventPostToPSN(Private) 直接发送鼠标点击到进程。
/// 降级前台：raise_window + CGEventPost(HID)。
fn navigate_to_panel(window: CFTypeRef, side: OrderSide) -> Result<()> {
    let target = match side {
        OrderSide::Buy => "港股买入",
        OrderSide::Sell => "港股卖出",
    };

    // 在 AXList 中找到目标 AXStaticText
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Some(elem) = ax_action::find_element(
            window,
            "AXStaticText",
            &ax_action::Matcher::Title(target),
            12,
        ) {
            click_nav_item(window, elem, target)?;
            std::thread::sleep(std::time::Duration::from_millis(1500));
            return Ok(());
        }

        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // 可能不在港股通 tab，先尝试切换
    if let Some(tab) = ax_action::find_element(
        window,
        "AXRadioButton",
        &ax_action::Matcher::Title("港股通"),
        12,
    ) {
        let selected = ax_action::get_value_str(tab)
            .map_or(false, |v| v == "1" || v == "true");
        if !selected {
            debug!("切换到 '港股通' tab");
            ax_action::perform_action(tab, "AXPress")
                .context("点击 '港股通' tab 失败")?;
            std::thread::sleep(std::time::Duration::from_millis(800));

            if let Some(elem) = ax_action::find_element(
                window,
                "AXStaticText",
                &ax_action::Matcher::Title(target),
                12,
            ) {
                click_nav_item(window, elem, target)?;
                std::thread::sleep(std::time::Duration::from_millis(1500));
                return Ok(());
            }
        }
    }

    anyhow::bail!(
        "未找到 '{}' 导航元素。请确认财富通已打开港股通面板。",
        target
    );
}

/// 点击导航菜单项：短暂激活窗口 + 前台坐标点击
///
/// Qt AXList 中的 AXStaticText 不响应 AXPress 和 CGEventPostToPSN，
/// 必须通过前台 CGEventPost(HID) 点击。这是整个交易流程中唯一的前台操作。
fn click_nav_item(window: CFTypeRef, elem: CFTypeRef, label: &str) -> Result<()> {
    let _ = ax_action::raise_window(window);
    std::thread::sleep(std::time::Duration::from_millis(200));
    ax_action::click_at_element(elem)
        .context(format!("点击 '{}' 失败", label))
}

/// 设置 AXIncrementor 的值
///
/// 优先后台：找到内部 AXTextField 子元素，直接 SetAttributeValue。
/// 降级前台：若无子 TextField 或设值未同步，用 Cmd+V 粘贴。
fn set_incrementor_value(incrementor: CFTypeRef, value: &str) -> Result<()> {
    // 尝试找内部 AXTextField（后台设值）
    if let Some(text_field) = ax_action::find_element(
        incrementor,
        "AXTextField",
        &ax_action::Matcher::Any,
        3,
    ) {
        ax_action::focus_element(text_field)?;
        ax_action::set_text_field_value(text_field, value)?;
        std::thread::sleep(std::time::Duration::from_millis(100));
        let readback = ax_action::get_value_str(text_field).unwrap_or_default();
        let inc_readback = ax_action::get_value_str(incrementor).unwrap_or_default();
        if inc_readback.contains(value) || readback.contains(value) {
            debug!("AXIncrementor 后台设值成功: {}", value);
            return Ok(());
        }
        warn!("AXIncrementor 子TextField 设值后未同步 (readback={}), 降级前台粘贴", inc_readback);
    }

    ax_action::type_value_via_paste(incrementor, value)
}

/// 在表单容器中，找到标签文本后的下一个指定角色的兄弟元素
///
/// Qt 表单布局：AXStaticText(label) → AXTextField/AXIncrementor(input) 是兄弟关系。
/// 遍历 parent 的 children，找到 value 包含 label_text 的 AXStaticText，
/// 然后返回其后第一个匹配 target_role 的兄弟。
fn find_field_after_label(
    parent: CFTypeRef,
    label_text: &str,
    target_role: &str,
) -> Option<CFTypeRef> {
    use crate::futu::ax_action::{cf_array_count, cf_array_get, get_attr, as_cf_array, get_role, get_value_str};

    let children = get_attr(parent, "AXChildren").ok().and_then(|v| as_cf_array(v))?;
    let count = cf_array_count(children);

    let mut found_label = false;
    for i in 0..count {
        let child = cf_array_get(children, i);
        if !found_label {
            // 寻找标签
            if get_role(child).as_deref() == Some("AXStaticText") {
                if let Some(val) = get_value_str(child) {
                    if val.contains(label_text) {
                        found_label = true;
                        continue;
                    }
                }
            }
        } else {
            // 找到标签后，返回第一个匹配目标角色的兄弟
            if get_role(child).as_deref() == Some(target_role) {
                return Some(child);
            }
        }
    }

    // 如果顶层没找到，递归搜索子容器（AXGroup / AXSplitGroup）
    for i in 0..count {
        let child = cf_array_get(children, i);
        let role = get_role(child).unwrap_or_default();
        if role == "AXGroup" || role == "AXSplitGroup" {
            if let Some(found) = find_field_after_label(child, label_text, target_role) {
                return Some(found);
            }
        }
    }

    None
}

/// 等待确认弹窗出现（轮询所有窗口）
///
/// Qt 确认弹窗是独立 AXWindow（空 title, subrole=AXDialog），
/// 直接扫描所有窗口找含确认按钮的窗口，跳过无效的主窗口搜索。
fn wait_for_confirm_dialog(pid: i32, timeout_ms: u64) -> Option<CFTypeRef> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);
    let confirm_labels = ["确认", "确定", "下单", "委托"];

    loop {
        if let Ok(app) = ax_action::create_app_element(pid) {
            if let Ok(windows) = ax_action::get_attr(app, "AXWindows") {
                if let Some(win_arr) = ax_action::as_cf_array(windows) {
                    let count = ax_action::cf_array_count(win_arr);
                    for i in 0..count {
                        let win = ax_action::cf_array_get(win_arr, i);
                        for label in &confirm_labels {
                            if ax_action::find_element(
                                win, "AXButton", &ax_action::Matcher::TitleContains(label), 5,
                            ).is_some() {
                                return Some(win);
                            }
                        }
                    }
                }
            }
        }

        if start.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
}

/// 从弹窗中验证内容（代码 + 价格）
///
/// 通过 AX 读取弹窗中的文本元素，检查是否包含预期代码和价格。
fn verify_dialog_content(
    dialog: CFTypeRef,
    expected_code: &str,
    expected_price: f64,
) -> Result<bool> {
    // 收集弹窗中所有 AXStaticText 的文本
    let texts = ax_action::find_all_elements(
        dialog,
        "AXStaticText",
        &ax_action::Matcher::Any,
        8,
    );

    let mut all_text = String::new();
    for text_elem in &texts {
        let value = get_element_text(*text_elem);
        if let Some(v) = value {
            all_text.push_str(&v);
            all_text.push(' ');
        }
    }

    debug!("弹窗文本: {}", all_text);

    // 验证代码
    let code_found = all_text.contains(expected_code);
    if !code_found {
        warn!("弹窗中未找到代码 '{}', 文本: {}", expected_code, all_text);
        return Ok(false);
    }

    // 验证价格（允许小数格式差异）
    let price_str = format!("{:.3}", expected_price);
    let price_str_2 = format!("{:.2}", expected_price);
    let price_found = all_text.contains(&price_str) || all_text.contains(&price_str_2);
    if !price_found {
        warn!(
            "弹窗中未找到价格 '{}' 或 '{}', 文本: {}",
            price_str, price_str_2, all_text
        );
        return Ok(false);
    }

    Ok(true)
}

/// 从 AX 元素读取文本（value 或 title）
fn get_element_text(element: CFTypeRef) -> Option<String> {
    ax_action::get_value_str(element)
        .or_else(|| ax_action::get_title(element))
        .filter(|s| !s.is_empty())
}

/// 检查是否出现错误弹窗（"委托失败"/"关闭(C)" 等）
///
/// 点击"委托"后，交易系统可能返回错误弹窗（独立 AXWindow）。
/// 如果检测到，读取错误信息、关闭弹窗、返回错误文本。
fn check_error_dialog(pid: i32) -> Option<String> {
    let app = ax_action::create_app_element(pid).ok()?;
    let windows = ax_action::get_attr(app, "AXWindows").ok()?;
    let win_arr = ax_action::as_cf_array(windows)?;
    let count = ax_action::cf_array_count(win_arr);

    for i in 0..count {
        let win = ax_action::cf_array_get(win_arr, i);
        // 错误弹窗特征：有"关闭"按钮，无"委托"/"确认"按钮
        let has_close = ax_action::find_element(
            win, "AXButton", &ax_action::Matcher::TitleContains("关闭"), 5,
        );
        if has_close.is_none() {
            continue;
        }
        // 排除确认弹窗（有"委托"按钮的不是错误弹窗）
        let has_confirm = ax_action::find_element(
            win, "AXButton", &ax_action::Matcher::TitleContains("委托"), 5,
        );
        if has_confirm.is_some() {
            continue;
        }

        // 收集弹窗中所有文本
        let texts = ax_action::find_all_elements(
            win, "AXStaticText", &ax_action::Matcher::Any, 5,
        );
        let mut msg = String::new();
        for elem in &texts {
            if let Some(text) = get_element_text(*elem) {
                if !msg.is_empty() { msg.push(' '); }
                msg.push_str(&text);
            }
        }

        if !msg.is_empty() {
            // 关闭错误弹窗
            if let Some(btn) = has_close {
                let _ = ax_action::perform_action(btn, "AXPress");
            }
            warn!("交易系统返回错误: {}", msg);
            return Some(msg);
        }
    }

    None
}

/// 尝试取消弹窗（找取消/关闭按钮并点击）
fn try_cancel_dialog(dialog: CFTypeRef) {
    // 尝试找"取消"按钮
    let cancel_labels = ["取消", "关闭", "Cancel", "Close"];
    for label in &cancel_labels {
        if let Some(btn) = ax_action::find_element(
            dialog,
            "AXButton",
            &ax_action::Matcher::TitleContains(label),
            5,
        ) {
            let _ = ax_action::perform_action(btn, "AXPress");
            debug!("已点击 '{}' 关闭弹窗", label);
            return;
        }
    }
    warn!("未找到弹窗取消按钮，弹窗可能仍然打开");
}

/// 在弹窗中找到确认/委托按钮
fn find_confirm_button(dialog: CFTypeRef) -> Option<CFTypeRef> {
    let confirm_labels = ["确认", "委托", "确定", "提交", "Confirm", "Submit"];
    for label in &confirm_labels {
        if let Some(btn) = ax_action::find_element(
            dialog,
            "AXButton",
            &ax_action::Matcher::TitleContains(label),
            5,
        ) {
            return Some(btn);
        }
    }
    None
}

// ===== cft5（财富通）专有逻辑 =====

/// 查找财富通V5.0进程 PID
///
/// `pgrep -f cft5` 会匹配到 QtWebEngineProcess 等子进程，
/// 需要用 `ps -o comm=` 过滤，只取 comm 以 `/cft5` 结尾的主进程。
pub fn find_cft5_pid() -> Result<i32> {
    use std::process::Command;

    let output = Command::new("pgrep")
        .args(["-f", "cft5"])
        .output()
        .context("Failed to run pgrep")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Ok(pid) = line.trim().parse::<i32>() {
            let check = Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "comm="])
                .output();
            if let Ok(check_output) = check {
                let comm = String::from_utf8_lossy(&check_output.stdout).trim().to_string();
                // 只匹配主进程（comm 以 /cft5 结尾），排除 QtWebEngineProcess 等子进程
                if comm.ends_with("/cft5") {
                    debug!("Found cft5 PID: {} ({})", pid, comm);
                    return Ok(pid);
                }
            }
        }
    }

    anyhow::bail!("未找到财富通进程。请确认 财富通V5.0体验版 已启动。")
}

/// 获取财富通主窗口（AXStandardWindow，跳过 Dialog/浮动窗）
pub fn get_cft5_main_window(app: CFTypeRef) -> Result<CFTypeRef> {
    let windows = ax_action::get_attr(app, "AXWindows").context("Failed to get AXWindows")?;
    let win_arr = ax_action::as_cf_array(windows).context("AXWindows is not an array")?;
    let count = ax_action::cf_array_count(win_arr);
    if count == 0 {
        anyhow::bail!("No windows found");
    }

    debug!("cft5 has {} window(s)", count);

    let mut standard_win: Option<CFTypeRef> = None;
    for i in 0..count {
        let win = ax_action::cf_array_get(win_arr, i);
        if win.is_null() {
            continue;
        }
        let subrole = ax_action::get_attr(win, "AXSubrole")
            .ok()
            .and_then(|v| ax_action::cftype_to_string(v))
            .unwrap_or_default();
        if subrole == "AXStandardWindow" && standard_win.is_none() {
            standard_win = Some(win);
        }
    }

    if let Some(win) = standard_win {
        return Ok(win);
    }

    let window = ax_action::cf_array_get(win_arr, 0);
    if window.is_null() {
        anyhow::bail!("First window is null");
    }
    Ok(window)
}
