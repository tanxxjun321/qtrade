//! 分析引擎：事件型 tick 信号检测
//!
//! 检测 5 类事件信号（触发一次后保持显示，不频繁翻转）：
//! - VWAP 偏离：价格偏离成交均价超阈值
//! - 日内新高/新低突破：突破已知极值
//! - 急涨急跌：短窗口内价格剧烈变动
//! - 振幅突破：日内振幅超阈值
//! - 量能突变：增量成交量相对窗口均值突增

use std::collections::HashMap;
use crate::config::AnalysisConfig;
use crate::models::{QuoteSnapshot, Signal, StockCode};

/// 每只股票的价格/量能历史窗口
#[derive(Debug)]
struct PriceWindow {
    prices: Vec<f64>,
    /// 增量成交量序列（相邻快照的 cumulative volume 之差）
    vol_deltas: Vec<u64>,
    max_size: usize,
}

impl PriceWindow {
    fn new(max_size: usize) -> Self {
        Self {
            prices: Vec::with_capacity(max_size),
            vol_deltas: Vec::with_capacity(max_size),
            max_size,
        }
    }

    fn push_price(&mut self, price: f64) {
        self.prices.push(price);
        if self.prices.len() > self.max_size {
            self.prices.remove(0);
        }
    }

    fn push_vol_delta(&mut self, delta: u64) {
        self.vol_deltas.push(delta);
        if self.vol_deltas.len() > self.max_size {
            self.vol_deltas.remove(0);
        }
    }
}

/// 每只股票的事件状态（防重复触发）
#[derive(Debug, Default)]
struct TickState {
    /// 已接收的 tick 数（预热计数）
    tick_count: u32,
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
    /// 急涨急跌上涨方向已触发（滞后重置）
    rapid_move_up_triggered: bool,
    /// 急涨急跌下跌方向已触发（滞后重置）
    rapid_move_down_triggered: bool,
    /// 上一次累计成交量（用于计算增量）
    prev_cumulative_volume: u64,
    /// 量能突变已触发（滞后重置）
    volume_spike_triggered: bool,
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
    rapid_move_reset_pct: f64,
    rapid_move_efficiency: f64,
    rapid_move_min_change: f64,
    amplitude_breakout_pct: f64,
    volume_spike_ratio: f64,
    warmup_ticks: u32,
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
            rapid_move_reset_pct: config.rapid_move_reset_pct,
            rapid_move_efficiency: config.rapid_move_efficiency,
            rapid_move_min_change: config.rapid_move_min_change,
            amplitude_breakout_pct: config.amplitude_breakout_pct,
            volume_spike_ratio: config.volume_spike_ratio,
            warmup_ticks: config.warmup_ticks,
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
        window.push_price(quote.last_price);

        let ts = self.tick_states.entry(quote.code.clone()).or_default();

        // 计算增量成交量
        if ts.prev_cumulative_volume > 0 && quote.volume >= ts.prev_cumulative_volume {
            let delta = quote.volume - ts.prev_cumulative_volume;
            window.push_vol_delta(delta);
        }
        ts.prev_cumulative_volume = quote.volume;

        // 预热：前 N 个 tick 仅记录数据，不产生信号
        ts.tick_count += 1;
        if ts.tick_count <= self.warmup_ticks {
            return signals;
        }

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

        // 3. 急涨急跌（停滞检查 + 方向效率 + 滞后重置）
        let prices = &window.prices;
        if prices.len() > self.rapid_move_window {
            // 第一层：价格停滞检查 — 当前价与上一快照一致则跳过
            let prev_price = prices[prices.len() - 2];
            let is_stale = prev_price > 0.0
                && ((quote.last_price - prev_price).abs() / prev_price * 100.0) < 0.01;

            if !is_stale {
                let window_start = prices.len() - 1 - self.rapid_move_window;
                let old_price = prices[window_start];
                if old_price > 0.0 {
                    let net_change = quote.last_price - old_price;
                    let change_pct = net_change / old_price * 100.0;

                    // 第二层：方向效率 = |净变动| / 总路径
                    let mut total_path = 0.0_f64;
                    for i in (window_start + 1)..prices.len() {
                        total_path += (prices[i] - prices[i - 1]).abs();
                    }
                    let efficiency = if total_path > 0.0 {
                        net_change.abs() / total_path
                    } else {
                        0.0
                    };

                    // 第三层：滞后重置 — 幅度达标 + 效率达标 + 绝对变动达标 + 未被抑制
                    let abs_change = net_change.abs();
                    if change_pct >= self.rapid_move_pct
                        && efficiency >= self.rapid_move_efficiency
                        && abs_change >= self.rapid_move_min_change
                        && !ts.rapid_move_up_triggered
                    {
                        signals.push(Signal::RapidMove { change_pct });
                        ts.rapid_move_up_triggered = true;
                    } else if change_pct <= -self.rapid_move_pct
                        && efficiency >= self.rapid_move_efficiency
                        && abs_change >= self.rapid_move_min_change
                        && !ts.rapid_move_down_triggered
                    {
                        signals.push(Signal::RapidMove { change_pct });
                        ts.rapid_move_down_triggered = true;
                    }

                    // 重置：变动回落到 reset 阈值内
                    if change_pct < self.rapid_move_reset_pct {
                        ts.rapid_move_up_triggered = false;
                    }
                    if change_pct > -self.rapid_move_reset_pct {
                        ts.rapid_move_down_triggered = false;
                    }
                }
            }
        }

        // 4. 振幅突破
        if quote.amplitude >= self.amplitude_breakout_pct && !ts.amplitude_triggered {
            signals.push(Signal::AmplitudeBreakout { amplitude_pct: quote.amplitude });
            ts.amplitude_triggered = true;
        }

        // 5. 量能突变：增量成交量 vs 窗口均值
        let deltas = &window.vol_deltas;
        if deltas.len() >= 3 {
            let latest = *deltas.last().unwrap() as f64;
            // 均值不含最新一条，避免自身拉高均值
            let avg: f64 = deltas[..deltas.len() - 1].iter().map(|d| *d as f64).sum::<f64>()
                / (deltas.len() - 1) as f64;
            if avg > 0.0 {
                let ratio = latest / avg;
                if ratio >= self.volume_spike_ratio && !ts.volume_spike_triggered {
                    signals.push(Signal::VolumeSpike { ratio });
                    ts.volume_spike_triggered = true;
                }
                // 滞后重置：回落到 1.5 倍以下
                if ratio < 1.5 {
                    ts.volume_spike_triggered = false;
                }
            }
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
            rapid_move_reset_pct: 0.5,
            rapid_move_efficiency: 0.6,
            rapid_move_min_change: 0.05,
            amplitude_breakout_pct: 5.0,
            volume_spike_ratio: 3.0,
            tick_signal_display_minutes: 5,
            warmup_ticks: 0, // 测试中默认关闭预热
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
    fn test_engine_volume_spike() {
        let config = AnalysisConfig {
            volume_spike_ratio: 3.0,
            rapid_move_window: 2,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        // 累计成交量递增：1000, 2000, 3000, 4000 → 增量均为 1000
        for cum_vol in [1000u64, 2000, 3000, 4000] {
            let mut q = make_quote("00700", 100.0);
            q.volume = cum_vol;
            engine.process(&q);
        }

        // 突然放量：增量 5000（5x 均值）→ 触发
        let mut q = make_quote("00700", 100.0);
        q.volume = 9000; // delta = 9000 - 4000 = 5000
        let sigs = engine.process(&q);
        assert!(sigs.iter().any(|s| matches!(s, Signal::VolumeSpike { ratio } if *ratio >= 3.0)));

        // 已触发，不重复
        let mut q2 = make_quote("00700", 100.0);
        q2.volume = 14000; // delta = 5000, 仍高但已触发
        let sigs = engine.process(&q2);
        assert!(sigs.iter().all(|s| !matches!(s, Signal::VolumeSpike { .. })));
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

    #[test]
    fn test_rapid_move_hysteresis() {
        // 测试：触发后同方向不重复，窗口 change_pct 回落到 reset 阈值内后可再触发
        let config = AnalysisConfig {
            rapid_move_window: 2,
            rapid_move_pct: 1.0,
            rapid_move_reset_pct: 0.5,
            rapid_move_efficiency: 0.6,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        // 填充窗口：3 个点 = window(2) + 1
        // 100.0, 100.0 不触发（len<=window 或 stale）
        engine.process(&make_quote("00700", 100.0));
        engine.process(&make_quote("00700", 100.0));

        // 急涨到 102.0（+2%），触发
        // prices=[100.0, 100.0, 102.0], prev=100.0, cur=102.0 → not stale
        // old=100.0, net=2%, eff: path=|100-100|+|102-100|=2, eff=2/2=1.0
        let sigs = engine.process(&make_quote("00700", 102.0));
        assert!(sigs.iter().any(|s| matches!(s, Signal::RapidMove { change_pct } if *change_pct > 0.0)),
            "should trigger rapid move up");

        // 继续上涨 — 已触发不重复
        let sigs = engine.process(&make_quote("00700", 103.0));
        assert!(sigs.iter().all(|s| !matches!(s, Signal::RapidMove { .. })),
            "should not repeat while up_triggered");

        // 价格稳定在高位，窗口 change_pct 回落
        // prices=[103.0, 103.2, 103.2]: old=103.0, change=0.19% < reset(0.5%) → reset
        engine.process(&make_quote("00700", 103.2));
        engine.process(&make_quote("00700", 103.2)); // stale, skip — up_triggered 不在这里重置

        // 需要 non-stale tick 来触发 reset 逻辑
        // prices=[103.2, 103.2, 103.3]: prev=103.2, cur=103.3 → not stale
        // old=103.2, change=0.097% < 0.5 → reset up_triggered
        engine.process(&make_quote("00700", 103.3));

        // 再次急涨
        // prices=[103.2, 103.3, 105.0]: old=103.2, change=1.74%, eff: path=0.1+1.7=1.8, net=1.8, eff=1.0
        let sigs = engine.process(&make_quote("00700", 105.0));
        assert!(sigs.iter().any(|s| matches!(s, Signal::RapidMove { change_pct } if *change_pct > 0.0)),
            "should re-trigger after reset");
    }

    #[test]
    fn test_rapid_move_oscillation_rejected() {
        let config = AnalysisConfig {
            rapid_move_window: 4,
            rapid_move_pct: 1.0,
            rapid_move_reset_pct: 0.5,
            rapid_move_efficiency: 0.6,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        // 震荡：100 → 101 → 100 → 101 → 100 → 101
        // net = 1%, total_path = 5%, efficiency = 0.2 < 0.6
        for price in [100.0, 101.0, 100.0, 101.0, 100.0] {
            engine.process(&make_quote("00700", price));
        }
        let sigs = engine.process(&make_quote("00700", 101.0));
        assert!(sigs.iter().all(|s| !matches!(s, Signal::RapidMove { .. })));
    }

    #[test]
    fn test_rapid_move_stale_price_rejected() {
        let config = AnalysisConfig {
            rapid_move_window: 3,
            rapid_move_pct: 1.0,
            rapid_move_reset_pct: 0.5,
            rapid_move_efficiency: 0.6,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        // 盘中涨到 101，然后休市价格不变
        // 窗口: [99, 100, 101, 101] → net=2%, 但最后两个相同 → stale
        for price in [99.0, 100.0, 101.0] {
            engine.process(&make_quote("00700", price));
        }
        // 价格不变（休市）
        let sigs = engine.process(&make_quote("00700", 101.0));
        assert!(sigs.iter().all(|s| !matches!(s, Signal::RapidMove { .. })));
    }

    #[test]
    fn test_rapid_move_consistent_direction() {
        let config = AnalysisConfig {
            rapid_move_window: 3,
            rapid_move_pct: 1.0,
            rapid_move_reset_pct: 0.5,
            rapid_move_efficiency: 0.6,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        // 初始窗口填充（不触发：窗口还没满或变化不够大）
        for price in [100.0, 99.8, 99.6] {
            let sigs = engine.process(&make_quote("00700", price));
            assert!(sigs.iter().all(|s| !matches!(s, Signal::RapidMove { .. })));
        }

        // 单向下跌触发：窗口 [100.0, 99.8, 99.6, 98.5]
        // net = (98.5-100.0)/100.0 = -1.5%, efficiency = 1.0 → 触发
        let sigs = engine.process(&make_quote("00700", 98.5));
        assert!(sigs.iter().any(|s| matches!(s, Signal::RapidMove { change_pct } if *change_pct < 0.0)));

        // 已触发，继续下跌不重复
        let sigs = engine.process(&make_quote("00700", 97.5));
        assert!(sigs.iter().all(|s| !matches!(s, Signal::RapidMove { .. })));
    }

    #[test]
    fn test_rapid_move_low_price_rejected() {
        // $0.20 股票涨 $0.01 = 5%，百分比超标但绝对值 < min_change(0.05)
        let config = AnalysisConfig {
            rapid_move_window: 2,
            rapid_move_pct: 1.0,
            rapid_move_min_change: 0.05,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        for price in [0.20, 0.20, 0.20] {
            engine.process(&make_quote("00700", price));
        }
        // +$0.01 = +5%，但绝对变动 $0.01 < $0.05 → 拒绝
        let sigs = engine.process(&make_quote("00700", 0.21));
        assert!(sigs.iter().all(|s| !matches!(s, Signal::RapidMove { .. })),
            "low-price $0.01 move should be rejected by min_change");

        // +$0.06 = +30%，绝对变动 $0.06 >= $0.05 → 通过
        let sigs = engine.process(&make_quote("00700", 0.27));
        assert!(sigs.iter().any(|s| matches!(s, Signal::RapidMove { .. })),
            "low-price $0.06 move should pass min_change");
    }

    #[test]
    fn test_warmup_suppresses_signals() {
        let config = AnalysisConfig {
            rapid_move_window: 2,
            rapid_move_pct: 1.0,
            warmup_ticks: 3,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        // 预热期内：即使数据剧烈变化也不产生信号
        let mut q1 = make_quote("00700", 100.0);
        q1.amplitude = 10.0; // 会触发振幅突破
        let sigs = engine.process(&q1); // tick 1
        assert!(sigs.is_empty(), "warmup tick 1 should produce no signals");

        let sigs = engine.process(&make_quote("00700", 105.0)); // tick 2
        assert!(sigs.is_empty(), "warmup tick 2 should produce no signals");

        let sigs = engine.process(&make_quote("00700", 110.0)); // tick 3
        assert!(sigs.is_empty(), "warmup tick 3 should produce no signals");

        // 预热结束后，正常产生信号
        let mut q4 = make_quote("00700", 115.0);
        q4.amplitude = 10.0;
        let sigs = engine.process(&q4); // tick 4
        assert!(!sigs.is_empty(), "post-warmup should produce signals");
    }
}
