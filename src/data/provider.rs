//! 数据提供者 trait 与调度
//!
//! 抽象数据源，支持 Accessibility API 和 FutuOpenD OpenAPI 两种实现

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use std::collections::{HashMap, HashSet};

use crate::config::AppConfig;
use crate::futu::accessibility::AccessibilityReader;
use crate::futu::ocr;
use crate::futu::openapi::OpenApiClient;
use crate::models::{DailyKline, Market, QuoteSnapshot, StockCode};

/// Accessibility API 数据提供者
pub struct AccessibilityProvider {
    reader: AccessibilityReader,
    connected: bool,
}

impl AccessibilityProvider {
    pub fn new() -> Self {
        Self {
            reader: AccessibilityReader::new(),
            connected: false,
        }
    }

    pub async fn connect(&mut self) -> Result<()> {
        self.reader.connect()?;
        self.connected = true;
        Ok(())
    }

    pub async fn get_quotes(&mut self, _codes: &[StockCode]) -> Result<Vec<QuoteSnapshot>> {
        self.reader.read_quotes()
    }

    pub fn name(&self) -> &str {
        "Accessibility"
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }
}

/// OpenAPI 数据提供者
pub struct OpenApiProvider {
    client: OpenApiClient,
    connected: bool,
}

impl OpenApiProvider {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            client: OpenApiClient::new(host, port),
            connected: false,
        }
    }

    pub async fn connect(&mut self) -> Result<()> {
        self.client.connect().await?;
        self.connected = true;
        Ok(())
    }

    /// 订阅行情（必须在 get_quotes 之前调用）
    pub async fn subscribe(&mut self, codes: &[StockCode]) -> Result<()> {
        // subType=1 表示基本报价
        self.client.subscribe(codes, &[1]).await
    }

    pub async fn get_quotes(&mut self, codes: &[StockCode]) -> Result<Vec<QuoteSnapshot>> {
        self.client.get_basic_quotes(codes).await
    }

    /// 获取历史日K线数据
    pub async fn get_daily_klines(
        &mut self,
        stocks: &[StockCode],
        days: u32,
    ) -> Result<HashMap<StockCode, Vec<DailyKline>>> {
        let end = chrono::Local::now().format("%Y-%m-%d").to_string();
        let begin = (chrono::Local::now() - chrono::Duration::days(days as i64 * 2))
            .format("%Y-%m-%d")
            .to_string();
        self.client
            .request_history_kline_batch(stocks, &begin, &end, days)
            .await
    }

    pub fn name(&self) -> &str {
        "OpenAPI"
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn set_quote_channel(&mut self, tx: mpsc::Sender<QuoteSnapshot>) {
        self.client.set_quote_channel(tx);
    }

    pub fn subscribed_markets(&self) -> HashSet<Market> {
        self.client.subscribed_markets().clone()
    }
}

/// OCR 数据提供者（窗口截图 + Vision OCR）
pub struct OcrProvider {
    futu_pid: Option<i32>,
    connected: bool,
    /// 上一次窗口尺寸，用于 resize 检测
    last_window_size: Option<(f64, f64)>,
    /// 上一次截图哈希，用于跳过未变化的帧
    last_image_hash: String,
    /// 上一轮有效 quotes 缓存（图像未变化时复用）
    last_quotes: Vec<QuoteSnapshot>,
    /// 非白名单代码的连续出现计数（连续 >=2 轮视为新增自选股）
    unknown_code_streak: HashMap<StockCode, u32>,
    /// 上一轮有效结果的代码集（用于清理 streak）
    last_valid_codes: HashSet<StockCode>,
}

impl OcrProvider {
    pub fn new() -> Self {
        Self {
            futu_pid: None,
            connected: false,
            last_window_size: None,
            last_image_hash: String::new(),
            last_quotes: Vec::new(),
            unknown_code_streak: HashMap::new(),
            last_valid_codes: HashSet::new(),
        }
    }

    pub async fn connect(&mut self) -> Result<()> {
        let pid = AccessibilityReader::find_futu_pid()?;
        self.futu_pid = Some(pid);
        self.connected = true;
        info!("OCR provider connected to Futu app (PID: {})", pid);
        Ok(())
    }

    pub async fn get_quotes(&mut self, _codes: &[StockCode]) -> Result<Vec<QuoteSnapshot>> {
        let pid = self
            .futu_pid
            .ok_or_else(|| anyhow::anyhow!("Not connected. Call connect() first."))?;

        // CG 截图和 Vision OCR 都是同步 API，放到阻塞线程池
        let prev_hash = self.last_image_hash.clone();
        let result = tokio::task::spawn_blocking(move || {
            ocr::ocr_capture_and_parse(pid, &prev_hash)
        })
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking failed: {}", e))??;

        // 图像未变化 → 直接返回缓存
        self.last_image_hash = result.image_hash;
        if result.skipped {
            return Ok(self.last_quotes.clone());
        }

        // Layer 1: 窗口 resize 检测 — 尺寸变化时跳过本轮
        let new_size = (result.window_width, result.window_height);
        if let Some(prev) = self.last_window_size {
            if (prev.0 - new_size.0).abs() > 1.0 || (prev.1 - new_size.1).abs() > 1.0 {
                warn!(
                    "Window resized ({:.0}x{:.0} → {:.0}x{:.0}), skipping OCR result",
                    prev.0, prev.1, new_size.0, new_size.1
                );
                self.last_window_size = Some(new_size);
                return Ok(Vec::new());
            }
        }
        self.last_window_size = Some(new_size);

        // Layer 2: 自选股白名单 + 连续出现计数
        // 每轮重新读取 plist 获取最新白名单
        let whitelist = self.load_whitelist();
        let mut accepted = Vec::new();
        let mut this_round_codes = HashSet::new();

        for q in result.quotes {
            this_round_codes.insert(q.code.clone());
            if whitelist.contains(&q.code) {
                accepted.push(q);
            } else {
                // 不在白名单：累计连续出现次数
                let count = self.unknown_code_streak.entry(q.code.clone()).or_insert(0);
                *count += 1;
                if *count >= 2 {
                    debug!("Accepting non-whitelist code {} (seen {} consecutive times)", q.code, *count);
                    accepted.push(q);
                } else {
                    debug!("Filtering non-whitelist code {} (first appearance)", q.code);
                }
            }
        }

        // 清理不再出现的 streak 计数
        self.unknown_code_streak.retain(|code, _| this_round_codes.contains(code));
        self.last_valid_codes = this_round_codes;

        // 缓存本轮结果（图像未变化时复用）
        self.last_quotes = accepted.clone();

        Ok(accepted)
    }

    /// 从 plist 读取当前自选股代码集作为白名单
    fn load_whitelist(&self) -> HashSet<StockCode> {
        match crate::futu::watchlist::load_watchlist(None, None) {
            Ok(entries) => entries.into_iter().map(|e| e.code).collect(),
            Err(e) => {
                warn!("Failed to load watchlist for whitelist: {}", e);
                HashSet::new()
            }
        }
    }

    pub fn name(&self) -> &str {
        "OCR"
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }
}

/// 数据源类型（枚举分发，无需 async_trait）
pub enum DataProviderKind {
    Accessibility(AccessibilityProvider),
    OpenApi(OpenApiProvider),
    Ocr(OcrProvider),
}

impl DataProviderKind {
    /// 根据配置创建数据提供者
    pub fn from_config(config: &AppConfig) -> Self {
        match config.data_source.source.as_str() {
            "openapi" => {
                info!("Using FutuOpenD OpenAPI data source");
                DataProviderKind::OpenApi(OpenApiProvider::new(
                    &config.futu.opend_host,
                    config.futu.opend_port,
                ))
            }
            "ocr" => {
                info!("Using window screenshot + Vision OCR data source");
                DataProviderKind::Ocr(OcrProvider::new())
            }
            _ => {
                info!("Using macOS Accessibility API data source");
                DataProviderKind::Accessibility(AccessibilityProvider::new())
            }
        }
    }

    pub async fn connect(&mut self) -> Result<()> {
        match self {
            DataProviderKind::Accessibility(p) => p.connect().await,
            DataProviderKind::OpenApi(p) => p.connect().await,
            DataProviderKind::Ocr(p) => p.connect().await,
        }
    }

    /// 订阅行情（OpenAPI 模式需要先订阅）
    pub async fn subscribe(&mut self, codes: &[StockCode]) -> Result<()> {
        match self {
            DataProviderKind::Accessibility(_) => Ok(()),
            DataProviderKind::OpenApi(p) => p.subscribe(codes).await,
            DataProviderKind::Ocr(_) => Ok(()),
        }
    }

    pub async fn get_quotes(&mut self, codes: &[StockCode]) -> Result<Vec<QuoteSnapshot>> {
        match self {
            DataProviderKind::Accessibility(p) => p.get_quotes(codes).await,
            DataProviderKind::OpenApi(p) => p.get_quotes(codes).await,
            DataProviderKind::Ocr(p) => p.get_quotes(codes).await,
        }
    }

    /// 获取历史日K线数据（Accessibility/OCR 模式返回空 Map）
    pub async fn get_daily_klines(
        &mut self,
        stocks: &[StockCode],
        days: u32,
    ) -> Result<HashMap<StockCode, Vec<DailyKline>>> {
        match self {
            DataProviderKind::Accessibility(_) => Ok(HashMap::new()),
            DataProviderKind::OpenApi(p) => p.get_daily_klines(stocks, days).await,
            DataProviderKind::Ocr(_) => Ok(HashMap::new()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            DataProviderKind::Accessibility(p) => p.name(),
            DataProviderKind::OpenApi(p) => p.name(),
            DataProviderKind::Ocr(p) => p.name(),
        }
    }

    pub fn is_connected(&self) -> bool {
        match self {
            DataProviderKind::Accessibility(p) => p.is_connected(),
            DataProviderKind::OpenApi(p) => p.is_connected(),
            DataProviderKind::Ocr(p) => p.is_connected(),
        }
    }

    /// 获取已订阅成功的市场集合
    pub fn subscribed_markets(&self) -> HashSet<Market> {
        match self {
            DataProviderKind::Accessibility(_) => HashSet::new(),
            DataProviderKind::OpenApi(p) => p.subscribed_markets(),
            DataProviderKind::Ocr(_) => HashSet::new(),
        }
    }
}
