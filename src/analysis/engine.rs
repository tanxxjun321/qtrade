//! 分析引擎：维护滚动窗口，收到新数据后计算指标

use std::collections::HashMap;
use crate::models::{QuoteSnapshot, Signal, StockCode, TechnicalIndicators};

use super::indicators;
use super::signals;

/// 单只股票的价格历史窗口
#[derive(Debug)]
struct PriceWindow {
    /// 价格序列（最新在末尾）
    prices: Vec<f64>,
    /// 成交量序列
    volumes: Vec<u64>,
    /// 最大窗口大小
    max_size: usize,
}

impl PriceWindow {
    fn new(max_size: usize) -> Self {
        Self {
            prices: Vec::with_capacity(max_size),
            volumes: Vec::with_capacity(max_size),
            max_size,
        }
    }

    fn push(&mut self, price: f64, volume: u64) {
        self.prices.push(price);
        self.volumes.push(volume);

        // 保持窗口大小
        if self.prices.len() > self.max_size {
            self.prices.remove(0);
            self.volumes.remove(0);
        }
    }

    fn len(&self) -> usize {
        self.prices.len()
    }
}

/// 分析引擎
pub struct AnalysisEngine {
    /// 每只股票的价格窗口
    windows: HashMap<StockCode, PriceWindow>,
    /// 窗口大小
    window_size: usize,
    /// 上一次的指标值（用于检测交叉）
    prev_indicators: HashMap<StockCode, TechnicalIndicators>,
}

impl AnalysisEngine {
    pub fn new(window_size: usize) -> Self {
        Self {
            windows: HashMap::new(),
            window_size,
            prev_indicators: HashMap::new(),
        }
    }

    /// 处理新的行情快照，返回计算的指标和信号
    pub fn process(
        &mut self,
        quote: &QuoteSnapshot,
    ) -> (TechnicalIndicators, Vec<Signal>) {
        // 更新价格窗口
        let ws = self.window_size;
        let window = self
            .windows
            .entry(quote.code.clone())
            .or_insert_with(|| PriceWindow::new(ws));

        window.push(quote.last_price, quote.volume);

        // 计算指标（拷贝出来避免借用冲突）
        let prices = window.prices.clone();
        let volumes = window.volumes.clone();

        let ti = Self::compute_indicators(&prices);

        // 检测信号
        let prev = self.prev_indicators.get(&quote.code);
        let sigs = signals::detect_signals(&ti, prev, &prices, &volumes);

        // 保存当前指标
        self.prev_indicators.insert(quote.code.clone(), ti.clone());

        (ti, sigs)
    }

    /// 批量处理多个行情快照
    pub fn process_batch(
        &mut self,
        quotes: &[QuoteSnapshot],
    ) -> HashMap<StockCode, (TechnicalIndicators, Vec<Signal>)> {
        let mut results = HashMap::new();
        for quote in quotes {
            let (ti, sigs) = self.process(quote);
            results.insert(quote.code.clone(), (ti, sigs));
        }
        results
    }

    /// 获取指定股票的当前指标
    pub fn get_indicators(&self, code: &StockCode) -> Option<&TechnicalIndicators> {
        self.prev_indicators.get(code)
    }

    /// 计算技术指标
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

    /// 获取价格窗口长度
    pub fn window_len(&self, code: &StockCode) -> usize {
        self.windows.get(code).map(|w| w.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DataSource, Market};

    fn make_quote(code: &str, price: f64) -> QuoteSnapshot {
        QuoteSnapshot {
            code: StockCode::new(Market::HK, code),
            name: "Test".to_string(),
            last_price: price,
            prev_close: price - 1.0,
            open_price: price,
            high_price: price + 1.0,
            low_price: price - 1.0,
            volume: 1000,
            turnover: 0.0,
            change: 1.0,
            change_pct: 0.5,
            turnover_rate: 0.0,
            amplitude: 0.0,
            extended_price: None,
            extended_change_pct: None,
            timestamp: chrono::Local::now(),
            source: DataSource::Cache,
        }
    }

    #[test]
    fn test_engine_basic() {
        let mut engine = AnalysisEngine::new(100);

        // 输入 20 个价格点
        for i in 0..20 {
            let price = 100.0 + i as f64;
            let quote = make_quote("00700", price);
            let (ti, _sigs) = engine.process(&quote);

            if i >= 4 {
                assert!(ti.ma5.is_some());
            }
        }
    }

    #[test]
    fn test_engine_multiple_stocks() {
        let mut engine = AnalysisEngine::new(100);

        let q1 = make_quote("00700", 388.0);
        let q2 = make_quote("09988", 120.0);

        engine.process(&q1);
        engine.process(&q2);

        assert_eq!(engine.window_len(&q1.code), 1);
        assert_eq!(engine.window_len(&q2.code), 1);
    }
}
