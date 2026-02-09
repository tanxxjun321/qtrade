//! 分析引擎：事件型 tick 信号检测
//!
//! 检测 4 类事件信号（触发一次后保持显示，不频繁翻转）：
//! - VWAP 偏离：价格偏离成交均价超阈值
//! - 日内新高/新低突破：突破已知极值
//! - 急涨急跌：短窗口内价格剧烈变动
//! - 振幅突破：日内振幅超阈值

use std::collections::HashMap;
use crate::config::AnalysisConfig;
use crate::models::{QuoteSnapshot, Signal, StockCode};

/// 每只股票的价格历史窗口（急涨急跌检测需要）
#[derive(Debug)]
struct PriceWindow {
    prices: Vec<f64>,
    max_size: usize,
}

impl PriceWindow {
    fn new(max_size: usize) -> Self {
        Self {
            prices: Vec::with_capacity(max_size),
            max_size,
        }
    }

    fn push(&mut self, price: f64) {
        self.prices.push(price);
        if self.prices.len() > self.max_size {
            self.prices.remove(0);
        }
    }
}

/// 每只股票的事件状态（防重复触发）
#[derive(Debug, Default)]
struct TickState {
    /// 上次已知日内最高
    last_known_high: f64,
    /// 上次已知日内最低
    last_known_low: f64,
    /// VWAP 偏离利多已触发（回到 reset 阈值内才重新检测）
    vwap_above_triggered: bool,
    /// VWAP 偏离利空已触发
    vwap_below_triggered: bool,
    /// 振幅突破已触发（日内仅一次）
    amplitude_triggered: bool,
}

/// 分析引擎
pub struct AnalysisEngine {
    /// 每只股票的价格窗口
    windows: HashMap<StockCode, PriceWindow>,
    /// 每只股票的事件状态
    tick_states: HashMap<StockCode, TickState>,
    /// 配置阈值
    vwap_deviation_pct: f64,
    vwap_reset_pct: f64,
    rapid_move_pct: f64,
    rapid_move_window: usize,
    amplitude_breakout_pct: f64,
}

impl AnalysisEngine {
    pub fn new(config: &AnalysisConfig) -> Self {
        Self {
            windows: HashMap::new(),
            tick_states: HashMap::new(),
            vwap_deviation_pct: config.vwap_deviation_pct,
            vwap_reset_pct: config.vwap_reset_pct,
            rapid_move_pct: config.rapid_move_pct,
            rapid_move_window: config.rapid_move_window as usize,
            amplitude_breakout_pct: config.amplitude_breakout_pct,
        }
    }

    /// 处理新的行情快照，返回新触发的事件型信号
    pub fn process(&mut self, quote: &QuoteSnapshot) -> Vec<Signal> {
        let mut signals = Vec::new();

        // 更新价格窗口
        let window_size = self.rapid_move_window + 1;
        let window = self
            .windows
            .entry(quote.code.clone())
            .or_insert_with(|| PriceWindow::new(window_size));
        window.push(quote.last_price);

        let ts = self.tick_states.entry(quote.code.clone()).or_default();

        // 1. VWAP 偏离
        if quote.volume > 0 && quote.turnover > 0.0 && quote.last_price > 0.0 {
            let vwap = quote.turnover / quote.volume as f64;
            let deviation = (quote.last_price - vwap) / vwap * 100.0;

            if deviation >= self.vwap_deviation_pct && !ts.vwap_above_triggered {
                signals.push(Signal::VwapDeviation { deviation_pct: deviation });
                ts.vwap_above_triggered = true;
            } else if deviation <= -self.vwap_deviation_pct && !ts.vwap_below_triggered {
                signals.push(Signal::VwapDeviation { deviation_pct: deviation });
                ts.vwap_below_triggered = true;
            }

            // 滞后重置
            if deviation.abs() < self.vwap_reset_pct {
                ts.vwap_above_triggered = false;
                ts.vwap_below_triggered = false;
            }
        }

        // 2. 日内新高/新低突破
        if quote.high_price > 0.0 && quote.low_price > 0.0 {
            if ts.last_known_high == 0.0 {
                // 首次：初始化，不触发
                ts.last_known_high = quote.high_price;
                ts.last_known_low = quote.low_price;
            } else {
                if quote.high_price > ts.last_known_high {
                    signals.push(Signal::IntradayHigh);
                    ts.last_known_high = quote.high_price;
                }
                if quote.low_price < ts.last_known_low {
                    signals.push(Signal::IntradayLow);
                    ts.last_known_low = quote.low_price;
                }
            }
        }

        // 3. 急涨急跌
        let prices = &window.prices;
        if prices.len() > self.rapid_move_window {
            let old_price = prices[prices.len() - 1 - self.rapid_move_window];
            if old_price > 0.0 {
                let change_pct = (quote.last_price - old_price) / old_price * 100.0;
                if change_pct.abs() >= self.rapid_move_pct {
                    signals.push(Signal::RapidMove { change_pct });
                }
            }
        }

        // 4. 振幅突破
        if quote.amplitude >= self.amplitude_breakout_pct && !ts.amplitude_triggered {
            signals.push(Signal::AmplitudeBreakout { amplitude_pct: quote.amplitude });
            ts.amplitude_triggered = true;
        }

        signals
    }

    /// 移除股票的 tick 级分析数据
    pub fn remove_stock(&mut self, code: &StockCode) {
        self.windows.remove(code);
        self.tick_states.remove(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AnalysisConfig;
    use crate::models::{DataSource, Market};

    fn default_config() -> AnalysisConfig {
        AnalysisConfig {
            daily_kline_enabled: true,
            daily_kline_days: 120,
            daily_kline_refresh_minutes: 30,
            vwap_deviation_pct: 2.0,
            vwap_reset_pct: 1.0,
            rapid_move_pct: 1.0,
            rapid_move_window: 5,
            amplitude_breakout_pct: 5.0,
            tick_signal_display_minutes: 5,
        }
    }

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
            turnover: price * 1000.0,
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
    fn test_engine_intraday_high() {
        let mut engine = AnalysisEngine::new(&default_config());

        // 首次：初始化，不触发
        let q1 = make_quote("00700", 100.0);
        let sigs = engine.process(&q1);
        assert!(sigs.iter().all(|s| !matches!(s, Signal::IntradayHigh)));

        // 新高突破
        let mut q2 = make_quote("00700", 102.0);
        q2.high_price = 103.0;
        let sigs = engine.process(&q2);
        assert!(sigs.iter().any(|s| matches!(s, Signal::IntradayHigh)));
    }

    #[test]
    fn test_engine_rapid_move() {
        let config = AnalysisConfig {
            rapid_move_window: 2,
            rapid_move_pct: 1.0,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        // 填充窗口
        for price in [100.0, 100.0, 100.0] {
            let q = make_quote("00700", price);
            engine.process(&q);
        }

        // 急涨
        let q = make_quote("00700", 102.0);
        let sigs = engine.process(&q);
        assert!(sigs.iter().any(|s| matches!(s, Signal::RapidMove { change_pct } if *change_pct > 0.0)));
    }

    #[test]
    fn test_engine_amplitude_breakout() {
        let mut engine = AnalysisEngine::new(&default_config());

        let mut q = make_quote("00700", 100.0);
        q.amplitude = 6.0;
        let sigs = engine.process(&q);
        assert!(sigs.iter().any(|s| matches!(s, Signal::AmplitudeBreakout { .. })));

        // 第二次不再触发
        let sigs = engine.process(&q);
        assert!(sigs.iter().all(|s| !matches!(s, Signal::AmplitudeBreakout { .. })));
    }

    #[test]
    fn test_engine_multiple_stocks() {
        let mut engine = AnalysisEngine::new(&default_config());

        let q1 = make_quote("00700", 388.0);
        let q2 = make_quote("09988", 120.0);

        engine.process(&q1);
        engine.process(&q2);

        assert!(engine.windows.contains_key(&q1.code));
        assert!(engine.windows.contains_key(&q2.code));
    }
}
