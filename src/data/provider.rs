//! 数据提供者 trait 与调度
//!
//! 抽象数据源，支持 Accessibility API 和 FutuOpenD OpenAPI 两种实现

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::info;

use crate::config::AppConfig;
use crate::futu::accessibility::AccessibilityReader;
use crate::futu::openapi::OpenApiClient;
use crate::models::{QuoteSnapshot, StockCode};

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

    pub fn name(&self) -> &str {
        "OpenAPI"
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn set_quote_channel(&mut self, tx: mpsc::Sender<QuoteSnapshot>) {
        self.client.set_quote_channel(tx);
    }
}

/// 数据源类型（枚举分发，无需 async_trait）
pub enum DataProviderKind {
    Accessibility(AccessibilityProvider),
    OpenApi(OpenApiProvider),
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
        }
    }

    /// 订阅行情（OpenAPI 模式需要先订阅）
    pub async fn subscribe(&mut self, codes: &[StockCode]) -> Result<()> {
        match self {
            DataProviderKind::Accessibility(_) => Ok(()), // AX 不需要订阅
            DataProviderKind::OpenApi(p) => p.subscribe(codes).await,
        }
    }

    pub async fn get_quotes(&mut self, codes: &[StockCode]) -> Result<Vec<QuoteSnapshot>> {
        match self {
            DataProviderKind::Accessibility(p) => p.get_quotes(codes).await,
            DataProviderKind::OpenApi(p) => p.get_quotes(codes).await,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            DataProviderKind::Accessibility(p) => p.name(),
            DataProviderKind::OpenApi(p) => p.name(),
        }
    }

    pub fn is_connected(&self) -> bool {
        match self {
            DataProviderKind::Accessibility(p) => p.is_connected(),
            DataProviderKind::OpenApi(p) => p.is_connected(),
        }
    }
}
