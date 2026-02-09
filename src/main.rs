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
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch, Mutex};
use tracing::{info, warn};

use crate::alerts::manager::AlertManager;
use crate::alerts::notify::Notifier;
use crate::alerts::rules::ChangeThresholdRule;
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

/// 过滤 watchlist entries：去掉 800xxx 内部索引和 Unknown 市场
fn filter_stock_codes(watchlist: &[models::WatchlistEntry]) -> Vec<StockCode> {
    watchlist
        .iter()
        .filter(|e| {
            !e.code.code.starts_with("800")
                && e.code.market != crate::models::Market::Unknown
        })
        .map(|e| e.code.clone())
        .collect()
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

    let stock_codes = filter_stock_codes(&watchlist);
    info!("可订阅股票: {} 只（已过滤内部索引代码）", stock_codes.len());

    // 检测 plist 路径（用于 mtime 监测）
    let plist_path = futu::watchlist::detect_plist_path(
        config.futu.data_path.as_deref(),
        config.futu.user_id.as_deref(),
    )?;
    info!("Plist path for monitoring: {}", plist_path.display());

    // 创建 watch channel 广播 watchlist 变化
    let (watch_tx, watch_rx) = watch::channel(stock_codes.clone());

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
    {
        let mut de = daily_engine.lock().await;
        de.load_cache();
        if de.stock_count() > 0 {
            info!("Loaded daily kline cache: {} stocks", de.stock_count());
        }
    };

    // 创建分析引擎
    let engine = Arc::new(Mutex::new(AnalysisEngine::new(&config.analysis)));

    // 创建提醒管理器
    let notifier = Notifier::new(config.alerts.webhook_url.clone());
    let mut alert_manager = AlertManager::new(notifier);
    if config.alerts.enabled {
        alert_manager.add_rule(Box::new(ChangeThresholdRule::new(
            config.alerts.change_threshold_pct,
        )));
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

    // Plist 监测任务（3s 轮询 mtime）
    let monitor_plist_path = plist_path.clone();
    let monitor_config_data_path = config.futu.data_path.clone();
    let monitor_config_user_id = config.futu.user_id.clone();
    let monitor_engine = engine.clone();
    let monitor_daily_engine = daily_engine.clone();
    let monitor_dash = dash_state.clone();
    let monitor_watch_tx = watch_tx.clone();
    let monitor_handle = tokio::spawn(async move {
        let mut last_mtime = monitor_plist_path
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);

        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;

            let current_mtime = match monitor_plist_path.metadata().and_then(|m| m.modified()) {
                Ok(mt) => mt,
                Err(_) => continue,
            };

            if current_mtime == last_mtime {
                continue;
            }
            last_mtime = current_mtime;
            info!("Plist mtime changed, reloading watchlist...");

            // 重新加载 watchlist
            let new_watchlist = match futu::watchlist::load_watchlist(
                monitor_config_data_path.as_deref(),
                monitor_config_user_id.as_deref(),
            ) {
                Ok(w) => w,
                Err(e) => {
                    warn!("Failed to reload watchlist: {}", e);
                    continue;
                }
            };

            let new_codes = filter_stock_codes(&new_watchlist);
            let old_codes: Vec<StockCode> = monitor_watch_tx.borrow().clone();

            let old_set: HashSet<&StockCode> = old_codes.iter().collect();
            let new_set: HashSet<&StockCode> = new_codes.iter().collect();

            let added: Vec<&StockCode> = new_set.difference(&old_set).copied().collect();
            let removed: Vec<&StockCode> = old_set.difference(&new_set).copied().collect();

            if added.is_empty() && removed.is_empty() {
                info!("Watchlist codes unchanged after reload");
                continue;
            }

            info!(
                "Watchlist changed: +{} added, -{} removed",
                added.len(),
                removed.len()
            );

            // 处理删除
            if !removed.is_empty() {
                let removed_codes: Vec<StockCode> = removed.iter().map(|c| (*c).clone()).collect();
                // 清理 tick 分析引擎
                {
                    let mut eng = monitor_engine.lock().await;
                    for code in &removed_codes {
                        eng.remove_stock(code);
                    }
                }
                // 清理日线分析引擎
                {
                    let mut de = monitor_daily_engine.lock().await;
                    de.remove_stocks(&removed_codes);
                }
                for code in &removed_codes {
                    info!("Removed stock: {}", code.display_code());
                }
            }

            // 同步 dashboard
            {
                let filtered_entries: Vec<_> = new_watchlist
                    .iter()
                    .filter(|e| {
                        !e.code.code.starts_with("800")
                            && e.code.market != crate::models::Market::Unknown
                    })
                    .cloned()
                    .collect();
                let mut state = monitor_dash.lock().await;
                state.sync_watchlist(&new_codes, &filtered_entries);
            }

            if !added.is_empty() {
                for code in &added {
                    info!("Added stock: {}", code.display_code());
                }
            }

            // 广播新的 stock_codes
            let _ = monitor_watch_tx.send(new_codes);
        }
    });

    // 数据采集任务（使用 watch channel 感知 watchlist 变化）
    let refresh_interval = Duration::from_secs(config.data_source.refresh_interval_secs);
    let dash_for_fetch = dash_state.clone();
    let mut watch_rx_fetch = watch_rx.clone();
    let fetch_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(refresh_interval);
        let mut current_codes = watch_rx_fetch.borrow_and_update().clone();

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match provider.get_quotes(&current_codes).await {
                        Ok(quotes) => {
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
                result = watch_rx_fetch.changed() => {
                    if result.is_err() {
                        break; // sender dropped
                    }
                    let new_codes = watch_rx_fetch.borrow_and_update().clone();
                    let old_set: HashSet<StockCode> = current_codes.iter().cloned().collect();
                    let new_set: HashSet<StockCode> = new_codes.iter().cloned().collect();

                    let removed: Vec<StockCode> = old_set.difference(&new_set).cloned().collect();
                    let added: Vec<StockCode> = new_set.difference(&old_set).cloned().collect();

                    // 退订已删股票
                    if !removed.is_empty() {
                        if let Err(e) = provider.unsubscribe(&removed).await {
                            warn!("Unsubscribe failed: {}", e);
                        }
                    }

                    // 订阅新增股票
                    if !added.is_empty() {
                        if let Err(e) = provider.subscribe(&added).await {
                            warn!("Subscribe new stocks failed: {}", e);
                        }
                    }

                    current_codes = new_codes;
                    info!("Fetch loop updated: {} stocks", current_codes.len());
                }
            }
        }
    });

    // 日K线异步获取任务（使用 watch channel 感知新增股票）
    let daily_refresh_handle = if config.analysis.daily_kline_enabled {
        let daily_engine_clone = daily_engine.clone();
        let dash_for_daily = dash_state.clone();
        let futu_host = config.futu.opend_host.clone();
        let futu_port = config.futu.opend_port;
        let daily_days = config.analysis.daily_kline_days;
        let refresh_mins = config.analysis.daily_kline_refresh_minutes;
        let mut watch_rx_daily = watch_rx.clone();

        info!("Daily K-line target: {} stocks", stock_codes.len());

        Some(tokio::spawn(async move {
            let mut current_codes = watch_rx_daily.borrow_and_update().clone();

            // 首次立即拉取
            run_daily_kline_cycle(
                &futu_host,
                futu_port,
                &current_codes,
                &daily_engine_clone,
                &dash_for_daily,
                daily_days,
            )
            .await;

            loop {
                // 等待定时刷新或 watchlist 变更
                let sleep_duration = if refresh_mins == 0 {
                    // 不定时刷新，但仍监听 watchlist 变更
                    Duration::from_secs(u64::MAX / 2)
                } else {
                    Duration::from_secs(refresh_mins * 60)
                };

                tokio::select! {
                    _ = tokio::time::sleep(sleep_duration) => {
                        // 定时全量刷新
                        current_codes = watch_rx_daily.borrow_and_update().clone();
                        run_daily_kline_cycle(
                            &futu_host,
                            futu_port,
                            &current_codes,
                            &daily_engine_clone,
                            &dash_for_daily,
                            daily_days,
                        )
                        .await;
                    }
                    result = watch_rx_daily.changed() => {
                        if result.is_err() {
                            break; // sender dropped
                        }
                        let new_codes = watch_rx_daily.borrow_and_update().clone();
                        let old_set: HashSet<StockCode> = current_codes.iter().cloned().collect();
                        let new_set: HashSet<StockCode> = new_codes.iter().cloned().collect();

                        let added: Vec<StockCode> = new_set.difference(&old_set).cloned().collect();
                        current_codes = new_codes;

                        // 仅对新增股票拉取日K线
                        if !added.is_empty() {
                            info!("Daily kline: fetching {} newly added stocks", added.len());
                            run_daily_kline_cycle(
                                &futu_host,
                                futu_port,
                                &added,
                                &daily_engine_clone,
                                &dash_for_daily,
                                daily_days,
                            )
                            .await;
                        }
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
    let tick_display_minutes = config.analysis.tick_signal_display_minutes;
    let analysis_handle = tokio::spawn(async move {
        while let Some(quotes) = quote_rx.recv().await {
            // 分析：事件型 tick 信号
            let mut eng = engine_clone.lock().await;
            let now = chrono::Local::now();
            let mut all_new_signals = std::collections::HashMap::new();
            for quote in &quotes {
                let new_sigs = eng.process(quote);
                if !new_sigs.is_empty() {
                    all_new_signals.insert(quote.code.clone(), new_sigs);
                }
            }
            drop(eng);

            // 提醒（仅 ChangeThresholdRule）
            let mut amgr = alert_clone.lock().await;
            for quote in &quotes {
                let events = amgr.evaluate(quote).await;
                if !events.is_empty() {
                    let mut state = dash_clone.lock().await;
                    state.recent_alerts.extend(events);
                }
            }
            drop(amgr);

            // 更新仪表盘状态
            let mut state = dash_clone.lock().await;
            state.update_quotes(quotes);

            // 写入新触发的 tick 信号
            for (code, sigs) in all_new_signals {
                let entry = state.tick_signals.entry(code).or_default();
                for sig in sigs {
                    entry.push((sig, now));
                }
            }

            // 清理过期 tick 信号
            let cutoff = now - chrono::Duration::minutes(tick_display_minutes as i64);
            state.tick_signals.retain(|_, sigs| {
                sigs.retain(|(_, at)| *at > cutoff);
                !sigs.is_empty()
            });
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
    monitor_handle.abort();
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

    // 2. 查找窗口（获取实际 GUI 进程 PID）
    println!("\n查找主窗口...");
    let win_info = ocr::find_futu_window(pid)?;
    let window_id = win_info.id;
    let gui_pid = win_info.owner_pid;
    println!("  窗口 ID: {}  GUI PID: {}", window_id, gui_pid);

    // 2.5 尝试 AX API 检测自选股表格区域（使用 GUI PID）
    println!("\n[AX] 检测自选股表格区域...");
    let grid_frame = match crate::futu::accessibility::find_watchlist_grid_frame(gui_pid) {
        Ok(frame) => {
            println!(
                "  ✓ 检测到 GridFrame: x={:.1}% y={:.1}% w={:.1}% h={:.1}%",
                frame.x * 100.0,
                frame.y * 100.0,
                frame.width * 100.0,
                frame.height * 100.0,
            );
            Some(frame)
        }
        Err(e) => {
            println!("  ✗ AX 检测失败: {}，将使用 Pass 1 OCR", e);
            None
        }
    };

    // 3-7: 截图 + OCR + 解析（同步 macOS API，必须在 spawn_blocking 中运行，
    //       否则 CGWindowListCreateImage 可能在 tokio 主线程上死锁）
    println!("\n截取窗口截图 + OCR...");
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        let image = ocr::capture_window(window_id)?;
        println!(
            "  截图尺寸: {}x{} 像素",
            objc2_core_graphics::CGImage::width(Some(&image)),
            objc2_core_graphics::CGImage::height(Some(&image)),
        );

        let mut fast_ms: u128 = 0;

        // 4. 布局检测：AX 优先，降级到 Pass 1
        let watchlist_crop = if let Some(gf) = grid_frame {
            println!("\n[AX] 使用 AX GridFrame 裁剪（跳过 Pass 1）...");
            let x_range = (gf.x, (gf.x + gf.width).min(1.0));
            let y_range = Some((gf.y, (gf.y + gf.height).min(1.0)));
            ocr::crop_image_xy(&image, x_range, y_range)?
        } else {
            println!("\n[Pass 1] 快速 OCR 检测布局...");
            let t0 = std::time::Instant::now();
            let fast_blocks = ocr::recognize_text_fast(&image)?;
            fast_ms = t0.elapsed().as_millis();
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
            ocr::crop_image(&image, layout.watchlist_x)?
        };

        // 保存截图和裁剪图片供调试
        let tmp_dir = std::path::Path::new("/tmp/qtrade-ocr");
        let _ = std::fs::create_dir_all(tmp_dir);
        save_cgimage_png(&image, &tmp_dir.join("full.png"));
        save_cgimage_png(&watchlist_crop, &tmp_dir.join("crop.png"));
        println!("  已保存: /tmp/qtrade-ocr/full.png, /tmp/qtrade-ocr/crop.png");

        // 5. Pass 2: 精确 OCR
        println!("\n[Pass 2] 裁剪区域 → 精确 OCR...");
        println!(
            "  裁剪尺寸: {}x{}",
            objc2_core_graphics::CGImage::width(Some(&watchlist_crop)),
            objc2_core_graphics::CGImage::height(Some(&watchlist_crop)),
        );

        let t1 = std::time::Instant::now();
        let blocks = ocr::recognize_text(&watchlist_crop)?;
        let acc_ms = t1.elapsed().as_millis();
        println!("  Accurate OCR: {} 个文字块 ({} ms)", blocks.len(), acc_ms);

        // 打印每个 OCR 块的详细信息
        println!("\n--- OCR 原始块 ---");
        for (i, b) in blocks.iter().enumerate() {
            println!(
                "  [{}] conf={:.2} bbox=({:.3},{:.3},{:.3},{:.3}) \"{}\"",
                i, b.confidence, b.bbox.0, b.bbox.1, b.bbox.2, b.bbox.3, b.text
            );
        }

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

        if grid_frame.is_some() {
            println!(
                "\n共解析 {} 条行情 (AX跳过Pass1 + Accurate {}ms)",
                quotes.len(),
                acc_ms,
            );
        } else {
            println!(
                "\n共解析 {} 条行情 (Fast {}ms + Accurate {}ms)",
                quotes.len(),
                fast_ms,
                acc_ms
            );
        }

        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking failed: {}", e))?;

    result?;

    Ok(())
}

/// 计算两个日期字符串之间的自然日间隔（"YYYY-MM-DD" 格式）
/// 判断错误信息是否为市场权限不足
fn is_permission_error(msg: &str) -> bool {
    msg.contains("无权限")
        || msg.contains("权限")
        || msg.contains("permission")
        || msg.contains("未开通")
        || msg.contains("no quota")
        || msg.contains("not available")
        || msg.contains("暂不提供")
        || msg.contains("暂不支持")
}

fn date_gap_days(from: &str, to: &str) -> u32 {
    let from_date = match chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return u32::MAX, // 解析失败视为需要全量
    };
    let to_date = match chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return u32::MAX,
    };
    (to_date - from_date).num_days().max(0) as u32
}

/// 按市场探测权限：每个市场试拉一只股票的K线，返回无权限的市场集合
async fn probe_market_permissions(
    client: &mut crate::futu::openapi::OpenApiClient,
    stocks: &[StockCode],
) -> std::collections::HashSet<crate::models::Market> {
    let mut no_permission = std::collections::HashSet::new();
    let today_str = chrono::Local::now().format("%Y-%m-%d").to_string();
    let probe_begin = (chrono::Local::now() - chrono::Duration::days(5))
        .format("%Y-%m-%d")
        .to_string();
    let mut probed = std::collections::HashSet::new();

    for stock in stocks {
        if probed.contains(&stock.market) || stock.market == crate::models::Market::Unknown {
            continue;
        }
        probed.insert(stock.market);
        match client
            .request_history_kline(stock, &probe_begin, &today_str, 2)
            .await
        {
            Ok(_) => {
                info!("市场权限检测: {} ✓", stock.market);
            }
            Err(e) => {
                let msg = format!("{}", e);
                if is_permission_error(&msg) {
                    info!("市场权限检测: {} ✗ 无权限", stock.market);
                    no_permission.insert(stock.market);
                } else {
                    warn!("市场权限检测: {} 探测失败 ({})", stock.market, msg);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    if !no_permission.is_empty() {
        let skipped: Vec<String> = no_permission.iter().map(|m| format!("{}", m)).collect();
        info!(
            "以下市场无权限，将跳过日K拉取: {}",
            skipped.join(", ")
        );
    }

    no_permission
}

/// 拉取单只股票的日K线并合并到引擎缓存，返回 Ok(true) 表示成功拉取新数据
///
/// 返回 Err 且 is_permission_error 为 true 时，调用方应将该市场加入无权限集合。
async fn fetch_and_merge_stock_kline(
    client: &mut crate::futu::openapi::OpenApiClient,
    stock: &StockCode,
    engine: &Mutex<DailyAnalysisEngine>,
    today: &str,
    daily_days: u32,
) -> Result<bool> {
    // 今天已拉取过 → 跳过
    {
        let de = engine.lock().await;
        if de.last_fetched_date(stock) == Some(today) {
            return Ok(false);
        }
    }

    // 确定拉取天数：无缓存→全量，有缓存→gap+5 自适应
    let (fetch_days, last_date) = {
        let de = engine.lock().await;
        match de.last_kline_date(stock) {
            Some(ld) => {
                let gap = date_gap_days(&ld, today);
                let days = gap.saturating_add(5).max(5).min(daily_days);
                (days, Some(ld))
            }
            None => (daily_days, None),
        }
    };

    let end = today.to_string();
    let begin = (chrono::Local::now() - chrono::Duration::days(fetch_days as i64 * 2))
        .format("%Y-%m-%d")
        .to_string();

    let klines = client
        .request_history_kline(stock, &begin, &end, fetch_days)
        .await?;

    if klines.is_empty() {
        return Ok(false);
    }

    // 验证连续性：缓存尾部日期必须出现在新数据中
    let mut de = engine.lock().await;
    if let Some(ref ld) = last_date {
        let has_overlap = klines.iter().any(|k| &k.date == ld);
        if has_overlap {
            let mut data = std::collections::HashMap::new();
            data.insert(stock.clone(), klines);
            de.merge_update(data);
        } else {
            warn!(
                "{}: cache discontinuous (last_date={}, fetched {}~{}), replacing",
                stock.display_code(),
                ld,
                klines.first().map(|k| k.date.as_str()).unwrap_or("?"),
                klines.last().map(|k| k.date.as_str()).unwrap_or("?"),
            );
            de.replace_stock(stock.clone(), klines);
        }
    } else {
        let mut data = std::collections::HashMap::new();
        data.insert(stock.clone(), klines);
        de.merge_update(data);
    }
    de.mark_fetched(stock, today);

    Ok(true)
}

/// 执行一轮日K线拉取：连接 FutuOpenD、探测权限、逐只拉取、保存缓存、更新 dashboard
async fn run_daily_kline_cycle(
    futu_host: &str,
    futu_port: u16,
    daily_codes: &[StockCode],
    daily_engine: &Arc<Mutex<DailyAnalysisEngine>>,
    dash_state: &Arc<Mutex<DashboardState>>,
    daily_days: u32,
) {
    let total = daily_codes.len();
    info!("Fetching daily K-line data ({}只)...", total);
    {
        let mut state = dash_state.lock().await;
        state.daily_kline_status = format!("日K拉取中(0/{})", total);
    }

    let mut client = crate::futu::openapi::OpenApiClient::new(futu_host, futu_port);
    match client.connect().await {
        Ok(()) => {
            let mut no_permission_markets =
                probe_market_permissions(&mut client, daily_codes).await;
            let today_str = chrono::Local::now().format("%Y-%m-%d").to_string();

            let mut fetched = 0u32;
            for (i, stock) in daily_codes.iter().enumerate() {
                if no_permission_markets.contains(&stock.market) {
                    continue;
                }

                match fetch_and_merge_stock_kline(
                    &mut client,
                    stock,
                    daily_engine,
                    &today_str,
                    daily_days,
                )
                .await
                {
                    Ok(true) => fetched += 1,
                    Ok(false) => {}
                    Err(e) => {
                        let msg = format!("{}", e);
                        if is_permission_error(&msg) {
                            warn!("{} market no permission, skipping: {}", stock.market, msg);
                            no_permission_markets.insert(stock.market);
                        } else {
                            warn!(
                                "Failed to get klines for {}: {}",
                                stock.display_code(),
                                msg
                            );
                        }
                    }
                }

                // 更新进度
                {
                    let mut state = dash_state.lock().await;
                    state.daily_kline_status = format!("日K拉取中({}/{})", i + 1, total);
                }

                // 每 10 只存盘一次 + 同步 dashboard
                if (i + 1) % 10 == 0 || i + 1 == total {
                    let de = daily_engine.lock().await;
                    de.save_cache();
                    let mut state = dash_state.lock().await;
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
                let de = daily_engine.lock().await;
                let mut state = dash_state.lock().await;
                state.daily_indicators = de.get_indicators().clone();
                state.daily_signals = de.get_signals().clone();
                let sig_count: usize = state.daily_signals.values().map(|v| v.len()).sum();
                state.daily_kline_status =
                    format!("日K:{}只 信号:{}", de.stock_count(), sig_count);
                info!(
                    "Daily K-line complete: fetched {}, total {}, {} signals",
                    fetched,
                    de.stock_count(),
                    sig_count
                );
            }

            client.disconnect().await;
        }
        Err(e) => {
            let mut state = dash_state.lock().await;
            state.daily_kline_status = "日K连接失败".to_string();
            warn!("Daily K-line connect failed: {}", e);
        }
    }
}

/// 将 CGImage 保存为 PNG 文件（通过 macOS ImageIO 框架）
fn save_cgimage_png(image: &objc2_core_foundation::CFRetained<objc2_core_graphics::CGImage>, path: &std::path::Path) {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_foundation::url::CFURL;

    extern "C" {
        fn CGImageDestinationCreateWithURL(
            url: core_foundation::url::CFURLRef,
            type_: core_foundation::string::CFStringRef,
            count: usize,
            options: *const std::ffi::c_void,
        ) -> *mut std::ffi::c_void;

        fn CGImageDestinationAddImage(
            dest: *mut std::ffi::c_void,
            image: *const std::ffi::c_void,
            properties: *const std::ffi::c_void,
        );

        fn CGImageDestinationFinalize(dest: *mut std::ffi::c_void) -> bool;
    }

    let path_str = path.to_string_lossy().to_string();
    let url = CFURL::from_file_system_path(CFString::new(&path_str), core_foundation::url::kCFURLPOSIXPathStyle, false);
    let png_type = CFString::new("public.png");

    unsafe {
        let dest = CGImageDestinationCreateWithURL(
            url.as_concrete_TypeRef(),
            png_type.as_concrete_TypeRef(),
            1,
            std::ptr::null(),
        );
        if dest.is_null() {
            eprintln!("Failed to create image destination for {}", path_str);
            return;
        }
        let raw_img = objc2_core_foundation::CFRetained::as_ptr(image).as_ptr() as *const std::ffi::c_void;
        CGImageDestinationAddImage(dest, raw_img, std::ptr::null());
        if !CGImageDestinationFinalize(dest) {
            eprintln!("Failed to finalize image at {}", path_str);
        }
    }
}
