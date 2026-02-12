//! MCP Server — 交易 MCP 工具（港股 + A股）
//!
//! 通过 Streamable HTTP 暴露 buy / sell / get_quote 工具，
//! 供大模型（如 Claude）调用。自动根据股票代码推断市场。

use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData as McpError, ServerHandler,
};

use crate::config::McpConfig;
use crate::trading::executor::{OrderRequest, OrderSide, TradingExecutor, TradingMarket};

// ===== Tool 参数定义 =====

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BuyParams {
    /// 股票代码：港股5位数字（如 "00700"），A股6位数字（如 "600519"、"000001"）
    #[schemars(description = "股票代码。港股5位（如 00700），A股6位（如 600519、000001、300750）")]
    pub stock_code: String,
    /// 委托价格（港股 HKD，A股 CNY）
    #[schemars(description = "委托价格（港股单位 HKD，A股单位 CNY）")]
    pub price: f64,
    /// 委托数量（股）
    #[schemars(description = "委托数量（股数）")]
    pub quantity: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SellParams {
    /// 股票代码：港股5位数字（如 "00700"），A股6位数字（如 "600519"、"000001"）
    #[schemars(description = "股票代码。港股5位（如 00700），A股6位（如 600519、000001、300750）")]
    pub stock_code: String,
    /// 委托价格（港股 HKD，A股 CNY）
    #[schemars(description = "委托价格（港股单位 HKD，A股单位 CNY）")]
    pub price: f64,
    /// 委托数量（股）
    #[schemars(description = "委托数量（股数）")]
    pub quantity: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetQuoteParams {
    /// 股票代码，如 "00700"（港股）、"600519"（A股）
    #[schemars(description = "股票代码")]
    pub stock_code: String,
}

// ===== MCP Server 结构体 =====

#[derive(Clone)]
pub struct QtradeMcpServer {
    executor: Arc<Mutex<TradingExecutor>>,
    tool_router: ToolRouter<Self>,
}

// ===== Tool 实现 =====

#[tool_router]
impl QtradeMcpServer {
    pub fn new(executor: Arc<Mutex<TradingExecutor>>) -> Self {
        Self {
            executor,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "买入委托。支持港股（5位代码如00700）和A股（6位代码如600519）。自动识别市场，通过财富通客户端提交限价买入订单。提交前会验证确认弹窗中的价格和代码。"
    )]
    async fn buy(
        &self,
        Parameters(params): Parameters<BuyParams>,
    ) -> Result<CallToolResult, McpError> {
        let market = TradingMarket::infer(&params.stock_code).ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "无法识别股票代码 '{}' 的市场。港股为5位数字（如 00700），A股为6位数字（如 600519）",
                    params.stock_code
                ),
                None,
            )
        })?;

        let req = OrderRequest {
            stock_code: params.stock_code,
            price: params.price,
            quantity: params.quantity,
            side: OrderSide::Buy,
            market,
        };

        let executor = self.executor.lock().await;
        match executor.execute_order(&req).await {
            Ok(result) => {
                let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                if result.success {
                    Ok(CallToolResult::success(vec![Content::text(json)]))
                } else {
                    Ok(CallToolResult::error(vec![Content::text(json)]))
                }
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "委托执行异常: {}",
                e
            ))])),
        }
    }

    #[tool(
        description = "卖出委托。支持港股（5位代码如00700）和A股（6位代码如600519）。自动识别市场，通过财富通客户端提交限价卖出订单。提交前会验证确认弹窗中的价格和代码。"
    )]
    async fn sell(
        &self,
        Parameters(params): Parameters<SellParams>,
    ) -> Result<CallToolResult, McpError> {
        let market = TradingMarket::infer(&params.stock_code).ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "无法识别股票代码 '{}' 的市场。港股为5位数字（如 00700），A股为6位数字（如 600519）",
                    params.stock_code
                ),
                None,
            )
        })?;

        let req = OrderRequest {
            stock_code: params.stock_code,
            price: params.price,
            quantity: params.quantity,
            side: OrderSide::Sell,
            market,
        };

        let executor = self.executor.lock().await;
        match executor.execute_order(&req).await {
            Ok(result) => {
                let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                if result.success {
                    Ok(CallToolResult::success(vec![Content::text(json)]))
                } else {
                    Ok(CallToolResult::error(vec![Content::text(json)]))
                }
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "委托执行异常: {}",
                e
            ))])),
        }
    }

    #[tool(
        description = "获取股票当前行情快照（只读）。返回最新价、涨跌幅等信息。可用于下单前确认价格。"
    )]
    async fn get_quote(
        &self,
        Parameters(params): Parameters<GetQuoteParams>,
    ) -> Result<CallToolResult, McpError> {
        let code = params.stock_code.clone();
        let result = tokio::task::spawn_blocking(move || get_quote_sync(&code))
            .await
            .map_err(|e| McpError::internal_error(format!("spawn error: {}", e), None))?;

        match result {
            Ok(info) => Ok(CallToolResult::success(vec![Content::text(info)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "获取行情失败: {}",
                e
            ))])),
        }
    }
}

// ===== ServerHandler =====

#[tool_handler]
impl ServerHandler for QtradeMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::LATEST,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "qtrade-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                title: None,
                description: None,
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "qtrade MCP 交易服务器 — 港股 + A股自动交易。\n\
                 提供 buy（买入）、sell（卖出）、get_quote（获取行情）三个工具。\n\
                 股票代码自动识别市场：港股5位（如 00700），A股6位（如 600519）。\n\
                 交易通过 macOS Accessibility API 驱动财富通V5.0客户端完成，需要辅助功能权限。"
                    .to_string(),
            ),
        }
    }
}

// ===== 行情查询（只读） =====

fn get_quote_sync(stock_code: &str) -> anyhow::Result<String> {
    let pid = crate::trading::executor::find_cft5_pid()?;
    let app = crate::futu::ax_action::create_app_element(pid)?;
    let window = crate::trading::executor::get_cft5_main_window(app)?;

    let elements = crate::futu::ax_action::find_all_elements(
        window,
        "AXStaticText",
        &crate::futu::ax_action::Matcher::TitleContains(stock_code),
        10,
    );

    if elements.is_empty() {
        return Ok(format!(
            "未在交易窗口中找到股票 {}。请确认该股票在自选股列表中。",
            stock_code
        ));
    }

    let mut info_parts = Vec::new();
    info_parts.push(format!("股票代码: {}", stock_code));

    for elem in &elements {
        if let Some(text) = crate::futu::ax_action::get_title(*elem)
            .or_else(|| crate::futu::ax_action::get_value_str(*elem))
        {
            info_parts.push(format!("找到: {}", text));
        }
    }

    Ok(info_parts.join("\n"))
}

// ===== Server 启动 =====

pub async fn run_mcp_server(config: &McpConfig) -> anyhow::Result<()> {
    info!("初始化交易执行器...");
    let executor = Arc::new(Mutex::new(TradingExecutor::new()?));
    info!("交易执行器就绪");

    let ct = tokio_util::sync::CancellationToken::new();

    let executor_for_factory = executor.clone();
    let service = StreamableHttpService::new(
        move || Ok(QtradeMcpServer::new(executor_for_factory.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig {
            cancellation_token: ct.child_token(),
            ..Default::default()
        },
    );

    let router =
        axum::Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn(
                |req: axum::extract::Request, next: axum::middleware::Next| async move {
                    debug!(
                        method = %req.method(),
                        uri = %req.uri(),
                        headers = ?req.headers(),
                        "MCP 收到请求"
                    );
                    let resp = next.run(req).await;
                    debug!(status = %resp.status(), "MCP 响应");
                    resp
                },
            ));

    let bind_addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!("MCP 服务器监听: http://{}/mcp", bind_addr);
    println!("MCP 服务器已启动: http://{}/mcp", bind_addr);
    println!("按 Ctrl+C 停止");

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.unwrap();
            info!("收到 Ctrl+C，正在关闭...");
            ct.cancel();
        })
        .await?;

    info!("MCP 服务器已关闭");
    Ok(())
}
