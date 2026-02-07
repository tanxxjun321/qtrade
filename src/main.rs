mod alerts;
mod analysis;
mod config;
mod data;
mod futu;
mod models;
mod trading;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use crate::alerts::manager::AlertManager;
use crate::alerts::notify::Notifier;
use crate::alerts::rules::{ChangeThresholdRule, SignalRule, VolumeSpikeRule};
use crate::analysis::daily::DailyAnalysisEngine;
use crate::analysis::engine::AnalysisEngine;
use crate::config::AppConfig;
use crate::data::provider::DataProviderKind;
use crate::models::{QuoteSnapshot, StockCode};
use crate::ui::dashboard::DashboardState;

#[derive(Parser)]
#[command(name = "qtrade", about = "量化交易盯盘系统")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// 配置文件路径
    #[arg(short, long)]
    config: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// 启动盯盘系统
    Start,
    /// 显示自选股列表
    Watchlist,
    /// 检查 Accessibility 权限并打印 App 元素树（调试用）
    Debug,
    /// 测试 FutuOpenD 连接并获取行情
    TestApi,
    /// 测试窗口截图 + Vision OCR 识别效果
    TestOcr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // 加载配置
    let config = match &cli.config {
        Some(path) => AppConfig::load(std::path::Path::new(path))?,
        None => AppConfig::load_or_default(),
    };

    // 初始化日志
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| config.general.log_level.parse().unwrap_or_default());

    let is_tui = matches!(cli.command, Commands::Start);
    if is_tui {
        // TUI 模式：日志写文件，避免干扰终端界面
        let log_file = std::fs::File::create("qtrade.log")
            .expect("Failed to create qtrade.log");
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(log_file)
            .with_ansi(false)
            .init();
    } else {
        // CLI 模式：日志输出到终端
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .init();
    }

    match cli.command {
        Commands::Start => cmd_start(config).await,
        Commands::Watchlist => cmd_watchlist(config),
        Commands::Debug => cmd_debug(config),
        Commands::TestApi => cmd_test_api(config).await,
        Commands::TestOcr => cmd_test_ocr(config).await,
    }
}

/// 显示自选股列表
fn cmd_watchlist(config: AppConfig) -> Result<()> {
    let entries = futu::watchlist::load_watchlist(
        config.futu.data_path.as_deref(),
        config.futu.user_id.as_deref(),
    )?;

    println!("自选股列表 ({} 只):", entries.len());
    println!("{:-<70}", "");
    println!(
        "{:<4} {:<14} {:<12} {}",
        "#", "代码", "名称", "缓存价格"
    );
    println!("{:-<70}", "");

    for (i, entry) in entries.iter().enumerate() {
        let price_str = match entry.cached_price {
            Some(p) => format!("{:.2}", p),
            None => "-".to_string(),
        };
        println!(
            "{:<4} {:<14} {:<12} {}",
            i + 1,
            entry.code.display_code(),
            entry.name,
            price_str
        );
    }

    Ok(())
}

/// 调试：检查 AX 权限并打印元素树
fn cmd_debug(_config: AppConfig) -> Result<()> {
    use crate::futu::accessibility::AccessibilityReader;

    println!("检查辅助功能权限...");
    if AccessibilityReader::check_permission() {
        println!("✓ 辅助功能权限已授权");
    } else {
        println!("✗ 辅助功能权限未授权，正在请求...");
        AccessibilityReader::request_permission();
        println!("请在 系统偏好设置 → 隐私与安全性 → 辅助功能 中授权 qtrade");
        return Ok(());
    }

    println!("\n查找富途牛牛进程...");
    let mut reader = AccessibilityReader::new();
    reader.connect()?;

    println!("\n读取 App 元素树...");
    let tree = reader.dump_element_tree()?;
    println!("{}", tree);

    Ok(())
}

/// 启动盯盘系统
async fn cmd_start(config: AppConfig) -> Result<()> {
    info!("qtrade 量化盯盘系统启动");

    // 读取自选股
    let watchlist = futu::watchlist::load_watchlist(
        config.futu.data_path.as_deref(),
        config.futu.user_id.as_deref(),
    )?;

    if watchlist.is_empty() {
        anyhow::bail!("自选股列表为空");
    }

    info!("已加载 {} 只自选股", watchlist.len());

    // 过滤掉 800xxx 内部索引代码和 Unknown 市场（CNmain 等非股票条目）
    let stock_codes: Vec<StockCode> = watchlist
        .iter()
        .filter(|e| {
            !e.code.code.starts_with("800")
                && e.code.market != crate::models::Market::Unknown
        })
        .map(|e| e.code.clone())
        .collect();

    info!("可订阅股票: {} 只（已过滤内部索引代码）", stock_codes.len());

    // 创建数据提供者
    let mut provider = DataProviderKind::from_config(&config);

    // 尝试连接
    match provider.connect().await {
        Ok(()) => {
            info!("数据源 [{}] 连接成功", provider.name());
            // 订阅行情
            match provider.subscribe(&stock_codes).await {
                Ok(()) => info!("已订阅 {} 只股票的实时行情", stock_codes.len()),
                Err(e) => warn!("订阅行情失败: {}", e),
            }
        }
        Err(e) => {
            warn!("数据源连接失败: {}，将使用缓存数据", e);
        }
    }

    // 创建日线分析引擎，加载缓存
    let daily_engine = Arc::new(Mutex::new(DailyAnalysisEngine::new()));
    let cache_last_updated = {
        let mut de = daily_engine.lock().await;
        let last = de.load_cache();
        if de.stock_count() > 0 {
            info!("Loaded daily kline cache: {} stocks", de.stock_count());
        }
        last
    };

    // 创建分析引擎
    let engine = Arc::new(Mutex::new(AnalysisEngine::new(200)));

    // 创建提醒管理器
    let notifier = Notifier::new(config.alerts.webhook_url.clone());
    let mut alert_manager = AlertManager::new(config.alerts.cooldown_secs, notifier);
    if config.alerts.enabled {
        alert_manager.add_rule(Box::new(ChangeThresholdRule::new(
            config.alerts.change_threshold_pct,
        )));
        alert_manager.add_rule(Box::new(SignalRule));
        alert_manager.add_rule(Box::new(VolumeSpikeRule {
            ratio_threshold: 2.0,
        }));
    }
    let alert_manager = Arc::new(Mutex::new(alert_manager));

    // 数据通道
    let (quote_tx, mut quote_rx) = mpsc::channel::<Vec<QuoteSnapshot>>(32);

    // 仪表盘状态
    let dash_state = Arc::new(Mutex::new(DashboardState::new()));
    {
        let mut state = dash_state.lock().await;
        state.source_name = provider.name().to_string();
        state.source_connected = provider.is_connected();

        // 初始数据：用缓存价格填充
        let initial_quotes: Vec<QuoteSnapshot> = watchlist
            .iter()
            .map(|e| {
                let mut q = QuoteSnapshot::empty(e.code.clone(), e.name.clone());
                if let Some(price) = e.cached_price {
                    q.last_price = price;
                }
                q
            })
            .collect();
        state.update_quotes(initial_quotes);

        // 如果有缓存，立即填充日线数据
        {
            let de = daily_engine.lock().await;
            if de.stock_count() > 0 {
                state.daily_indicators = de.get_indicators().clone();
                state.daily_signals = de.get_signals().clone();
                let sig_count: usize =
                    state.daily_signals.values().map(|v| v.len()).sum();
                state.daily_kline_status =
                    format!("日K:{}只 信号:{} (缓存)", de.stock_count(), sig_count);
            }
        }
    }

    // 数据采集任务
    let refresh_interval = Duration::from_secs(config.data_source.refresh_interval_secs);
    let codes_for_fetch = stock_codes.clone();
    let dash_for_fetch = dash_state.clone();
    let fetch_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(refresh_interval);
        loop {
            interval.tick().await;
            match provider.get_quotes(&codes_for_fetch).await {
                Ok(quotes) => {
                    // 清除错误
                    {
                        let mut state = dash_for_fetch.lock().await;
                        state.last_error = None;
                    }
                    if !quotes.is_empty() {
                        if quote_tx.send(quotes).await.is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let msg = format!("{}", e);
                    warn!("数据获取失败: {}", msg);
                    let mut state = dash_for_fetch.lock().await;
                    state.last_error = Some(msg);
                }
            }
        }
    });

    // 日K线异步获取任务（启动后立即获取，之后定时刷新）
    let daily_refresh_handle = if config.analysis.daily_kline_enabled {
        let daily_engine_clone = daily_engine.clone();
        let dash_for_daily = dash_state.clone();
        let futu_host = config.futu.opend_host.clone();
        let futu_port = config.futu.opend_port;
        let daily_days = config.analysis.daily_kline_days;
        let refresh_mins = config.analysis.daily_kline_refresh_minutes;
        let cached_date = cache_last_updated.clone();

        // 日K线通过独立连接拉取，不依赖实时行情的订阅状态
        let daily_codes = stock_codes.clone();
        info!("Daily K-line target: {} stocks", daily_codes.len());

        Some(tokio::spawn(async move {
            let mut first = true;
            loop {
                if !first {
                    if refresh_mins == 0 {
                        return; // 仅启动时获取一次
                    }
                    tokio::time::sleep(Duration::from_secs(refresh_mins * 60)).await;
                }

                // 判断全量还是增量
                let need_full = if first {
                    match &cached_date {
                        Some(d) => {
                            crate::analysis::daily::cache_needs_full_refresh(d, 3)
                        }
                        None => true,
                    }
                } else {
                    false // 运行中刷新始终增量
                };
                first = false;

                let fetch_days: u32 = if need_full { daily_days } else { 5 };
                let mode = if need_full { "全量" } else { "增量" };
                let total = daily_codes.len();

                info!("Fetching daily K-line data ({}，{}天，{}只)...", mode, fetch_days, total);
                {
                    let mut state = dash_for_daily.lock().await;
                    state.daily_kline_status =
                        format!("日K{}中(0/{})", mode, total);
                }

                // 创建独立连接
                let mut client =
                    crate::futu::openapi::OpenApiClient::new(&futu_host, futu_port);
                match client.connect().await {
                    Ok(()) => {
                        let end = chrono::Local::now().format("%Y-%m-%d").to_string();
                        let begin = (chrono::Local::now()
                            - chrono::Duration::days(fetch_days as i64 * 2))
                            .format("%Y-%m-%d")
                            .to_string();

                        // 逐只拉取，即时合并
                        // 按市场检测权限：某市场首次返回权限错误后跳过该市场剩余股票
                        let mut no_permission_markets: std::collections::HashSet<crate::models::Market> =
                            std::collections::HashSet::new();
                        let mut fetched = 0u32;
                        // 增量模式下，缓存不足的股票单独全量拉取
                        const MIN_CACHED_DAYS: usize = 60;
                        for (i, stock) in daily_codes.iter().enumerate() {
                            if no_permission_markets.contains(&stock.market) {
                                continue;
                            }

                            let stock_fetch_days = if !need_full {
                                let cached = {
                                    let de = daily_engine_clone.lock().await;
                                    de.cached_days(stock)
                                };
                                if cached < MIN_CACHED_DAYS { daily_days } else { fetch_days }
                            } else {
                                fetch_days
                            };

                            match client
                                .request_history_kline(stock, &begin, &end, stock_fetch_days)
                                .await
                            {
                                Ok(klines) => {
                                    if !klines.is_empty() {
                                        let mut data = std::collections::HashMap::new();
                                        data.insert(stock.clone(), klines);
                                        let mut de = daily_engine_clone.lock().await;
                                        de.merge_update(data);
                                        fetched += 1;
                                    }
                                }
                                Err(e) => {
                                    let msg = format!("{}", e);
                                    let is_permission = msg.contains("permission")
                                        || msg.contains("未开通")
                                        || msg.contains("no quota")
                                        || msg.contains("not available");
                                    if is_permission {
                                        warn!(
                                            "{} market no permission, skipping: {}",
                                            stock.market, msg
                                        );
                                        no_permission_markets.insert(stock.market);
                                    } else {
                                        warn!(
                                            "Failed to get klines for {}: {}",
                                            stock.display_code(), msg
                                        );
                                    }
                                }
                            }

                            // 更新进度
                            {
                                let mut state = dash_for_daily.lock().await;
                                state.daily_kline_status =
                                    format!("日K{}中({}/{})", mode, i + 1, total);
                            }

                            // 每 10 只存盘一次 + 同步 dashboard
                            if (i + 1) % 10 == 0 || i + 1 == total {
                                let de = daily_engine_clone.lock().await;
                                de.save_cache();
                                let mut state = dash_for_daily.lock().await;
                                state.daily_indicators = de.get_indicators().clone();
                                state.daily_signals = de.get_signals().clone();
                            }

                            // 间隔 200ms 防限流
                            if i + 1 < total {
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        }

                        // 最终状态
                        {
                            let de = daily_engine_clone.lock().await;
                            let mut state = dash_for_daily.lock().await;
                            state.daily_indicators = de.get_indicators().clone();
                            state.daily_signals = de.get_signals().clone();
                            let sig_count: usize =
                                state.daily_signals.values().map(|v| v.len()).sum();
                            state.daily_kline_status =
                                format!("日K:{}只 信号:{}", de.stock_count(), sig_count);
                            info!(
                                "Daily K-line {} complete: fetched {}, total {}, {} signals",
                                mode, fetched, de.stock_count(), sig_count
                            );
                        }

                        client.disconnect().await;
                    }
                    Err(e) => {
                        let mut state = dash_for_daily.lock().await;
                        state.daily_kline_status = "日K连接失败".to_string();
                        warn!("Daily K-line connect failed: {}", e);
                    }
                }
            }
        }))
    } else {
        None
    };

    // 分析 + 提醒任务
    let engine_clone = engine.clone();
    let alert_clone = alert_manager.clone();
    let dash_clone = dash_state.clone();
    let analysis_handle = tokio::spawn(async move {
        while let Some(quotes) = quote_rx.recv().await {
            // 分析
            let mut eng = engine_clone.lock().await;
            let results = eng.process_batch(&quotes);

            // 提醒
            let mut amgr = alert_clone.lock().await;
            for quote in &quotes {
                if let Some((ti, signals)) = results.get(&quote.code) {
                    let events = amgr.evaluate(quote, ti, signals).await;
                    if !events.is_empty() {
                        let mut state = dash_clone.lock().await;
                        state.recent_alerts.extend(events);
                    }
                }
            }

            // 更新仪表盘状态
            let mut state = dash_clone.lock().await;
            state.update_quotes(quotes);

            // 更新指标和信号
            for (code, (ti, sigs)) in &results {
                state.indicators.insert(code.clone(), ti.clone());
                state.signals.insert(code.clone(), sigs.clone());
            }
        }
    });

    // UI 主循环
    let mut terminal = ui::dashboard::init_terminal()?;
    let dash_for_ui = dash_state.clone();

    loop {
        // 渲染
        {
            let state = dash_for_ui.lock().await;
            terminal.draw(|frame| {
                ui::dashboard::render(frame, &state);
            })?;
        }

        // 处理输入
        {
            let mut state = dash_for_ui.lock().await;
            if ui::dashboard::handle_input(&mut state)? {
                break;
            }
        }
    }

    // 清理
    ui::dashboard::restore_terminal()?;
    fetch_handle.abort();
    analysis_handle.abort();
    if let Some(h) = daily_refresh_handle {
        h.abort();
    }

    info!("qtrade 已退出");
    Ok(())
}

/// 测试 FutuOpenD OpenAPI 连接
async fn cmd_test_api(config: AppConfig) -> Result<()> {
    use crate::futu::openapi::OpenApiClient;

    println!("测试 FutuOpenD 连接...");
    println!(
        "目标: {}:{}",
        config.futu.opend_host, config.futu.opend_port
    );

    let mut client = OpenApiClient::new(&config.futu.opend_host, config.futu.opend_port);

    // 连接 + InitConnect
    client.connect().await?;
    println!("✓ 连接成功");

    // 读取自选股列表
    let watchlist = futu::watchlist::load_watchlist(
        config.futu.data_path.as_deref(),
        config.futu.user_id.as_deref(),
    )?;

    if watchlist.is_empty() {
        println!("自选股列表为空");
        return Ok(());
    }

    // 只取前 5 只港股测试（排除 A 股指数和 800xxx 内部代码）
    let test_stocks: Vec<StockCode> = watchlist
        .iter()
        .filter(|e| {
            e.code.market == crate::models::Market::HK
                && !e.code.code.starts_with("800")
        })
        .take(5)
        .map(|e| e.code.clone())
        .collect();

    println!(
        "\n请求 {} 只股票的实时行情...",
        test_stocks.len()
    );
    for s in &test_stocks {
        println!("  {}", s.display_code());
    }

    // 先订阅（subType=1 表示基本报价）
    println!("\n订阅行情...");
    client.subscribe(&test_stocks, &[1]).await?;
    println!("✓ 订阅成功");

    // 构建中文名映射
    let name_map: std::collections::HashMap<&StockCode, &str> = watchlist
        .iter()
        .map(|e| (&e.code, e.name.as_str()))
        .collect();

    // 获取行情
    match client.get_basic_quotes(&test_stocks).await {
        Ok(quotes) => {
            println!("\n✓ 收到 {} 条行情:", quotes.len());
            println!(
                "{:<16} {:<12} {:>10} {:>10} {:>10}",
                "代码", "名称", "最新价", "涨跌", "涨跌%"
            );
            println!("{:-<65}", "");
            for q in &quotes {
                let api_name = q.name.as_str();
                let name = name_map.get(&q.code).copied().unwrap_or(api_name);
                println!(
                    "{:<16} {:<12} {:>10.2} {:>+10.2} {:>+9.2}%",
                    q.code.display_code(),
                    name,
                    q.last_price,
                    q.change,
                    q.change_pct
                );
            }
        }
        Err(e) => {
            println!("\n✗ 获取行情失败: {}", e);
        }
    }

    // 优雅断开连接
    client.disconnect().await;
    println!("\n✓ 连接已断开");

    Ok(())
}

/// 测试窗口截图 + Vision OCR
async fn cmd_test_ocr(_config: AppConfig) -> Result<()> {
    use crate::futu::accessibility::AccessibilityReader;
    use crate::futu::ocr;

    println!("测试窗口截图 + Vision OCR...\n");

    // 0. 检查屏幕录制权限
    if !ocr::check_screen_capture_permission() {
        println!("⚠ 屏幕录制权限未授权，正在请求...");
        if !ocr::request_screen_capture_permission() {
            anyhow::bail!(
                "屏幕录制权限未授权。\n\
                 请前往 系统设置 → 隐私与安全性 → 屏幕录制 中授权当前终端 App，然后重试"
            );
        }
    }
    println!("✓ 屏幕录制权限已授权");

    // 1. 查找富途进程
    println!("查找富途牛牛进程...");
    let pid = AccessibilityReader::find_futu_pid()?;
    println!("  PID: {}", pid);

    // 2. 查找窗口
    println!("\n查找主窗口...");
    let window_id = ocr::find_futu_window_id(pid)?;
    println!("  窗口 ID: {}", window_id);

    // 3. 截图
    println!("\n截取窗口截图...");
    let image = ocr::capture_window(window_id)?;
    println!(
        "  截图尺寸: {}x{} 像素",
        objc2_core_graphics::CGImage::width(Some(&image)),
        objc2_core_graphics::CGImage::height(Some(&image)),
    );

    // 4. Pass 1: 快速 OCR → 检测布局
    println!("\n[Pass 1] 快速 OCR 检测布局...");
    let t0 = std::time::Instant::now();
    let fast_blocks = ocr::recognize_text_fast(&image)?;
    let fast_ms = t0.elapsed().as_millis();
    println!("  Fast OCR: {} 个文字块 ({} ms)", fast_blocks.len(), fast_ms);

    let layout = ocr::detect_layout(&fast_blocks);
    println!(
        "  自选股区域: x = {:.1}% ~ {:.1}%",
        layout.watchlist_x.0 * 100.0,
        layout.watchlist_x.1 * 100.0
    );
    if let Some((ql, qr)) = layout.quote_x {
        println!(
            "  报价详情区域: x = {:.1}% ~ {:.1}%",
            ql * 100.0,
            qr * 100.0
        );
    } else {
        println!("  报价详情区域: 未检测到");
    }

    // 5. Pass 2: 裁剪 → 精确 OCR
    println!("\n[Pass 2] 裁剪自选股区域 → 精确 OCR...");
    let watchlist_crop = ocr::crop_image(&image, layout.watchlist_x)?;
    println!(
        "  裁剪尺寸: {}x{}",
        objc2_core_graphics::CGImage::width(Some(&watchlist_crop)),
        objc2_core_graphics::CGImage::height(Some(&watchlist_crop)),
    );

    let t1 = std::time::Instant::now();
    let blocks = ocr::recognize_text(&watchlist_crop)?;
    let acc_ms = t1.elapsed().as_millis();
    println!("  Accurate OCR: {} 个文字块 ({} ms)", blocks.len(), acc_ms);

    // 6. 分行
    println!("\n--- 行分组结果 ---");
    let rows = ocr::group_into_rows(&blocks);
    println!("  共 {} 行", rows.len());
    for (i, row) in rows.iter().enumerate() {
        let line: String = row.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join(" | ");
        println!("  行{:2}: {}", i, line);
    }

    // 7. 解析自选股行情
    println!("\n--- 自选股行情 ---");
    let quotes = ocr::parse_watchlist_from_ocr(&rows);
    let session = crate::models::us_market_session();
    let session_label = session.extended_label();
    for q in &quotes {
        let ext_info = match (q.extended_price, q.extended_change_pct) {
            (Some(ep), Some(epc)) => format!("  {}:{:.3} {:>+.2}%", session_label, ep, epc),
            (Some(ep), None) => format!("  {}:{:.3}", session_label, ep),
            _ => String::new(),
        };
        println!(
            "  {} {:<12} {:>10.3}  {:>+8.3}  {:>+7.2}%{}",
            q.code.display_code(),
            q.name,
            q.last_price,
            q.change,
            q.change_pct,
            ext_info,
        );
    }

    // 8. 裁剪报价详情区域
    if let Some(quote_range) = layout.quote_x {
        println!("\n[Pass 2] 裁剪报价详情区域 → 精确 OCR...");
        let quote_crop = ocr::crop_image(&image, quote_range)?;
        println!(
            "  裁剪尺寸: {}x{}",
            objc2_core_graphics::CGImage::width(Some(&quote_crop)),
            objc2_core_graphics::CGImage::height(Some(&quote_crop)),
        );

        let t2 = std::time::Instant::now();
        let quote_blocks = ocr::recognize_text(&quote_crop)?;
        let q_ms = t2.elapsed().as_millis();
        println!("  Accurate OCR: {} 个文字块 ({} ms)", quote_blocks.len(), q_ms);

        let quote_rows = ocr::group_into_rows(&quote_blocks);
        println!("\n--- 报价详情 ---");
        for (i, row) in quote_rows.iter().enumerate() {
            let line: String = row.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join(" | ");
            println!("  行{:2}: {}", i, line);
        }
    }

    println!(
        "\n共解析 {} 条行情 (Fast {}ms + Accurate {}ms)",
        quotes.len(),
        fast_ms,
        acc_ms
    );

    Ok(())
}
