//! FutuOpenD OpenAPI 客户端
//!
//! 通过 TCP 连接 FutuOpenD 网关获取结构化行情数据。
//! 协议格式：固定 44 字节头部 + protobuf body
//! 默认连接地址：localhost:11111

use std::collections::HashSet;

use anyhow::{Context, Result};
use bytes::{Buf, BufMut, BytesMut};
use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::models::{DataSource, DailyKline, Market, QuoteSnapshot, StockCode};

/// Futu 协议头部大小
const HEADER_SIZE: usize = 44;

/// 协议魔数
const FUTU_MAGIC: [u8; 2] = [0x46, 0x54]; // "FT"

/// 协议版本
const PROTO_VERSION: u8 = 0;

/// 常用协议号
mod proto_id {
    pub const INIT_CONNECT: u32 = 1001;
    pub const KEEP_ALIVE: u32 = 1004;
    pub const QOT_SUB: u32 = 3001;
    pub const QOT_GET_BASIC_QOT: u32 = 3004;
    pub const QOT_UPDATE_BASIC_QOT: u32 = 3005; // 推送
    pub const QOT_REQUEST_HISTORY_KL: u32 = 3103; // 历史K线
}

/// Futu 市场代码（QotMarket 枚举值）
mod futu_market {
    pub const HK: i32 = 1;      // 港股 QotMarket_HK_Security
    pub const US: i32 = 11;     // 美股 QotMarket_US_Security
    pub const CN_SH: i32 = 21;  // A股-沪 QotMarket_CNSH_Security
    pub const CN_SZ: i32 = 22;  // A股-深 QotMarket_CNSZ_Security
    pub const SG: i32 = 13;     // 新加坡 QotMarket_SG_Security
}

// ---- Protobuf 消息定义 (prost derive) ----

/// 通用 Security 类型
#[derive(Clone, PartialEq, Message)]
struct Security {
    #[prost(int32, tag = "1")]
    market: i32,
    #[prost(string, tag = "2")]
    code: String,
}

/// InitConnect
mod pb_init {
    use prost::Message;

    #[derive(Clone, PartialEq, Message)]
    pub struct C2S {
        #[prost(int32, tag = "1")]
        pub client_ver: i32,
        #[prost(string, tag = "2")]
        pub client_id: String,
        #[prost(bool, tag = "3")]
        pub recv_notify: bool,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct S2C {
        #[prost(uint64, tag = "1")]
        pub server_ver: u64,
        #[prost(uint64, tag = "2")]
        pub login_user_id: u64,
        #[prost(uint64, tag = "3")]
        pub conn_id: u64,
        #[prost(string, tag = "4")]
        pub conn_aes_key: String,
        #[prost(int32, tag = "5")]
        pub keep_alive_interval: i32,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct Request {
        #[prost(message, optional, tag = "4")]
        pub c2s: Option<C2S>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct Response {
        #[prost(int32, tag = "1")]
        pub ret_type: i32,
        #[prost(string, optional, tag = "2")]
        pub ret_msg: Option<String>,
        #[prost(int32, optional, tag = "3")]
        pub err_code: Option<i32>,
        #[prost(message, optional, tag = "4")]
        pub s2c: Option<S2C>,
    }
}

/// QotSub (3001)
mod pb_sub {
    use prost::Message;

    #[derive(Clone, PartialEq, Message)]
    pub struct C2S {
        #[prost(message, repeated, tag = "1")]
        pub security_list: Vec<super::Security>,
        #[prost(int32, repeated, tag = "2")]
        pub sub_type_list: Vec<i32>,
        #[prost(bool, tag = "3")]
        pub is_sub_or_un_sub: bool,
        #[prost(bool, tag = "4")]
        pub is_reg_or_un_reg_push: bool,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct Request {
        #[prost(message, optional, tag = "4")]
        pub c2s: Option<C2S>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct Response {
        #[prost(int32, tag = "1")]
        pub ret_type: i32,
        #[prost(string, optional, tag = "2")]
        pub ret_msg: Option<String>,
    }
}

/// QotGetBasicQot (3004) / QotUpdateBasicQot (3005)
mod pb_basic_qot {
    use prost::Message;

    #[derive(Clone, PartialEq, Message)]
    pub struct C2S {
        #[prost(message, repeated, tag = "1")]
        pub security_list: Vec<super::Security>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct BasicQot {
        #[prost(message, optional, tag = "1")]
        pub security: Option<super::Security>,
        #[prost(bool, optional, tag = "2")]
        pub is_suspended: Option<bool>,
        #[prost(string, optional, tag = "3")]
        pub list_time: Option<String>,
        #[prost(double, optional, tag = "4")]
        pub price_spread: Option<f64>, // 价差（tick size，非涨跌额）
        #[prost(string, optional, tag = "5")]
        pub update_time: Option<String>,
        #[prost(double, optional, tag = "6")]
        pub high_price: Option<f64>,
        #[prost(double, optional, tag = "7")]
        pub open_price: Option<f64>,
        #[prost(double, optional, tag = "8")]
        pub low_price: Option<f64>,
        #[prost(double, optional, tag = "9")]
        pub cur_price: Option<f64>,
        #[prost(double, optional, tag = "10")]
        pub last_close_price: Option<f64>,
        #[prost(int64, optional, tag = "11")]
        pub volume: Option<i64>,
        #[prost(double, optional, tag = "12")]
        pub turnover: Option<f64>,
        #[prost(double, optional, tag = "13")]
        pub turnover_rate: Option<f64>,
        #[prost(double, optional, tag = "14")]
        pub amplitude: Option<f64>,
        #[prost(int32, optional, tag = "15")]
        pub dark_status: Option<i32>,
        // field 16: optionExData (skip)
        #[prost(double, optional, tag = "17")]
        pub list_timestamp: Option<f64>,
        #[prost(double, optional, tag = "18")]
        pub update_timestamp: Option<f64>,
        // field 19: preMarket, 20: afterMarket (skip)
        #[prost(int32, optional, tag = "21")]
        pub sec_status: Option<i32>,
        // field 22: futureExData, 23: warrantExData (skip)
        #[prost(string, optional, tag = "24")]
        pub name: Option<String>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct S2C {
        #[prost(message, repeated, tag = "1")]
        pub basic_qot_list: Vec<BasicQot>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct Request {
        #[prost(message, optional, tag = "4")]
        pub c2s: Option<C2S>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct Response {
        #[prost(int32, tag = "1")]
        pub ret_type: i32,
        #[prost(string, optional, tag = "2")]
        pub ret_msg: Option<String>,
        #[prost(int32, optional, tag = "3")]
        pub err_code: Option<i32>,
        #[prost(message, optional, tag = "4")]
        pub s2c: Option<S2C>,
    }
}

// ---- 客户端实现 ----

/// OpenAPI 客户端
pub struct OpenApiClient {
    host: String,
    port: u16,
    stream: Option<TcpStream>,
    serial_no: u32,
    conn_id: u64,
    /// 推送数据接收通道
    quote_tx: Option<mpsc::Sender<QuoteSnapshot>>,
    /// 订阅成功的市场（只对这些市场发起行情请求）
    subscribed_markets: HashSet<Market>,
}

impl OpenApiClient {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            port,
            stream: None,
            serial_no: 0,
            conn_id: 0,
            quote_tx: None,
            subscribed_markets: HashSet::new(),
        }
    }

    /// 设置行情推送通道
    /// 获取已订阅成功的市场集合
    pub fn subscribed_markets(&self) -> &HashSet<Market> {
        &self.subscribed_markets
    }

    pub fn set_quote_channel(&mut self, tx: mpsc::Sender<QuoteSnapshot>) {
        self.quote_tx = Some(tx);
    }

    /// 断开连接（优雅关闭 TCP）
    pub async fn disconnect(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            // 等待服务器完成内部处理后再关闭
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let _ = stream.shutdown().await;
            info!("Disconnected from FutuOpenD");
        }
    }

    /// 连接到 FutuOpenD
    pub async fn connect(&mut self) -> Result<()> {
        let addr = format!("{}:{}", self.host, self.port);
        info!("Connecting to FutuOpenD at {}", addr);

        let stream = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("Failed to connect to FutuOpenD at {}", addr))?;

        self.stream = Some(stream);
        info!("TCP connection established");

        // 发送 InitConnect
        self.init_connect().await?;

        Ok(())
    }

    /// 发送 InitConnect 请求（JSON 模式，FutuOpenD 对此接口支持 JSON）
    async fn init_connect(&mut self) -> Result<()> {
        let body = serde_json::json!({
            "c2s": {
                "clientVer": 100,
                "clientID": "qtrade",
                "recvNotify": true
            }
        });

        let body_bytes = serde_json::to_vec(&body)?;
        self.send_packet_with_fmt(proto_id::INIT_CONNECT, &body_bytes, 1)
            .await?;

        let (_proto_id, response) = self.recv_packet().await?;

        // InitConnect 响应也是 JSON
        let resp: serde_json::Value = serde_json::from_slice(&response)
            .with_context(|| "Failed to parse InitConnect response")?;

        let ret_type = resp.get("retType").and_then(|v| v.as_i64()).unwrap_or(-1);
        if ret_type != 0 {
            let msg = resp.get("retMsg").and_then(|v| v.as_str()).unwrap_or("unknown");
            anyhow::bail!("InitConnect failed: retType={}, msg={}", ret_type, msg);
        }

        if let Some(conn_id) = resp.pointer("/s2c/connID").and_then(|v| v.as_u64()) {
            self.conn_id = conn_id;
            let keep_alive = resp
                .pointer("/s2c/keepAliveInterval")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            info!(
                "InitConnect successful, connID: {}, keepAlive: {}s",
                conn_id, keep_alive
            );
        }

        Ok(())
    }

    /// 订阅行情（按市场分批，避免一个市场失败影响全部）
    /// 订阅成功的市场会记录下来，后续 get_basic_quotes 只查这些市场
    pub async fn subscribe(
        &mut self,
        stocks: &[StockCode],
        sub_types: &[i32],
    ) -> Result<()> {
        let markets = [Market::HK, Market::SH, Market::SZ, Market::US, Market::SG];
        let market_names = ["HK", "SH", "SZ", "US", "SG"];

        // 按市场分组（跳过 Unknown/FX）
        let mut groups: Vec<Vec<&StockCode>> = vec![Vec::new(); 5];
        for s in stocks {
            match s.market {
                Market::HK => groups[0].push(s),
                Market::SH => groups[1].push(s),
                Market::SZ => groups[2].push(s),
                Market::US => groups[3].push(s),
                Market::SG => groups[4].push(s),
                Market::FX | Market::Unknown => {
                    debug!("Skipping unsupported market stock: {}", s.display_code());
                }
            }
        }

        let mut success_count = 0;

        for (i, group) in groups.iter().enumerate() {
            if group.is_empty() {
                continue;
            }
            match self.subscribe_batch(group, sub_types).await {
                Ok(()) => {
                    self.subscribed_markets.insert(markets[i]);
                    info!("Subscribed {} {} stocks", group.len(), market_names[i]);
                    success_count += group.len();
                }
                Err(e) => {
                    warn!("{} market unavailable: {}", market_names[i], e);
                }
            }
        }

        if success_count == 0 {
            anyhow::bail!("All subscription groups failed");
        }
        info!(
            "Total subscribed: {} stocks (markets: {:?})",
            success_count,
            self.subscribed_markets
        );
        Ok(())
    }

    /// 订阅单批次行情
    async fn subscribe_batch(
        &mut self,
        stocks: &[&StockCode],
        sub_types: &[i32],
    ) -> Result<()> {
        let security_list: Vec<serde_json::Value> = stocks
            .iter()
            .map(|s| {
                serde_json::json!({
                    "market": stock_code_to_futu_market(s),
                    "code": &s.code
                })
            })
            .collect();

        let body = serde_json::json!({
            "c2s": {
                "securityList": security_list,
                "subTypeList": sub_types,
                "isSubOrUnSub": true,
                "isRegOrUnRegPush": true
            }
        });

        let body_bytes = serde_json::to_vec(&body)?;
        self.send_packet_with_fmt(proto_id::QOT_SUB, &body_bytes, 1)
            .await?;

        let response = self.recv_response(proto_id::QOT_SUB).await?;
        // 先尝试 protobuf
        if let Ok(resp) = pb_sub::Response::decode(response.as_slice()) {
            if resp.ret_type != 0 {
                anyhow::bail!(
                    "QotSub failed: {}",
                    resp.ret_msg.as_deref().unwrap_or("unknown")
                );
            }
            return Ok(());
        }
        // 尝试 JSON 错误响应
        if let Ok(json_resp) = serde_json::from_slice::<serde_json::Value>(&response) {
            let ret_type = json_resp.get("retType").and_then(|v| v.as_i64()).unwrap_or(-1);
            let ret_msg = json_resp.get("retMsg").and_then(|v| v.as_str()).unwrap_or("unknown");
            if ret_type != 0 {
                anyhow::bail!("QotSub error: {}", ret_msg);
            }
        }

        Ok(())
    }

    /// 退订行情
    pub async fn unsubscribe(
        &mut self,
        stocks: &[StockCode],
        sub_types: &[i32],
    ) -> Result<()> {
        // 按市场分组
        let mut groups: Vec<Vec<&StockCode>> = vec![Vec::new(); 5];
        for s in stocks {
            match s.market {
                Market::HK => groups[0].push(s),
                Market::SH => groups[1].push(s),
                Market::SZ => groups[2].push(s),
                Market::US => groups[3].push(s),
                Market::SG => groups[4].push(s),
                Market::FX | Market::Unknown => {}
            }
        }

        for group in &groups {
            if group.is_empty() {
                continue;
            }
            if let Err(e) = self.unsubscribe_batch(group, sub_types).await {
                warn!("Unsubscribe batch failed: {}", e);
            }
        }

        Ok(())
    }

    /// 退订单批次行情
    async fn unsubscribe_batch(
        &mut self,
        stocks: &[&StockCode],
        sub_types: &[i32],
    ) -> Result<()> {
        let security_list: Vec<serde_json::Value> = stocks
            .iter()
            .map(|s| {
                serde_json::json!({
                    "market": stock_code_to_futu_market(s),
                    "code": &s.code
                })
            })
            .collect();

        let body = serde_json::json!({
            "c2s": {
                "securityList": security_list,
                "subTypeList": sub_types,
                "isSubOrUnSub": false,
                "isRegOrUnRegPush": false
            }
        });

        let body_bytes = serde_json::to_vec(&body)?;
        self.send_packet_with_fmt(proto_id::QOT_SUB, &body_bytes, 1)
            .await?;

        let response = self.recv_response(proto_id::QOT_SUB).await?;
        if let Ok(resp) = pb_sub::Response::decode(response.as_slice()) {
            if resp.ret_type != 0 {
                anyhow::bail!(
                    "QotUnsub failed: {}",
                    resp.ret_msg.as_deref().unwrap_or("unknown")
                );
            }
            return Ok(());
        }
        if let Ok(json_resp) = serde_json::from_slice::<serde_json::Value>(&response) {
            let ret_type = json_resp.get("retType").and_then(|v| v.as_i64()).unwrap_or(-1);
            let ret_msg = json_resp.get("retMsg").and_then(|v| v.as_str()).unwrap_or("unknown");
            if ret_type != 0 {
                anyhow::bail!("QotUnsub error: {}", ret_msg);
            }
        }

        Ok(())
    }

    /// 获取基本行情（只查询已订阅成功的市场）
    pub async fn get_basic_quotes(
        &mut self,
        stocks: &[StockCode],
    ) -> Result<Vec<QuoteSnapshot>> {
        let markets = [Market::HK, Market::SH, Market::SZ, Market::US, Market::SG];
        let market_names = ["HK", "SH", "SZ", "US", "SG"];

        // 按市场分组，只保留已订阅的市场
        let mut groups: Vec<Vec<&StockCode>> = vec![Vec::new(); 5];
        for s in stocks {
            let idx = match s.market {
                Market::HK => Some(0),
                Market::SH => Some(1),
                Market::SZ => Some(2),
                Market::US => Some(3),
                Market::SG => Some(4),
                Market::FX | Market::Unknown => None,
            };
            if let Some(i) = idx {
                if self.subscribed_markets.contains(&markets[i]) {
                    groups[i].push(s);
                }
            }
        }

        let mut all_quotes = Vec::new();

        for (i, group) in groups.iter().enumerate() {
            if group.is_empty() {
                continue;
            }
            match self.get_basic_quotes_batch(group).await {
                Ok(quotes) => {
                    debug!("Got {} quotes for {} market", quotes.len(), market_names[i]);
                    all_quotes.extend(quotes);
                }
                Err(e) => {
                    warn!("Failed to fetch {} quotes: {}", market_names[i], e);
                }
            }
        }

        Ok(all_quotes)
    }

    /// 获取单批次基本行情
    async fn get_basic_quotes_batch(
        &mut self,
        stocks: &[&StockCode],
    ) -> Result<Vec<QuoteSnapshot>> {
        let security_list: Vec<serde_json::Value> = stocks
            .iter()
            .map(|s| {
                serde_json::json!({
                    "market": stock_code_to_futu_market(s),
                    "code": &s.code
                })
            })
            .collect();

        let body = serde_json::json!({
            "c2s": {
                "securityList": security_list
            }
        });

        let body_bytes = serde_json::to_vec(&body)?;
        self.send_packet_with_fmt(proto_id::QOT_GET_BASIC_QOT, &body_bytes, 1)
            .await?;

        let response = self.recv_response(proto_id::QOT_GET_BASIC_QOT).await?;

        // 尝试 JSON 解码（FutuOpenD 对 JSON 请求通常返回 JSON）
        if let Ok(json_resp) = serde_json::from_slice::<serde_json::Value>(&response) {
            let ret_type = json_resp.get("retType").and_then(|v| v.as_i64()).unwrap_or(-1);
            if ret_type != 0 {
                let ret_msg = json_resp.get("retMsg").and_then(|v| v.as_str()).unwrap_or("unknown");
                anyhow::bail!("QotGetBasicQot error: {}", ret_msg);
            }
            return Ok(parse_basic_qot_json(&json_resp));
        }

        // JSON 失败，尝试 protobuf
        if let Ok(resp) = pb_basic_qot::Response::decode(response.as_slice()) {
            if resp.ret_type != 0 {
                anyhow::bail!(
                    "QotGetBasicQot failed: {}",
                    resp.ret_msg.as_deref().unwrap_or("unknown")
                );
            }
            return Ok(parse_basic_qot_list(resp.s2c.as_ref()));
        }

        // 两种格式都失败
        anyhow::bail!(
            "Failed to decode QotGetBasicQot response ({} bytes)",
            response.len()
        )
    }

    /// 请求单只股票的历史日K线
    pub async fn request_history_kline(
        &mut self,
        stock: &StockCode,
        begin: &str,
        end: &str,
        max_count: u32,
    ) -> Result<Vec<DailyKline>> {
        let body = serde_json::json!({
            "c2s": {
                "security": {
                    "market": stock_code_to_futu_market(stock),
                    "code": &stock.code
                },
                "klType": 2,
                "rehabType": 1,
                "beginTime": begin,
                "endTime": end,
                "maxCount": max_count,
                "needKLFieldsFlag": 127
            }
        });

        let body_bytes = serde_json::to_vec(&body)?;
        self.send_packet_with_fmt(proto_id::QOT_REQUEST_HISTORY_KL, &body_bytes, 1)
            .await?;

        let response = self
            .recv_response(proto_id::QOT_REQUEST_HISTORY_KL)
            .await?;

        // 尝试 JSON 解码
        if let Ok(json_resp) = serde_json::from_slice::<serde_json::Value>(&response) {
            let ret_type = json_resp
                .get("retType")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            if ret_type != 0 {
                let ret_msg = json_resp
                    .get("retMsg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                anyhow::bail!("QotRequestHistoryKL error: {}", ret_msg);
            }
            return Ok(parse_kline_json(&json_resp));
        }

        anyhow::bail!(
            "Failed to decode QotRequestHistoryKL response ({} bytes)",
            response.len()
        )
    }

    /// 批量请求多只股票的历史日K线（顺序请求，每次间隔 200ms 防限流）
    pub async fn request_history_kline_batch(
        &mut self,
        stocks: &[StockCode],
        begin: &str,
        end: &str,
        max_count: u32,
    ) -> Result<std::collections::HashMap<StockCode, Vec<DailyKline>>> {
        let mut result = std::collections::HashMap::new();

        for (i, stock) in stocks.iter().enumerate() {
            match self
                .request_history_kline(stock, begin, end, max_count)
                .await
            {
                Ok(klines) => {
                    debug!(
                        "Got {} daily klines for {}",
                        klines.len(),
                        stock.display_code()
                    );
                    result.insert(stock.clone(), klines);
                }
                Err(e) => {
                    warn!(
                        "Failed to get klines for {}: {}",
                        stock.display_code(),
                        e
                    );
                }
            }

            // 间隔 200ms 防限流（最后一个不需要等）
            if i + 1 < stocks.len() {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }

        Ok(result)
    }

    /// 批量请求历史日K线，带进度回调更新到仪表盘
    pub async fn request_history_kline_batch_with_progress(
        &mut self,
        stocks: &[StockCode],
        begin: &str,
        end: &str,
        max_count: u32,
        dash_state: &std::sync::Arc<tokio::sync::Mutex<crate::ui::dashboard::DashboardState>>,
    ) -> Result<std::collections::HashMap<StockCode, Vec<DailyKline>>> {
        let mut result = std::collections::HashMap::new();
        let total = stocks.len();

        for (i, stock) in stocks.iter().enumerate() {
            match self
                .request_history_kline(stock, begin, end, max_count)
                .await
            {
                Ok(klines) => {
                    info!(
                        "Got {} daily klines for {}",
                        klines.len(),
                        stock.display_code()
                    );
                    result.insert(stock.clone(), klines);
                }
                Err(e) => {
                    warn!(
                        "Failed to get klines for {}: {}",
                        stock.display_code(),
                        e
                    );
                }
            }

            // 更新进度
            {
                let mut state = dash_state.lock().await;
                state.daily_kline_status =
                    format!("日K获取中({}/{})", i + 1, total);
            }

            // 间隔 200ms 防限流（最后一个不需要等）
            if i + 1 < total {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }

        Ok(result)
    }

    /// 启动推送接收循环
    pub async fn start_push_loop(mut self) -> Result<()> {
        info!("Starting push receive loop");

        loop {
            match self.recv_packet().await {
                Ok((pid, data)) => {
                    if pid == proto_id::QOT_UPDATE_BASIC_QOT {
                        if let Ok(resp) = pb_basic_qot::Response::decode(data.as_slice()) {
                            let quotes = parse_basic_qot_list(resp.s2c.as_ref());
                            if let Some(tx) = &self.quote_tx {
                                for quote in quotes {
                                    if tx.send(quote).await.is_err() {
                                        warn!("Quote channel closed");
                                        return Ok(());
                                    }
                                }
                            }
                        }
                    } else if pid == proto_id::KEEP_ALIVE {
                        debug!("KeepAlive received");
                    }
                }
                Err(e) => {
                    error!("Push loop error: {}", e);
                    return Err(e);
                }
            }
        }
    }

    /// 发送 protobuf 数据包
    async fn send_proto_packet<M: Message>(&mut self, proto_id: u32, msg: &M) -> Result<()> {
        let body = msg.encode_to_vec();
        self.send_packet(proto_id, &body).await
    }

    /// 发送原始数据包（默认 protobuf 格式）
    async fn send_packet(&mut self, proto_id: u32, body: &[u8]) -> Result<()> {
        self.send_packet_with_fmt(proto_id, body, 0).await
    }

    /// 发送数据包，指定格式类型（0=protobuf, 1=json）
    async fn send_packet_with_fmt(
        &mut self,
        proto_id: u32,
        body: &[u8],
        fmt: u8,
    ) -> Result<()> {
        let stream = self
            .stream
            .as_mut()
            .context("Not connected")?;

        self.serial_no += 1;
        let header = build_header(proto_id, self.serial_no, body, fmt);

        stream.write_all(&header).await?;
        stream.write_all(body).await?;
        stream.flush().await?;

        debug!(
            "Sent packet: proto_id={}, serial={}, body_len={}, fmt={}",
            proto_id, self.serial_no, body.len(), fmt
        );

        Ok(())
    }

    /// 接收数据包
    async fn recv_packet(&mut self) -> Result<(u32, Vec<u8>)> {
        let stream = self
            .stream
            .as_mut()
            .context("Not connected")?;

        // 读取头部
        let mut header_buf = [0u8; HEADER_SIZE];
        stream.read_exact(&mut header_buf).await?;

        let (proto_id, body_len) = parse_header(&header_buf)?;

        // 读取 body
        let mut body = vec![0u8; body_len as usize];
        if body_len > 0 {
            stream.read_exact(&mut body).await?;
        }

        debug!(
            "Received packet: proto_id={}, body_len={}",
            proto_id, body_len
        );

        Ok((proto_id, body))
    }

    /// 接收指定 proto_id 的响应，跳过推送和心跳包
    async fn recv_response(&mut self, expected_pid: u32) -> Result<Vec<u8>> {
        loop {
            let (pid, data) = self.recv_packet().await?;
            if pid == expected_pid {
                return Ok(data);
            }
            if pid == proto_id::QOT_UPDATE_BASIC_QOT {
                debug!("Skipping push update while waiting for response {}", expected_pid);
            } else if pid == proto_id::KEEP_ALIVE {
                debug!("Skipping keepalive while waiting for response {}", expected_pid);
            } else {
                debug!("Skipping unexpected packet proto_id={} while waiting for {}", pid, expected_pid);
            }
        }
    }
}

/// 构建 Futu 协议头部（44 字节）
fn build_header(proto_id: u32, serial_no: u32, body: &[u8], fmt: u8) -> Vec<u8> {
    use sha1::{Sha1, Digest};

    let mut buf = BytesMut::with_capacity(HEADER_SIZE);

    // 计算 body 的 SHA1
    let mut hasher = Sha1::new();
    hasher.update(body);
    let sha1_hash: [u8; 20] = hasher.finalize().into();

    buf.put_slice(&FUTU_MAGIC);            // 0-1: magic "FT"
    buf.put_u32_le(proto_id);              // 2-5: proto_id
    buf.put_u8(fmt);                       // 6: proto_fmt_type (0=protobuf, 1=json)
    buf.put_u8(PROTO_VERSION);             // 7: proto_ver
    buf.put_u32_le(serial_no);             // 8-11: serial_no
    buf.put_u32_le(body.len() as u32);     // 12-15: body_len
    buf.put_slice(&sha1_hash);             // 16-35: sha1
    buf.put_slice(&[0u8; 8]);              // 36-43: reserved

    buf.to_vec()
}

/// 解析 Futu 协议头部
fn parse_header(buf: &[u8]) -> Result<(u32, u32)> {
    if buf.len() < HEADER_SIZE {
        anyhow::bail!("Header too short: {} bytes", buf.len());
    }

    // 验证 magic
    if buf[0] != FUTU_MAGIC[0] || buf[1] != FUTU_MAGIC[1] {
        anyhow::bail!("Invalid magic bytes: {:02x}{:02x}", buf[0], buf[1]);
    }

    let mut cursor = &buf[2..];
    let proto_id = cursor.get_u32_le();
    let _proto_fmt = cursor.get_u8();
    let _proto_ver = cursor.get_u8();
    let _serial_no = cursor.get_u32_le();
    let body_len = cursor.get_u32_le();

    Ok((proto_id, body_len))
}

/// 从 protobuf BasicQot 列表构建 QuoteSnapshot
fn parse_basic_qot_list(s2c: Option<&pb_basic_qot::S2C>) -> Vec<QuoteSnapshot> {
    let Some(s2c) = s2c else {
        return Vec::new();
    };

    s2c.basic_qot_list
        .iter()
        .map(|qot| {
            let (market, code) = qot
                .security
                .as_ref()
                .map(|s| (s.market, s.code.as_str()))
                .unwrap_or((0, ""));
            let stock_code = futu_market_to_stock_code(market, code);

            let cur_price = qot.cur_price.unwrap_or(0.0);
            let last_close = qot.last_close_price.unwrap_or(0.0);
            let change = cur_price - last_close;
            let change_pct = if last_close > 0.0 {
                change / last_close * 100.0
            } else {
                0.0
            };

            QuoteSnapshot {
                code: stock_code,
                name: qot.name.clone().unwrap_or_default(),
                last_price: cur_price,
                prev_close: last_close,
                open_price: qot.open_price.unwrap_or(0.0),
                high_price: qot.high_price.unwrap_or(0.0),
                low_price: qot.low_price.unwrap_or(0.0),
                volume: qot.volume.unwrap_or(0) as u64,
                turnover: qot.turnover.unwrap_or(0.0),
                change,
                change_pct,
                turnover_rate: qot.turnover_rate.unwrap_or(0.0),
                amplitude: qot.amplitude.unwrap_or(0.0),
                extended_price: None,
                extended_change_pct: None,
                timestamp: chrono::Local::now(),
                source: DataSource::OpenApi,
            }
        })
        .collect()
}

/// 从 JSON 值中提取整数（兼容数字和字符串格式）
/// FutuOpenD 对大数值（如 volume）可能返回字符串而非数字
fn json_as_i64(v: &serde_json::Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// 从 JSON 值中提取浮点数（兼容数字和字符串格式）
fn json_as_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// 从 JSON 响应解析 BasicQot 列表
fn parse_basic_qot_json(resp: &serde_json::Value) -> Vec<QuoteSnapshot> {
    let Some(list) = resp.pointer("/s2c/basicQotList").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    list.iter()
        .filter_map(|qot| {
            let security = qot.get("security")?;
            let market = security.get("market").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let code = security.get("code").and_then(|v| v.as_str()).unwrap_or("");
            let stock_code = futu_market_to_stock_code(market, code);

            let cur_price = qot.get("curPrice").and_then(json_as_f64).unwrap_or(0.0);
            let last_close = qot.get("lastClosePrice").and_then(json_as_f64).unwrap_or(0.0);
            let change = cur_price - last_close;
            let change_pct = if last_close > 0.0 {
                change / last_close * 100.0
            } else {
                0.0
            };

            Some(QuoteSnapshot {
                code: stock_code,
                name: qot.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                last_price: cur_price,
                prev_close: last_close,
                open_price: qot.get("openPrice").and_then(json_as_f64).unwrap_or(0.0),
                high_price: qot.get("highPrice").and_then(json_as_f64).unwrap_or(0.0),
                low_price: qot.get("lowPrice").and_then(json_as_f64).unwrap_or(0.0),
                volume: qot.get("volume").and_then(json_as_i64).unwrap_or(0) as u64,
                turnover: qot.get("turnover").and_then(json_as_f64).unwrap_or(0.0),
                change,
                change_pct,
                turnover_rate: qot.get("turnoverRate").and_then(json_as_f64).unwrap_or(0.0),
                amplitude: qot.get("amplitude").and_then(json_as_f64).unwrap_or(0.0),
                extended_price: None,
                extended_change_pct: None,
                timestamp: chrono::Local::now(),
                source: DataSource::OpenApi,
            })
        })
        .collect()
}

/// StockCode → protobuf Security
fn stock_code_to_security(code: &StockCode) -> Security {
    Security {
        market: stock_code_to_futu_market(code),
        code: code.code.clone(),
    }
}

/// StockCode → Futu 市场代码
fn stock_code_to_futu_market(code: &StockCode) -> i32 {
    match code.market {
        Market::HK => futu_market::HK,
        Market::US => futu_market::US,
        Market::SH => futu_market::CN_SH,
        Market::SZ => futu_market::CN_SZ,
        Market::SG => futu_market::SG,
        Market::FX => futu_market::HK, // FX 暂无独立市场码，降级到 HK
        Market::Unknown => futu_market::HK,
    }
}

/// 从 JSON 响应解析历史K线
fn parse_kline_json(resp: &serde_json::Value) -> Vec<DailyKline> {
    let Some(list) = resp.pointer("/s2c/klList").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    list.iter()
        .filter_map(|kl| {
            Some(DailyKline {
                open: kl.get("openPrice").and_then(|v| v.as_f64()).unwrap_or(0.0),
                close: kl.get("closePrice").and_then(|v| v.as_f64()).unwrap_or(0.0),
                high: kl.get("highPrice").and_then(|v| v.as_f64()).unwrap_or(0.0),
                low: kl.get("lowPrice").and_then(|v| v.as_f64()).unwrap_or(0.0),
                volume: kl.get("volume").and_then(|v| v.as_i64()).unwrap_or(0) as u64,
                turnover: kl.get("turnover").and_then(|v| v.as_f64()).unwrap_or(0.0),
                date: kl
                    .get("time")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

/// Futu 市场代码 → StockCode
fn futu_market_to_stock_code(market: i32, code: &str) -> StockCode {
    let m = match market {
        1 => Market::HK,
        11 => Market::US,
        21 => Market::SH,
        22 => Market::SZ,
        13 => Market::SG,
        _ => Market::Unknown,
    };
    StockCode::new(m, code)
}
