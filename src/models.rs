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
            timestamp: Local::now(),
            source: DataSource::Cache,
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
    /// 本地缓存（plist 等）
    Cache,
}

impl fmt::Display for DataSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataSource::Accessibility => write!(f, "AX"),
            DataSource::OpenApi => write!(f, "OpenAPI"),
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
    VolumeSpike { ratio: f64 },
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
            Signal::VolumeSpike { ratio } => {
                write!(f, "放量({:.1}x)", ratio)
            }
        }
    }
}

/// K线时间周期
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeframe {
    /// Tick 级别（实时）
    Tick,
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
            Timeframe::Tick => write!(f, "{}", self.signal),
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
}

/// 提醒级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}
