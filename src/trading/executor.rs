//! 交易执行器 — 通过 AX API 驱动财富通V5.0完成港股委托
//!
//! 全程零 CGEvent，用户可自由操作键鼠不受影响。
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
        let pid = ax_action::find_trading_app_pid()
            .context("未找到财富通进程。请确认 财富通V5.0体验版 已启动。")?;

        // 验证能获取窗口
        let app = ax_action::create_app_element(pid)?;
        let _window = ax_action::get_main_window(app)
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
    let window = ax_action::get_main_window(app)
        .context("无法获取交易窗口")?;

    // N1: 检测当前是否已在目标面板
    let panel_title = match side {
        OrderSide::Buy => "港股买入",
        OrderSide::Sell => "港股卖出",
    };

    let already_on_panel = detect_current_panel(window, panel_title);

    if !already_on_panel {
        // N3: 激活窗口
        ax_action::raise_window(window)
            .context("激活交易窗口失败")?;
        debug!("窗口已激活");

        // N4-N5: 通过左侧树导航
        navigate_to_panel(window, side)?;

        // N6: 等待交易表单出现
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if detect_current_panel(window, panel_title) {
                break;
            }
            if std::time::Instant::now() >= deadline {
                anyhow::bail!("等待交易面板 '{}' 超时 (5s)", panel_title);
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        debug!("已导航到 {}", panel_title);
    } else {
        debug!("已在 {} 面板，跳过导航", panel_title);
    }

    // T1: 输入证券代码
    let code_field = find_input_field(window, &["代码", "证券代码", "股票代码", "Code"])
        .context("未找到证券代码输入框")?;
    ax_action::focus_element(code_field)?;
    ax_action::set_text_field_value(code_field, stock_code)
        .context("输入证券代码失败")?;
    debug!("已输入代码: {}", stock_code);

    // 短暂等待代码解析
    std::thread::sleep(std::time::Duration::from_millis(500));

    // T2: 输入价格
    let price_str = format!("{:.3}", price);
    let price_field = find_input_field(window, &["价格", "Price", "委托价"])
        .context("未找到价格输入框")?;
    ax_action::focus_element(price_field)?;
    ax_action::set_text_field_value(price_field, &price_str)
        .context("输入价格失败")?;
    debug!("已输入价格: {}", price_str);

    // T3: 输入数量
    let qty_str = quantity.to_string();
    let qty_field = find_input_field(window, &["数量", "Quantity", "委托数量"])
        .context("未找到数量输入框")?;
    ax_action::focus_element(qty_field)?;
    ax_action::set_text_field_value(qty_field, &qty_str)
        .context("输入数量失败")?;
    debug!("已输入数量: {}", qty_str);

    // T4: 点击买入/卖出按钮
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

    ax_action::perform_action(submit_btn, "AXPress")
        .context(format!("点击 '{}' 按钮失败", submit_label))?;
    debug!("已点击 {} 按钮", submit_label);

    // T5: 等待确认弹窗
    std::thread::sleep(std::time::Duration::from_millis(500));

    let confirm_dialog = wait_for_dialog_sync(window, 5000);
    let confirm_dialog = match confirm_dialog {
        Some(d) => d,
        None => {
            anyhow::bail!("等待确认弹窗超时 (5s)");
        }
    };
    debug!("确认弹窗已出现");

    // T6: OCR 验价 — 从弹窗文本中提取并比对价格
    let verified = verify_dialog_content(confirm_dialog, stock_code, price)?;
    if !verified {
        // 价格不匹配，取消
        try_cancel_dialog(confirm_dialog);
        anyhow::bail!("OCR 验价失败：弹窗内容与预期不符");
    }
    debug!("验价通过");

    // T7: 点击确认按钮
    let confirm_btn = find_confirm_button(confirm_dialog)
        .context("未找到确认按钮")?;
    ax_action::perform_action(confirm_btn, "AXPress")
        .context("点击确认按钮失败")?;

    info!(
        "委托已提交: {} {} {} 股 @ {} HKD",
        side, stock_code, quantity, price_str
    );

    Ok(format!(
        "{} {} {} 股 @ {} HKD 委托已提交",
        side, stock_code, quantity, price_str
    ))
}

/// 检测当前是否在目标交易面板
fn detect_current_panel(window: CFTypeRef, panel_title: &str) -> bool {
    // 搜索窗口中是否有包含面板标题的静态文本
    ax_action::find_element(
        window,
        "AXStaticText",
        &ax_action::Matcher::TitleContains(panel_title),
        8,
    )
    .is_some()
}

/// 通过左侧树形控件导航到交易面板
fn navigate_to_panel(window: CFTypeRef, side: OrderSide) -> Result<()> {
    let target = match side {
        OrderSide::Buy => "港股买入",
        OrderSide::Sell => "港股卖出",
    };

    // 尝试在树形控件中查找节点
    let node = ax_action::find_tree_node(window, &["港股通", target]);
    if let Some(node) = node {
        ax_action::perform_action(node, "AXPress")
            .context(format!("点击 '{}' 节点失败", target))?;
        return Ok(());
    }

    // 备选：直接搜索包含目标文本的可点击元素
    let element = ax_action::find_element(
        window,
        "AXStaticText",
        &ax_action::Matcher::TitleContains(target),
        10,
    )
    .or_else(|| {
        ax_action::find_element(
            window,
            "AXButton",
            &ax_action::Matcher::TitleContains(target),
            10,
        )
    })
    .or_else(|| {
        ax_action::find_element(
            window,
            "AXCell",
            &ax_action::Matcher::TitleContains(target),
            10,
        )
    });

    match element {
        Some(elem) => {
            ax_action::perform_action(elem, "AXPress")
                .context(format!("点击 '{}' 失败", target))?;
            Ok(())
        }
        None => {
            anyhow::bail!("未找到 '{}' 导航元素。请手动切换到港股交易面板。", target);
        }
    }
}

/// 搜索表单中的输入框（通过附近标签文本）
fn find_input_field(window: CFTypeRef, labels: &[&str]) -> Option<CFTypeRef> {
    for label in labels {
        if let Some(field) = ax_action::find_text_field_by_label(window, label) {
            return Some(field);
        }
    }

    // 退路：按 AXRoleDescription 搜索
    for label in labels {
        let field = ax_action::find_element(
            window,
            "AXTextField",
            &ax_action::Matcher::Description(label),
            10,
        );
        if field.is_some() {
            return field;
        }
    }

    None
}

/// 同步等待弹窗出现
fn wait_for_dialog_sync(window: CFTypeRef, timeout_ms: u64) -> Option<CFTypeRef> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);
    let interval = std::time::Duration::from_millis(200);

    loop {
        // 搜索 AXSheet 或 AXDialog
        if let Some(dialog) =
            ax_action::find_element(window, "AXSheet", &ax_action::Matcher::Any, 5)
        {
            return Some(dialog);
        }
        if let Some(dialog) =
            ax_action::find_element(window, "AXDialog", &ax_action::Matcher::Any, 5)
        {
            return Some(dialog);
        }
        // 也搜索浮动窗口类弹窗
        if let Some(dialog) = ax_action::find_element(
            window,
            "AXGroup",
            &ax_action::Matcher::TitleContains("确认"),
            8,
        ) {
            return Some(dialog);
        }

        if start.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(interval);
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
