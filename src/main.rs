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
