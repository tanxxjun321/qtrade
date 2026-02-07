//! 日K线分析引擎
//!
//! 基于历史日K线数据计算技术指标和信号，支持 JSON 缓存

use std::collections::HashMap;
use std::path::PathBuf;

use tracing::{info, warn};

use crate::models::{DailyKline, StockCode, TechnicalIndicators, Timeframe, TimedSignal};

use super::indicators;
use super::signals;

/// 缓存文件结构
#[derive(serde::Serialize, serde::Deserialize)]
struct KlineCache {
    /// 最后更新日期 (YYYY-MM-DD)
    last_updated: String,
    /// 每只股票的K线数据，key 为 "HK.00700" 格式
    stocks: HashMap<String, Vec<DailyKline>>,
}

/// 日线分析引擎
pub struct DailyAnalysisEngine {
    /// 每只股票的日K线数据
    klines: HashMap<StockCode, Vec<DailyKline>>,
    /// 当前指标
    indicators: HashMap<StockCode, TechnicalIndicators>,
    /// 上一次指标（用于检测交叉）
    prev_indicators: HashMap<StockCode, TechnicalIndicators>,
    /// 日线信号
    signals: HashMap<StockCode, Vec<TimedSignal>>,
}

/// 最大保留天数
const MAX_KLINE_DAYS: usize = 150;

impl DailyAnalysisEngine {
    pub fn new() -> Self {
        Self {
            klines: HashMap::new(),
            indicators: HashMap::new(),
            prev_indicators: HashMap::new(),
            signals: HashMap::new(),
        }
    }

    /// 缓存文件路径
    pub fn cache_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(".config/qtrade/kline_cache.json")
    }

    /// 从缓存文件加载K线数据，返回 last_updated 日期
    pub fn load_cache(&mut self) -> Option<String> {
        let path = Self::cache_path();
        if !path.exists() {
            return None;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read kline cache: {}", e);
                return None;
            }
        };

        let cache: KlineCache = match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to parse kline cache: {}", e);
                return None;
            }
        };

        let last_updated = cache.last_updated.clone();
        let mut count = 0;

        for (key, klines) in cache.stocks {
            if let Some(code) = parse_stock_key(&key) {
                count += 1;
                self.klines.insert(code, klines);
            }
        }

        if count > 0 {
            // 用缓存数据计算指标
            self.recompute_all();
            info!("Loaded {} stocks from kline cache ({})", count, last_updated);
            Some(last_updated)
        } else {
            None
        }
    }

    /// 写入缓存文件
    pub fn save_cache(&self) {
        let path = Self::cache_path();

        // 确保目录存在
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                warn!("Failed to create cache dir: {}", e);
                return;
            }
        }

        let mut stocks = HashMap::new();
        for (code, klines) in &self.klines {
            stocks.insert(code.display_code(), klines.clone());
        }

        let cache = KlineCache {
            last_updated: chrono::Local::now().format("%Y-%m-%d").to_string(),
            stocks,
        };

        match serde_json::to_string(&cache) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    warn!("Failed to write kline cache: {}", e);
                } else {
                    info!("Saved kline cache to {}", path.display());
                }
            }
            Err(e) => {
                warn!("Failed to serialize kline cache: {}", e);
            }
        }
    }

    /// 合并新K线数据到已有缓存（按日期去重，保留最近 MAX_KLINE_DAYS 天）
    pub fn merge_update(&mut self, new_data: HashMap<StockCode, Vec<DailyKline>>) {
        for (code, new_klines) in new_data {
            let existing = self.klines.entry(code.clone()).or_default();

            // 用 HashMap 按日期去重，新数据覆盖旧数据
            let mut by_date: HashMap<String, DailyKline> = existing
                .drain(..)
                .map(|k| (k.date.clone(), k))
                .collect();

            for kl in new_klines {
                by_date.insert(kl.date.clone(), kl);
            }

            // 按日期排序，保留最近 MAX_KLINE_DAYS 天
            let mut merged: Vec<DailyKline> = by_date.into_values().collect();
            merged.sort_by(|a, b| a.date.cmp(&b.date));
            if merged.len() > MAX_KLINE_DAYS {
                merged = merged.split_off(merged.len() - MAX_KLINE_DAYS);
            }

            *existing = merged;
        }

        self.recompute_all();
    }

    /// 全量更新（替换所有数据）
    pub fn update(&mut self, kline_data: HashMap<StockCode, Vec<DailyKline>>) {
        for (code, klines) in kline_data {
            self.klines.insert(code, klines);
        }
        self.recompute_all();
    }

    /// 重新计算所有股票的指标和信号
    fn recompute_all(&mut self) {
        for (code, klines) in &self.klines {
            if klines.len() < 2 {
                continue;
            }

            let close_prices: Vec<f64> = klines.iter().map(|k| k.close).collect();
            let volumes: Vec<u64> = klines.iter().map(|k| k.volume).collect();

            // 用倒数第二根K线的数据计算 prev_indicators
            if close_prices.len() >= 2 {
                let prev_prices = &close_prices[..close_prices.len() - 1];
                let prev_ti = Self::compute_indicators(prev_prices);
                self.prev_indicators.insert(code.clone(), prev_ti);
            }

            // 用全部K线计算当前指标
            let ti = Self::compute_indicators(&close_prices);

            // 检测信号
            let prev = self.prev_indicators.get(code);
            let raw_signals = signals::detect_signals(&ti, prev, &close_prices, &volumes);

            let timed_signals: Vec<TimedSignal> = raw_signals
                .into_iter()
                .map(|signal| TimedSignal {
                    signal,
                    timeframe: Timeframe::Daily,
                })
                .collect();

            self.indicators.insert(code.clone(), ti);
            self.signals.insert(code.clone(), timed_signals);
        }
    }

    /// 获取已缓存的股票数量
    pub fn stock_count(&self) -> usize {
        self.klines.len()
    }

    /// 获取某只股票的缓存K线天数
    pub fn cached_days(&self, code: &StockCode) -> usize {
        self.klines.get(code).map(|k| k.len()).unwrap_or(0)
    }

    /// 获取所有日线指标
    pub fn get_indicators(&self) -> &HashMap<StockCode, TechnicalIndicators> {
        &self.indicators
    }

    /// 获取所有日线信号
    pub fn get_signals(&self) -> &HashMap<StockCode, Vec<TimedSignal>> {
        &self.signals
    }

    /// 计算技术指标（复用 indicators 模块的纯函数）
    fn compute_indicators(prices: &[f64]) -> TechnicalIndicators {
        TechnicalIndicators {
            ma5: indicators::sma(prices, 5),
            ma10: indicators::sma(prices, 10),
            ma20: indicators::sma(prices, 20),
            ma60: indicators::sma(prices, 60),

            macd_dif: {
                let (dif, _, _) = indicators::macd_latest(prices, 12, 26, 9);
                dif
            },
            macd_dea: {
                let (_, dea, _) = indicators::macd_latest(prices, 12, 26, 9);
                dea
            },
            macd_histogram: {
                let (_, _, hist) = indicators::macd_latest(prices, 12, 26, 9);
                hist
            },

            rsi6: indicators::rsi(prices, 6),
            rsi12: indicators::rsi(prices, 12),
            rsi24: indicators::rsi(prices, 24),
        }
    }
}

/// 解析 "HK.00700" 格式的 key 为 StockCode
fn parse_stock_key(key: &str) -> Option<StockCode> {
    let mut parts = key.splitn(2, '.');
    let market_str = parts.next()?;
    let code = parts.next()?;
    let market = match market_str {
        "HK" => crate::models::Market::HK,
        "SH" => crate::models::Market::SH,
        "SZ" => crate::models::Market::SZ,
        "US" => crate::models::Market::US,
        _ => return None,
    };
    Some(StockCode::new(market, code))
}

/// 判断缓存是否需要全量刷新（距今超过 max_age_days 天）
pub fn cache_needs_full_refresh(last_updated: &str, max_age_days: i64) -> bool {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    if last_updated == today {
        return false; // 当天已更新，不需要刷新
    }

    let last = match chrono::NaiveDate::parse_from_str(last_updated, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return true,
    };
    let now = match chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return true,
    };

    (now - last).num_days() > max_age_days
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Market;

    fn make_klines(count: usize, base_price: f64) -> Vec<DailyKline> {
        (0..count)
            .map(|i| {
                let price = base_price + (i as f64 * 0.1).sin() * 5.0 + i as f64 * 0.3;
                DailyKline {
                    open: price - 0.5,
                    close: price,
                    high: price + 1.0,
                    low: price - 1.0,
                    volume: 1_000_000 + (i as u64 * 10_000),
                    turnover: price * 1_000_000.0,
                    date: format!("2025-{:02}-{:02}", (i / 28) + 1, (i % 28) + 1),
                }
            })
            .collect()
    }

    #[test]
    fn test_daily_engine_basic() {
        let mut engine = DailyAnalysisEngine::new();

        let code = StockCode::new(Market::HK, "00700");
        let klines = make_klines(60, 380.0);

        let mut data = HashMap::new();
        data.insert(code.clone(), klines);

        engine.update(data);

        let indicators = engine.get_indicators();
        let ti = indicators.get(&code).expect("should have indicators");

        assert!(ti.ma5.is_some(), "MA5 should be computed");
        assert!(ti.ma20.is_some(), "MA20 should be computed");
        assert!(ti.ma60.is_some(), "MA60 should be computed");
        assert!(ti.macd_dif.is_some(), "MACD DIF should be computed");
        assert!(ti.rsi6.is_some(), "RSI6 should be computed");
    }

    #[test]
    fn test_daily_signals_tagged() {
        let mut engine = DailyAnalysisEngine::new();

        let code = StockCode::new(Market::HK, "00700");
        let klines: Vec<DailyKline> = (0..30)
            .map(|i| {
                let price = 100.0 + i as f64 * 2.0;
                DailyKline {
                    open: price - 0.5,
                    close: price,
                    high: price + 1.0,
                    low: price - 1.0,
                    volume: 1_000_000,
                    turnover: price * 1_000_000.0,
                    date: format!("2025-01-{:02}", i + 1),
                }
            })
            .collect();

        let mut data = HashMap::new();
        data.insert(code.clone(), klines);

        engine.update(data);

        let signals = engine.get_signals();
        if let Some(sigs) = signals.get(&code) {
            for sig in sigs {
                assert_eq!(
                    sig.timeframe,
                    Timeframe::Daily,
                    "All signals should be Daily timeframe"
                );
            }
        }
    }

    #[test]
    fn test_merge_update() {
        let mut engine = DailyAnalysisEngine::new();
        let code = StockCode::new(Market::HK, "00700");

        // 初始数据：10 天
        let initial: Vec<DailyKline> = (0..10)
            .map(|i| DailyKline {
                open: 100.0,
                close: 100.0 + i as f64,
                high: 105.0,
                low: 95.0,
                volume: 1_000_000,
                turnover: 100_000_000.0,
                date: format!("2025-01-{:02}", i + 1),
            })
            .collect();

        let mut data = HashMap::new();
        data.insert(code.clone(), initial);
        engine.update(data);

        assert_eq!(engine.klines.get(&code).unwrap().len(), 10);

        // 增量合并：3 天（1 天重叠 + 2 天新增）
        let incremental: Vec<DailyKline> = vec![
            DailyKline {
                open: 100.0,
                close: 200.0, // 覆盖 01-10
                high: 205.0,
                low: 195.0,
                volume: 2_000_000,
                turnover: 200_000_000.0,
                date: "2025-01-10".to_string(),
            },
            DailyKline {
                open: 100.0,
                close: 210.0,
                high: 215.0,
                low: 205.0,
                volume: 2_000_000,
                turnover: 210_000_000.0,
                date: "2025-01-11".to_string(),
            },
            DailyKline {
                open: 100.0,
                close: 220.0,
                high: 225.0,
                low: 215.0,
                volume: 2_000_000,
                turnover: 220_000_000.0,
                date: "2025-01-12".to_string(),
            },
        ];

        let mut new_data = HashMap::new();
        new_data.insert(code.clone(), incremental);
        engine.merge_update(new_data);

        let klines = engine.klines.get(&code).unwrap();
        assert_eq!(klines.len(), 12); // 10 - 1 overlap + 3 = 12
        // 01-10 应该被覆盖为 close=200.0
        let jan10 = klines.iter().find(|k| k.date == "2025-01-10").unwrap();
        assert_eq!(jan10.close, 200.0);
    }

    #[test]
    fn test_cache_needs_full_refresh() {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert!(!cache_needs_full_refresh(&today, 3));
        assert!(cache_needs_full_refresh("2020-01-01", 3));
    }
}
