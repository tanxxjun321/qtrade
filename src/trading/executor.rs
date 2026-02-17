//! 交易执行器 — 通过 AX API 驱动财富通V5.0完成港股委托
//!
//! 大部分操作通过后台 AX API 完成（表单填写、按钮点击、弹窗验证）。
//! 导航和 AXIncrementor 输入需要短暂激活窗口（前台 CGEvent 点击/粘贴）。
//! 安全不变量：验价通过后才点击确认，任何步骤失败自动清理弹窗。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::futu::accessibility::AccessibilityReader;
use crate::futu::ax::{self, Element};

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

/// 交易市场
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradingMarket {
    /// 港股（通过港股通）
    HK,
    /// A股（沪深）
    CN,
}

impl TradingMarket {
    /// 根据股票代码推断市场
    pub fn infer(stock_code: &str) -> Option<Self> {
        let code = stock_code.trim();
        if code.len() == 5 && code.chars().all(|c| c.is_ascii_digit()) {
            Some(TradingMarket::HK)
        } else if code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) {
            let prefix = &code[..1];
            match prefix {
                "6" | "0" | "3" => Some(TradingMarket::CN),
                _ => None,
            }
        } else {
            None
        }
    }

    /// 价格小数位数
    pub fn price_decimals(self) -> usize {
        match self {
            TradingMarket::HK => 3,
            TradingMarket::CN => 2,
        }
    }

    /// 货币名称
    pub fn currency(self) -> &'static str {
        match self {
            TradingMarket::HK => "HKD",
            TradingMarket::CN => "CNY",
        }
    }
}

impl std::fmt::Display for TradingMarket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradingMarket::HK => write!(f, "港股"),
            TradingMarket::CN => write!(f, "A股"),
        }
    }
}

/// 委托请求
#[derive(Debug, Clone)]
pub struct OrderRequest {
    /// 股票代码（如 "00700"、"600519"）
    pub stock_code: String,
    /// 委托价格
    pub price: f64,
    /// 委托数量（股）
    pub quantity: u32,
    /// 买入/卖出
    pub side: OrderSide,
    /// 交易市场
    pub market: TradingMarket,
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
        let pid = find_cft5_pid().context("未找到财富通进程。请确认 财富通V5.0体验版 已启动。")?;

        // 确保窗口可见（隐藏/最小化/其他桌面/状态栏恢复）
        let _window = prepare_trading_window(pid).context("无法获取财富通窗口")?;

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
        let prec = req.market.price_decimals();
        let currency = req.market.currency();
        info!(
            "开始执行委托: {} {} {} {} 股 @ {:.prec$} {}",
            req.market,
            req.side,
            req.stock_code,
            req.quantity,
            req.price,
            currency,
            prec = prec
        );

        // 所有 AX 操作需要在 spawn_blocking 中执行（CFTypeRef 不跨线程）
        let pid = self.app_pid;
        let stock_code = req.stock_code.clone();
        let price = req.price;
        let quantity = req.quantity;
        let side = req.side;
        let market = req.market;

        let result = tokio::task::spawn_blocking(move || {
            execute_order_sync(pid, &stock_code, price, quantity, side, market)
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
    market: TradingMarket,
) -> Result<String> {
    let window = prepare_trading_window(pid).context("无法获取交易窗口")?;

    // 导航到目标面板
    navigate_to_panel(&window, side, market)?;

    let prec = market.price_decimals();
    let currency = market.currency();

    // T1: 输入证券代码
    // 布局：AXStaticText value="证券代码" → 下一个 AXTextField
    let code_field = find_field_after_label(&window, "证券代码", "AXTextField")
        .context("未找到证券代码输入框")?;
    code_field.set_focused(true).context("聚焦代码输入框失败")?;
    code_field.set_string_value(stock_code).context("输入证券代码失败")?;
    let readback = code_field.value().unwrap_or_default();
    debug!("已输入代码: {} (readback={})", stock_code, readback);

    // 等待代码解析（服务器查询股票信息）
    std::thread::sleep(std::time::Duration::from_millis(800));

    // T2: 输入价格
    // 布局：AXStaticText value="买入价格"/"卖出价格" → 下一个 AXIncrementor
    let price_label = match side {
        OrderSide::Buy => "买入价格",
        OrderSide::Sell => "卖出价格",
    };
    let price_str = format!("{:.prec$}", price, prec = prec);
    let price_inc = find_field_after_label(&window, price_label, "AXIncrementor")
        .context(format!("未找到 '{}' 输入框", price_label))?;
    set_incrementor_value(&price_inc, &price_str).context("输入价格失败")?;
    let price_readback = price_inc.value().unwrap_or_default();
    debug!("已输入价格: {} (readback={})", price_str, price_readback);

    // T3: 输入数量
    // 布局：AXStaticText value="买入数量"/"卖出数量" → 下一个 AXIncrementor
    let qty_label = match side {
        OrderSide::Buy => "买入数量",
        OrderSide::Sell => "卖出数量",
    };
    let qty_str = quantity.to_string();
    let qty_field = find_field_after_label(&window, qty_label, "AXIncrementor")
        .context(format!("未找到 '{}' 输入框", qty_label))?;
    set_incrementor_value(&qty_field, &qty_str).context("输入数量失败")?;
    let qty_readback = qty_field.value().unwrap_or_default();
    debug!("已输入数量: {} (readback={})", qty_str, qty_readback);

    // T4: 点击买入/卖出按钮
    // AXButton title="买入(B)" / "卖出(S)"
    let submit_label = match side {
        OrderSide::Buy => "买入",
        OrderSide::Sell => "卖出",
    };
    let submit_btn = window.find(|e| {
        e.role().as_deref() == Some("AXButton") &&
        e.title().map_or(false, |t| t.contains(submit_label))
    }, 10)
    .context(format!("未找到 '{}' 按钮", submit_label))?;

    let btn_title = submit_btn.title().unwrap_or_default();
    debug!("找到提交按钮: \"{}\"", btn_title);
    // 先尝试 AXPress，失败则用前台坐标点击
    if submit_btn.click().is_err() {
        debug!("AXPress 失败，使用前台坐标点击");
        let _ = window.perform_action("AXRaise");
        std::thread::sleep(std::time::Duration::from_millis(100));
        ax::action::click_at_element(&submit_btn)
            .context(format!("点击 '{}' 按钮失败", submit_label))?;
    }

    // T5: 等待确认弹窗（直接轮询所有窗口，弹窗是独立 AXWindow）
    let confirm_dialog = wait_for_confirm_dialog(pid, 3000).context("等待确认弹窗超时 (3s)")?;
    debug!("确认弹窗已出现");

    // T6: AX 验价 — 从弹窗 AXStaticText 中提取并比对代码和价格
    let verified = verify_dialog_content(&confirm_dialog, stock_code, price, market)?;
    if !verified {
        // 价格不匹配，取消
        try_cancel_dialog(&confirm_dialog);
        anyhow::bail!("AX 验价失败：弹窗内容与预期不符");
    }
    debug!("验价通过");

    // T7: 点击确认按钮
    let confirm_btn = find_confirm_button(&confirm_dialog).context("未找到确认按钮")?;
    confirm_btn.click().context("点击确认按钮失败")?;

    // T8: 检查是否出现错误弹窗（如 "委托失败"/"系统正在初始化" 等）
    std::thread::sleep(std::time::Duration::from_millis(800));
    if let Some(error_msg) = check_error_dialog(pid) {
        anyhow::bail!("委托被拒绝: {}", error_msg);
    }

    info!(
        "委托已提交: {} {} {} 股 @ {} {}",
        side, stock_code, quantity, price_str, currency
    );

    Ok(format!(
        "{} {} {} 股 @ {} {} 委托已提交",
        side, stock_code, quantity, price_str, currency
    ))
}

/// 确保交易窗口可见并返回 window 引用
///
/// 按优先级处理以下场景：
/// 1. App 被 Cmd+H 隐藏 → AXHidden = false
/// 2. 窗口在其他桌面（Space）被挤占 → SetFrontProcess 激活到当前桌面
/// 3. 主窗口已关闭（仅剩状态栏图标）→ 点击状态栏"显示主窗口"恢复
/// 4. 窗口被最小化 → AXMinimized = false
fn prepare_trading_window(pid: i32) -> Result<Element> {
    let app = ax::Application::new(pid)
        .map_err(|e| anyhow::anyhow!("Failed to create app element: {:?}", e))?;

    // Step 1: 取消 Cmd+H 隐藏
    if app.is_hidden() {
        debug!("App 已隐藏，取消隐藏");
        app.set_hidden(false).context("取消 App 隐藏失败")?;
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    // Step 2: 激活应用到前台（从其他桌面带到当前桌面）
    activate_app(pid);

    // Step 3: 获取主窗口，失败则通过状态栏图标恢复
    let window = match get_cft5_main_window(&app) {
        Ok(w) => w,
        Err(e) => {
            debug!("获取窗口失败: {}，尝试状态栏'显示主窗口'", e);
            show_via_tray_icon(&app).context("通过状态栏'显示主窗口'恢复失败")?;
            std::thread::sleep(std::time::Duration::from_millis(500));
            get_cft5_main_window(&app)
                .context("状态栏恢复后仍无法获取主窗口。请手动打开财富通窗口。")?
        }
    };

    // Step 4: 取消最小化
    if window.is_minimized() {
        debug!("窗口已最小化，恢复显示");
        window.set_minimized(false).context("取消窗口最小化失败")?;
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    // Step 5: 激活窗口
    window.perform_action("AXRaise").context("激活窗口失败")?;

    Ok(window)
}

/// 激活应用到前台（处理跨桌面/Space 场景）
fn activate_app(pid: i32) {
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct ProcessSerialNumber {
        high: u32,
        low: u32,
    }

    extern "C" {
        fn GetProcessForPID(pid: i32, psn: *mut ProcessSerialNumber) -> i32;
        fn SetFrontProcess(psn: *const ProcessSerialNumber) -> i32;
    }

    unsafe {
        let mut psn = ProcessSerialNumber { high: 0, low: 0 };
        if GetProcessForPID(pid, &mut psn) == 0 {
            let status = SetFrontProcess(&psn);
            if status != 0 {
                debug!("SetFrontProcess failed: {}", status);
            }
        } else {
            debug!("GetProcessForPID({}) failed", pid);
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
}

/// 通过状态栏图标菜单恢复主窗口
///
/// AX 路径: AXApplication → AXExtrasMenuBar → AXMenuBarItem → 坐标点击
///   → AXMenu → AXMenuItem(显示主窗口) → AXPress
///
/// 注意: AXMenuBarItem 不支持 AXPress (-25206)，必须用前台坐标点击。
/// AXMenuItem 打开后支持 AXPress。
fn show_via_tray_icon(app: &ax::Application) -> Result<()> {
    // 获取状态栏菜单
    let extras_bar = app.element().attribute("AXExtrasMenuBar")
        .map_err(|e| anyhow::anyhow!("未找到状态栏图标（AXExtrasMenuBar）: {:?}", e))?;

    let extras_bar_elem = match extras_bar {
        ax::CfType::Element(e) => ax::Element::from_wrapper(e),
        _ => anyhow::bail!("AXExtrasMenuBar 不是元素类型"),
    };

    let children = match extras_bar_elem.children() {
        Ok(children) => children,
        _ => anyhow::bail!("状态栏无菜单项"),
    };

    if children.is_empty() {
        anyhow::bail!("状态栏无菜单项");
    }

    // 获取第一个菜单项
    let tray_item = children.into_iter().next().context("获取状态栏菜单项失败")?;

    // 前台坐标点击状态栏图标（AXPress 不支持 -25206）
    ax::action::click_at_element(&tray_item).context("点击状态栏图标失败")?;
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 搜索"显示主窗口"菜单项
    let menu_item = tray_item.find(|e| {
        e.role().as_deref() == Some("AXMenuItem") &&
        e.title().map_or(false, |t| t.contains("主窗口"))
    }, 5)
    .context("未找到'显示主窗口'菜单项")?;

    // AXMenuItem 支持 AXPress，失败则降级坐标点击
    if menu_item.click().is_err() {
        debug!("AXPress 菜单项失败，降级坐标点击");
        ax::action::click_at_element(&menu_item).context("点击'显示主窗口'失败")?;
    }
    debug!("已通过状态栏图标显示主窗口");

    std::thread::sleep(std::time::Duration::from_millis(300));
    Ok(())
}

/// 导航到交易面板
///
/// HK: 港股通 tab → "港股买入"/"港股卖出" (AXStaticText, 前台 HID 点击)
/// CN: 股票 tab → "证券买入"/"证券卖出" (AXStaticText, 前台 HID 点击)
fn navigate_to_panel(window: &Element, side: OrderSide, market: TradingMarket) -> Result<()> {
    let (tab_title, target_label, target_role) = match market {
        TradingMarket::HK => {
            let label = match side {
                OrderSide::Buy => "港股买入",
                OrderSide::Sell => "港股卖出",
            };
            ("港股通", label, "AXStaticText")
        }
        TradingMarket::CN => {
            let label = match side {
                OrderSide::Buy => "证券买入",
                OrderSide::Sell => "证券卖出",
            };
            ("股票", label, "AXStaticText")
        }
    };

    // 先尝试直接找到目标元素
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Some(elem) = window.find(|e| {
            e.role().as_deref() == Some(target_role) &&
            e.title().as_deref() == Some(target_label)
        }, 12) {
            click_panel_element(window, &elem, target_label)?;
            std::thread::sleep(std::time::Duration::from_millis(1500));
            return Ok(());
        }

        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // 切换到目标 tab
    if let Some(tab) = window.find(|e| {
        e.role().as_deref() == Some("AXRadioButton") &&
        e.title().as_deref() == Some(tab_title)
    }, 12) {
        let selected = tab.value().map_or(false, |v| v == "1" || v == "true");
        if !selected {
            debug!("切换到 '{}' tab", tab_title);
            tab.click()
                .context(format!("点击 '{}' tab 失败", tab_title))?;
            std::thread::sleep(std::time::Duration::from_millis(800));

            if let Some(elem) = window.find(|e| {
                e.role().as_deref() == Some(target_role) &&
                e.title().as_deref() == Some(target_label)
            }, 12) {
                click_panel_element(window, &elem, target_label)?;
                std::thread::sleep(std::time::Duration::from_millis(1500));
                return Ok(());
            }
        }
    }

    anyhow::bail!(
        "未找到 '{}' 导航元素。请确认财富通已打开 '{}' 面板。",
        target_label,
        tab_title
    );
}

/// 点击面板导航元素（AXStaticText, 前台 HID 坐标点击）
fn click_panel_element(window: &Element, elem: &Element, label: &str) -> Result<()> {
    let _ = window.perform_action("AXRaise");
    std::thread::sleep(std::time::Duration::from_millis(200));
    ax::action::click_at_element(elem).context(format!("点击 '{}' 失败", label))
}

/// 设置 AXIncrementor 的值
///
/// 优先后台：找到内部 AXTextField 子元素，直接 SetAttributeValue。
/// 降级前台：若无子 TextField 或设值未同步，用 Cmd+V 粘贴。
fn set_incrementor_value(incrementor: &Element, value: &str) -> Result<()> {
    // 使用安全 API 的辅助函数
    ax::action::set_incrementor_value(incrementor, value)
        .map_err(|e| anyhow::anyhow!("设置 incrementor 值失败: {:?}", e))
}

/// 在表单容器中，找到标签文本后的下一个指定角色的兄弟元素
///
/// Qt 表单布局：AXStaticText(label) → AXTextField/AXIncrementor(input) 是兄弟关系。
/// 遍历 parent 的 children，找到 value 包含 label_text 的 AXStaticText，
/// 然后返回其后第一个匹配 target_role 的兄弟。
fn find_field_after_label(
    parent: &Element,
    label_text: &str,
    target_role: &str,
) -> Option<Element> {
    let children = parent.children().ok()?;

    let mut found_label = false;
    for child in &children {
        if !found_label {
            // 寻找标签
            if child.role().as_deref() == Some("AXStaticText") {
                if let Some(val) = child.value() {
                    if val.contains(label_text) {
                        found_label = true;
                        continue;
                    }
                }
            }
        } else {
            // 找到标签后，返回第一个匹配目标角色的兄弟
            if child.role().as_deref() == Some(target_role) {
                return Some(child.clone());
            }
        }
    }

    // 如果顶层没找到，递归搜索子容器（AXGroup / AXSplitGroup）
    for child in &children {
        let role = child.role().unwrap_or_default();
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
fn wait_for_confirm_dialog(pid: i32, timeout_ms: u64) -> Option<Element> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);
    let confirm_labels = ["确认", "确定", "下单", "委托"];

    loop {
        if let Ok(app) = ax::Application::new(pid) {
            if let Ok(windows) = app.windows() {
                for win in windows {
                    for label in &confirm_labels {
                        if win.find(|e| {
                            e.role().as_deref() == Some("AXButton") &&
                            e.title().map_or(false, |t| t.contains(label))
                        }, 5).is_some() {
                            return Some(win);
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
    dialog: &Element,
    expected_code: &str,
    expected_price: f64,
    market: TradingMarket,
) -> Result<bool> {
    // 收集弹窗中所有 AXStaticText 的文本
    let texts = dialog.find_all(|e| e.role().as_deref() == Some("AXStaticText"), 8);

    let mut all_text = String::new();
    for text_elem in &texts {
        let value = ax::action::get_element_text(text_elem);
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

    // 验证价格（允许小数格式差异：检查 market 精度 + 相邻精度）
    let prec = market.price_decimals();
    let price_str = format!("{:.prec$}", expected_price, prec = prec);
    let price_str_alt = if prec >= 1 {
        format!("{:.prec$}", expected_price, prec = prec - 1)
    } else {
        format!("{:.1}", expected_price)
    };
    let price_str_extra = format!("{:.prec$}", expected_price, prec = prec + 1);
    let price_found = all_text.contains(&price_str)
        || all_text.contains(&price_str_alt)
        || all_text.contains(&price_str_extra);
    if !price_found {
        warn!(
            "弹窗中未找到价格 '{}' 或 '{}', 文本: {}",
            price_str, price_str_alt, all_text
        );
        return Ok(false);
    }

    Ok(true)
}

/// 检查是否出现错误弹窗（"委托失败"/"关闭(C)" 等）
///
/// 点击"委托"后，交易系统可能返回错误弹窗（独立 AXWindow）。
/// 如果检测到，读取错误信息、关闭弹窗、返回错误文本。
fn check_error_dialog(pid: i32) -> Option<String> {
    let app = ax::Application::new(pid).ok()?;
    let windows = app.windows().ok()?;

    for win in windows {
        // 错误弹窗特征：有"关闭"按钮，无"委托"/"确认"按钮
        let has_close = win.find(|e| {
            e.role().as_deref() == Some("AXButton") &&
            e.title().map_or(false, |t| t.contains("关闭"))
        }, 5);
        if has_close.is_none() {
            continue;
        }
        // 排除确认弹窗（有"委托"按钮的不是错误弹窗）
        let has_confirm = win.find(|e| {
            e.role().as_deref() == Some("AXButton") &&
            e.title().map_or(false, |t| t.contains("委托"))
        }, 5);
        if has_confirm.is_some() {
            continue;
        }

        // 收集弹窗中所有文本
        let texts = win.find_all(|e| e.role().as_deref() == Some("AXStaticText"), 5);
        let mut msg = String::new();
        for elem in &texts {
            if let Some(text) = ax::action::get_element_text(elem) {
                if !msg.is_empty() {
                    msg.push(' ');
                }
                msg.push_str(&text);
            }
        }

        if !msg.is_empty() {
            // 关闭错误弹窗
            if let Some(ref btn) = has_close {
                let _ = btn.click();
            }
            warn!("交易系统返回错误: {}", msg);
            return Some(msg);
        }
    }

    None
}

/// 尝试取消弹窗（找取消/关闭按钮并点击）
fn try_cancel_dialog(dialog: &Element) {
    // 尝试找"取消"按钮
    let cancel_labels = ["取消", "关闭", "Cancel", "Close"];
    for label in &cancel_labels {
        if let Some(btn) = dialog.find(|e| {
            e.role().as_deref() == Some("AXButton") &&
            e.title().map_or(false, |t| t.contains(label))
        }, 5) {
            let _ = btn.click();
            debug!("已点击 '{}' 关闭弹窗", label);
            return;
        }
    }
    warn!("未找到弹窗取消按钮，弹窗可能仍然打开");
}

/// 在弹窗中找到确认/委托按钮
fn find_confirm_button(dialog: &Element) -> Option<Element> {
    let confirm_labels = ["确认", "委托", "确定", "提交", "Confirm", "Submit"];
    for label in &confirm_labels {
        if let Some(btn) = dialog.find(|e| {
            e.role().as_deref() == Some("AXButton") &&
            e.title().map_or(false, |t| t.contains(label))
        }, 5) {
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
                let comm = String::from_utf8_lossy(&check_output.stdout)
                    .trim()
                    .to_string();
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
pub fn get_cft5_main_window(app: &ax::Application) -> Result<Element> {
    let windows = app.windows().context("Failed to get windows")?;
    if windows.is_empty() {
        anyhow::bail!("No windows found");
    }

    debug!("cft5 has {} window(s)", windows.len());

    // 找第一个标准窗口
    for win in &windows {
        if let Some(subrole) = win.subrole() {
            if subrole == "AXStandardWindow" {
                return Ok(win.clone());
            }
        }
    }

    // 没找到标准窗口，返回第一个窗口
    windows.into_iter().next().context("No windows found")
}
