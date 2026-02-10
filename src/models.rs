use chrono::{DateTime, Local};
use std::fmt;

/// 市场类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Market {
    /// 港股
    HK,
    /// 沪市 A 股
    SH,
    /// 深市 A 股
    SZ,
    /// 美股
    US,
    /// 新加坡
    SG,
    /// 外汇
    FX,
    /// 未知
    Unknown,
}

impl fmt::Display for Market {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Market::HK => write!(f, "HK"),
            Market::SH => write!(f, "SH"),
            Market::SZ => write!(f, "SZ"),
            Market::US => write!(f, "US"),
            Market::SG => write!(f, "SG"),
            Market::FX => write!(f, "FX"),
            Market::Unknown => write!(f, "??"),
        }
    }
}

/// 股票代码
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct StockCode {
    /// 市场
    pub market: Market,
    /// 代码（纯数字部分）
    pub code: String,
}

impl StockCode {
    pub fn new(market: Market, code: impl Into<String>) -> Self {
        Self {
            market,
            code: code.into(),
        }
    }

    /// 从富途 plist 中的数字 ID 解码
    /// 1XXXXXX = 沪市, 2XXXXXX = 深市, 800XXX / 其他 = 港股
    pub fn from_futu_id(id: u64) -> Self {
        if id >= 1_000_000 && id < 2_000_000 {
            // 沪市：去掉前缀 1
            let code = format!("{:06}", id - 1_000_000);
            Self::new(Market::SH, code)
        } else if id >= 2_000_000 && id < 3_000_000 {
            // 深市：去掉前缀 2
            let code = format!("{:06}", id - 2_000_000);
            Self::new(Market::SZ, code)
        } else {
            // 港股
            let code = format!("{:05}", id);
            Self::new(Market::HK, code)
        }
    }

    /// 返回用于显示的完整代码，如 "HK.00700"
    pub fn display_code(&self) -> String {
        format!("{}.{}", self.market, self.code)
    }

    /// 是否为指数代码（指数的 VWAP/换手率等指标无意义）
    pub fn is_index(&self) -> bool {
        match self.market {
            Market::SH => self.code.starts_with("000"),  // 上证指数系列
            Market::SZ => self.code.starts_with("399"),  // 深证指数系列
            Market::HK => self.code.starts_with("800"),  // 港股指数
            Market::US => self.code.starts_with('.'),     // 美股指数
            _ => false,
        }
    }
}

impl fmt::Display for StockCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.market, self.code)
    }
}

/// 实时行情快照
#[derive(Debug, Clone, serde::Serialize)]
pub struct QuoteSnapshot {
    /// 股票代码
    pub code: StockCode,
    /// 股票名称
    pub name: String,
    /// 最新价
    pub last_price: f64,
    /// 昨收价
    pub prev_close: f64,
    /// 开盘价
    pub open_price: f64,
    /// 最高价
    pub high_price: f64,
    /// 最低价
    pub low_price: f64,
    /// 成交量
    pub volume: u64,
    /// 成交额
    pub turnover: f64,
    /// 涨跌额
    pub change: f64,
    /// 涨跌幅 (%)
    pub change_pct: f64,
    /// 换手率 (%)
    pub turnover_rate: f64,
    /// 振幅 (%)
    pub amplitude: f64,
    /// 盘前/盘后价格（美股）
    pub extended_price: Option<f64>,
    /// 盘前/盘后涨跌幅 (%)（美股）
    pub extended_change_pct: Option<f64>,
    /// 数据时间戳
    pub timestamp: DateTime<Local>,
    /// 数据源
    pub source: DataSource,
}

impl QuoteSnapshot {
    /// 创建一个空快照（仅含代码和名称）
    pub fn empty(code: StockCode, name: String) -> Self {
        Self {
            code,
            name,
            last_price: 0.0,
            prev_close: 0.0,
            open_price: 0.0,
            high_price: 0.0,
            low_price: 0.0,
            volume: 0,
            turnover: 0.0,
            change: 0.0,
            change_pct: 0.0,
            turnover_rate: 0.0,
            amplitude: 0.0,
            extended_price: None,
            extended_change_pct: None,
            timestamp: Local::now(),
            source: DataSource::Cache,
        }
    }
}

/// 美股交易时段
///
/// 四个时段连续循环（美东 ET 时间固定，DST 由 chrono-tz 自动处理）：
/// - 盘前 04:00–09:30 ET
/// - 盘中 09:30–16:00 ET
/// - 盘后 16:00–20:00 ET
/// - 夜盘 20:00–04:00 ET（跨午夜）
///
/// 详见 docs/US_MARKET_SESSIONS.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsMarketSession {
    /// 盘前 (Pre-market) 04:00–09:30 ET
    PreMarket,
    /// 盘中 (Regular) 09:30–16:00 ET
    Regular,
    /// 盘后 (After-hours) 16:00–20:00 ET
    AfterHours,
    /// 夜盘 (Overnight) 20:00–04:00 ET
    Overnight,
    /// 休市（周末）
    Closed,
}

impl fmt::Display for UsMarketSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsMarketSession::PreMarket => write!(f, "盘前"),
            UsMarketSession::Regular => write!(f, "盘中"),
            UsMarketSession::AfterHours => write!(f, "盘后"),
            UsMarketSession::Overnight => write!(f, "夜盘"),
            UsMarketSession::Closed => write!(f, "休市"),
        }
    }
}

impl UsMarketSession {
    /// 当前时段灰色小字（扩展价格）的标签
    ///
    /// - 盘前/盘中/休市 → "盘前"（盘中时灰色显示盘前价，休市时灰色可能是过时的盘前价）
    /// - 盘后 → "盘后"
    /// - 夜盘 → "夜盘"
    pub fn extended_label(&self) -> &'static str {
        match self {
            UsMarketSession::PreMarket
            | UsMarketSession::Regular
            | UsMarketSession::Closed => "盘前",
            UsMarketSession::AfterHours => "盘后",
            UsMarketSession::Overnight => "夜盘",
        }
    }
}

/// 获取当前美股交易时段
///
/// 使用 `chrono-tz` 转换为美东时间 (America/New_York)，自动处理夏/冬令时。
/// 仅需比较 ET 本地时间与四个分界点：04:00 / 09:30 / 16:00 / 20:00。
///
/// 周末休市：周六 04:00 ET – 周日 20:00 ET
pub fn us_market_session() -> UsMarketSession {
    use chrono::{Datelike, Timelike, Utc};
    use chrono_tz::America::New_York;

    let now = Utc::now().with_timezone(&New_York);
    let weekday = now.weekday();
    let hhmm = now.hour() * 100 + now.minute();

    match weekday {
        chrono::Weekday::Sat => {
            // 周六 00:00–04:00 ET: 周五夜盘延续
            // 周六 04:00 之后: 休市
            if hhmm < 400 {
                UsMarketSession::Overnight
            } else {
                UsMarketSession::Closed
            }
        }
        chrono::Weekday::Sun => {
            // 周日 20:00 起: 夜盘（属于下周一交易日）
            // 周日 20:00 之前: 休市
            if hhmm >= 2000 {
                UsMarketSession::Overnight
            } else {
                UsMarketSession::Closed
            }
        }
        _ => {
            // 周一至周五
            if hhmm < 400 {
                UsMarketSession::Overnight
            } else if hhmm < 930 {
                UsMarketSession::PreMarket
            } else if hhmm < 1600 {
                UsMarketSession::Regular
            } else if hhmm < 2000 {
                UsMarketSession::AfterHours
            } else {
                UsMarketSession::Overnight
            }
        }
    }
}

/// 数据源类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DataSource {
    /// macOS Accessibility API
    Accessibility,
    /// FutuOpenD OpenAPI
    OpenApi,
    /// 窗口截图 + Vision OCR
    Ocr,
    /// 本地缓存（plist 等）
    Cache,
}

impl fmt::Display for DataSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataSource::Accessibility => write!(f, "AX"),
            DataSource::OpenApi => write!(f, "OpenAPI"),
            DataSource::Ocr => write!(f, "OCR"),
            DataSource::Cache => write!(f, "Cache"),
        }
    }
}

/// 自选股条目（从 plist 读取）
#[derive(Debug, Clone)]
pub struct WatchlistEntry {
    /// 股票代码
    pub code: StockCode,
    /// 富途内部 ID（用于关联 StockDB 名称）
    pub stock_id: u64,
    /// 股票名称
    pub name: String,
    /// 缓存价格（从 plist 读取的高精度整数转换）
    pub cached_price: Option<f64>,
    /// 在 plist 中的排序位置
    pub sort_index: usize,
}

/// 信号情绪方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sentiment {
    Bullish,  // 利多
    Bearish,  // 利空
    Neutral,  // 中性
}

impl fmt::Display for Sentiment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Sentiment::Bullish => write!(f, "利多"),
            Sentiment::Bearish => write!(f, "利空"),
            Sentiment::Neutral => write!(f, "中性"),
        }
    }
}

/// 技术指标值
#[derive(Debug, Clone, Default)]
pub struct TechnicalIndicators {
    /// 移动平均线
    pub ma5: Option<f64>,
    pub ma10: Option<f64>,
    pub ma20: Option<f64>,
    pub ma60: Option<f64>,

    /// MACD
    pub macd_dif: Option<f64>,
    pub macd_dea: Option<f64>,
    pub macd_histogram: Option<f64>,

    /// RSI
    pub rsi6: Option<f64>,
    pub rsi12: Option<f64>,
    pub rsi24: Option<f64>,
}

/// 交易信号
#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    /// MA 金叉
    MaGoldenCross { short: usize, long: usize },
    /// MA 死叉
    MaDeathCross { short: usize, long: usize },
    /// MACD 金叉
    MacdGoldenCross,
    /// MACD 死叉
    MacdDeathCross,
    /// RSI 超买
    RsiOverbought { period: usize, value: f64 },
    /// RSI 超卖
    RsiOversold { period: usize, value: f64 },
    /// 放量（成交量突增）
    VolumeSpike { ratio: f64, price: f64, delta: u64 },
    /// VWAP 偏离（正=高于VWAP利多，负=低于VWAP利空）
    VwapDeviation { deviation_pct: f64 },
    /// 急涨急跌（正=急涨，负=急跌）
    RapidMove { change_pct: f64 },
    /// 振幅突破
    AmplitudeBreakout { amplitude_pct: f64 },
    /// MS-MACD 买入（空头区域动能衰减）
    MsMacdBuy,
    /// MS-MACD 卖出（多头区域动能衰减）
    MsMacdSell,
}

impl Signal {
    /// 返回信号的情绪方向
    pub fn sentiment(&self) -> Sentiment {
        match self {
            Signal::MaGoldenCross { .. } => Sentiment::Bullish,
            Signal::MaDeathCross { .. } => Sentiment::Bearish,
            Signal::MacdGoldenCross => Sentiment::Bullish,
            Signal::MacdDeathCross => Sentiment::Bearish,
            Signal::RsiOverbought { .. } => Sentiment::Bearish,
            Signal::RsiOversold { .. } => Sentiment::Bullish,
            Signal::VolumeSpike { .. } => Sentiment::Neutral,
            Signal::VwapDeviation { deviation_pct } => {
                if *deviation_pct > 0.0 { Sentiment::Bullish } else { Sentiment::Bearish }
            }
            Signal::RapidMove { change_pct } => {
                if *change_pct > 0.0 { Sentiment::Bullish } else { Sentiment::Bearish }
            }
            Signal::AmplitudeBreakout { .. } => Sentiment::Neutral,
            Signal::MsMacdBuy => Sentiment::Bullish,
            Signal::MsMacdSell => Sentiment::Bearish,
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Signal::MaGoldenCross { short, long } => {
                write!(f, "MA{}/{} 金叉", short, long)
            }
            Signal::MaDeathCross { short, long } => {
                write!(f, "MA{}/{} 死叉", short, long)
            }
            Signal::MacdGoldenCross => write!(f, "MACD 金叉"),
            Signal::MacdDeathCross => write!(f, "MACD 死叉"),
            Signal::RsiOverbought { period, value } => {
                write!(f, "RSI{} 超买({:.1})", period, value)
            }
            Signal::RsiOversold { period, value } => {
                write!(f, "RSI{} 超卖({:.1})", period, value)
            }
            Signal::VolumeSpike { ratio, .. } => {
                write!(f, "放量({:.0}x)", ratio)
            }
            Signal::VwapDeviation { deviation_pct } => {
                write!(f, "VWAP偏离{:+.1}%", deviation_pct)
            }
            Signal::RapidMove { change_pct } => {
                if *change_pct > 0.0 {
                    write!(f, "急涨{:+.1}%", change_pct)
                } else {
                    write!(f, "急跌{:+.1}%", change_pct)
                }
            }
            Signal::AmplitudeBreakout { amplitude_pct } => {
                write!(f, "振幅突破{:.1}%", amplitude_pct)
            }
            Signal::MsMacdBuy => write!(f, "MS-MACD 买入"),
            Signal::MsMacdSell => write!(f, "MS-MACD 卖出"),
        }
    }
}

/// K线时间周期
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeframe {
    /// 日线级别
    Daily,
}

/// 日K线数据
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DailyKline {
    pub open: f64,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub volume: u64,
    pub turnover: f64,
    pub date: String,
}

/// 带时间周期标签的信号
#[derive(Debug, Clone, PartialEq)]
pub struct TimedSignal {
    pub signal: Signal,
    pub timeframe: Timeframe,
}

impl fmt::Display for TimedSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.timeframe {
            Timeframe::Daily => write!(f, "[日]{}", self.signal),
        }
    }
}

/// 提醒事件
#[derive(Debug, Clone)]
pub struct AlertEvent {
    /// 股票代码
    pub code: StockCode,
    /// 股票名称
    pub name: String,
    /// 触发的规则名称
    pub rule_name: String,
    /// 描述
    pub message: String,
    /// 触发时间
    pub triggered_at: DateTime<Local>,
    /// 严重级别
    pub severity: AlertSeverity,
    /// 情绪方向（用于 UI 着色）
    pub sentiment: Option<Sentiment>,
}

/// 提醒级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_index() {
        assert!(StockCode::new(Market::SH, "000001").is_index()); // 上证指数
        assert!(StockCode::new(Market::SZ, "399006").is_index()); // 创业板指
        assert!(StockCode::new(Market::HK, "800000").is_index()); // 恒生指数
        assert!(StockCode::new(Market::US, ".DJI").is_index());   // 道琼斯
        assert!(!StockCode::new(Market::SZ, "000001").is_index()); // 平安银行
        assert!(!StockCode::new(Market::HK, "00700").is_index());  // 腾讯
        assert!(!StockCode::new(Market::US, "AAPL").is_index());   // 苹果
    }
}
