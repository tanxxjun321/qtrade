//! 分析引擎：事件型 tick 信号检测
//!
//! 检测 4 类事件信号（触发一次后保持显示，不频繁翻转）：
//! - VWAP 偏离：价格偏离成交均价超阈值
//! - 急涨急跌：短窗口内价格剧烈变动
//! - 振幅突破：日内振幅超阈值
//! - 量能突变：增量成交量相对窗口均值突增

use std::collections::HashMap;
use crate::config::AnalysisConfig;
use crate::models::{QuoteSnapshot, Signal, StockCode};

/// 每只股票的价格历史窗口
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

    fn push_price(&mut self, price: f64) {
        self.prices.push(price);
        if self.prices.len() > self.max_size {
            self.prices.remove(0);
        }
    }
}

/// 每只股票的成交量跟踪器（时间戳 + 累计成交量 ring buffer）
///
/// 用时间归一化的量速率（股/秒）做基线，消除 tick 间隔抖动：
///   baseline_rate = 基线窗口总量 / 基线窗口秒数
///   expected      = baseline_rate × 当前 tick 间隔
///   ratio         = actual_delta / expected
#[derive(Debug)]
struct VolumeTracker {
    /// (时间戳秒, 累计成交量)
    samples: std::collections::VecDeque<(f64, u64)>,
    /// 基线窗口长度（秒）
    max_window_secs: f64,
    /// 基线不足此秒数不触发
    min_baseline_secs: f64,
}

impl VolumeTracker {
    fn new(max_window_secs: f64, min_baseline_secs: f64) -> Self {
        Self {
            samples: std::collections::VecDeque::new(),
            max_window_secs,
            min_baseline_secs,
        }
    }

    /// 记录一个采样点
    fn push(&mut self, timestamp: f64, cumulative_volume: u64) {
        self.samples.push_back((timestamp, cumulative_volume));
        // 淘汰超出窗口的旧样本（保留至少 2 个）
        while self.samples.len() > 2 {
            let oldest_time = self.samples[0].0;
            if timestamp - oldest_time > self.max_window_secs {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// 计算当前 tick 的量能倍数
    ///
    /// 返回 `Some((ratio, delta))` 或 `None`（基线不足）
    fn compute_ratio(&self) -> Option<(f64, u64)> {
        let n = self.samples.len();
        if n < 3 {
            return None;
        }

        let (cur_time, cur_vol) = self.samples[n - 1];
        let (prev_time, prev_vol) = self.samples[n - 2];
        let (oldest_time, oldest_vol) = self.samples[0];

        // 当前 tick 的增量和间隔
        let elapsed = cur_time - prev_time;
        if elapsed <= 0.0 {
            return None;
        }
        let delta = cur_vol.saturating_sub(prev_vol);

        // 基线：从最早到倒数第二个样本（不含当前 tick）
        let baseline_time = prev_time - oldest_time;
        if baseline_time < self.min_baseline_secs {
            return None;
        }
        let baseline_vol = prev_vol.saturating_sub(oldest_vol);
        let baseline_rate = baseline_vol as f64 / baseline_time;
        if baseline_rate <= 0.0 {
            return None;
        }

        // 当前 tick "应该"有多少量
        let expected = baseline_rate * elapsed;
        if expected <= 0.0 {
            return None;
        }

        let ratio = delta as f64 / expected;
        Some((ratio, delta))
    }
}

/// 每只股票的事件状态（防重复触发）
#[derive(Debug, Default)]
struct TickState {
    /// 已接收的 tick 数（预热计数）
    tick_count: u32,
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
    /// 量能突变已触发（滞后重置）
    volume_spike_triggered: bool,
}

/// ADV 绝对量门槛除数：单 tick delta >= ADV / ADV_DIVISOR 才算放量
const ADV_DIVISOR: f64 = 1000.0;

/// 分析引擎
pub struct AnalysisEngine {
    /// 每只股票的价格窗口
    windows: HashMap<StockCode, PriceWindow>,
    /// 每只股票的成交量跟踪器
    vol_trackers: HashMap<StockCode, VolumeTracker>,
    /// 每只股票的事件状态
    tick_states: HashMap<StockCode, TickState>,
    /// 每只股票的日均成交量（ADV），由外部 daily engine 注入
    adv_map: HashMap<StockCode, f64>,
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
    volume_baseline_secs: f64,
    volume_min_baseline_secs: f64,
    warmup_ticks: u32,
}

impl AnalysisEngine {
    pub fn new(config: &AnalysisConfig) -> Self {
        Self {
            windows: HashMap::new(),
            vol_trackers: HashMap::new(),
            tick_states: HashMap::new(),
            adv_map: HashMap::new(),
            vwap_deviation_pct: config.vwap_deviation_pct,
            vwap_reset_pct: config.vwap_reset_pct,
            rapid_move_pct: config.rapid_move_pct,
            rapid_move_window: config.rapid_move_window as usize,
            rapid_move_reset_pct: config.rapid_move_reset_pct,
            rapid_move_efficiency: config.rapid_move_efficiency,
            rapid_move_min_change: config.rapid_move_min_change,
            amplitude_breakout_pct: config.amplitude_breakout_pct,
            volume_spike_ratio: config.volume_spike_ratio,
            volume_baseline_secs: config.volume_baseline_secs,
            volume_min_baseline_secs: config.volume_min_baseline_secs,
            warmup_ticks: config.warmup_ticks,
        }
    }

    /// 更新日均成交量（ADV）数据，由 daily engine 注入
    pub fn update_adv(&mut self, adv: HashMap<StockCode, f64>) {
        self.adv_map = adv;
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

        // 更新成交量跟踪器（时间戳 + 累计量）
        let baseline_secs = self.volume_baseline_secs;
        let min_baseline_secs = self.volume_min_baseline_secs;
        let vol_tracker = self
            .vol_trackers
            .entry(quote.code.clone())
            .or_insert_with(|| VolumeTracker::new(baseline_secs, min_baseline_secs));
        let ts_secs = quote.timestamp.timestamp() as f64
            + quote.timestamp.timestamp_subsec_millis() as f64 / 1000.0;
        vol_tracker.push(ts_secs, quote.volume);

        let ts = self.tick_states.entry(quote.code.clone()).or_default();

        // 预热：前 N 个 tick 仅记录数据，不产生信号
        ts.tick_count += 1;
        if ts.tick_count <= self.warmup_ticks {
            return signals;
        }

        // 1. VWAP 偏离（指数的 turnover/volume 与指数点位不可比，跳过）
        if !quote.code.is_index()
            && quote.volume > 0 && quote.turnover > 0.0 && quote.last_price > 0.0
        {
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

        // 2. 急涨急跌（停滞检查 + 方向效率 + 滞后重置）
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

        // 3. 振幅突破
        if quote.amplitude >= self.amplitude_breakout_pct && !ts.amplitude_triggered {
            signals.push(Signal::AmplitudeBreakout { amplitude_pct: quote.amplitude });
            ts.amplitude_triggered = true;
        }

        // 4. 量能突变：时间归一化量速率 + ADV 绝对量门槛（指数跳过）
        if !quote.code.is_index() {
        if let Some((ratio, delta)) = vol_tracker.compute_ratio() {
            // 绝对量门槛：有 ADV 时要求 delta >= ADV / ADV_DIVISOR，无 ADV 时仅看倍数
            let adv_ok = match self.adv_map.get(&quote.code) {
                Some(&adv) if adv > 0.0 => delta as f64 >= adv / ADV_DIVISOR,
                _ => true,
            };

            if ratio >= self.volume_spike_ratio && adv_ok && !ts.volume_spike_triggered {
                signals.push(Signal::VolumeSpike { ratio, price: quote.last_price, delta });
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
        self.vol_trackers.remove(code);
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
            volume_baseline_secs: 300.0,
            volume_min_baseline_secs: 0.0, // 测试中关闭最短基线要求
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

        // 构造带时间戳的报价：每 3 秒一个 tick，累计量均匀递增
        let base_time = chrono::Local::now();
        let make_timed = |secs: i64, cum_vol: u64| {
            let mut q = make_quote("00700", 100.0);
            q.volume = cum_vol;
            q.timestamp = base_time + chrono::Duration::seconds(secs);
            q
        };

        // 基线：每 3 秒增加 1000 股（约 333 股/秒）
        engine.process(&make_timed(0, 1000));
        engine.process(&make_timed(3, 2000));
        engine.process(&make_timed(6, 3000));
        engine.process(&make_timed(9, 4000));

        // 突然放量：3 秒内增加 5000 股（约 1667 股/秒，5x 基线）→ 触发
        let sigs = engine.process(&make_timed(12, 9000));
        assert!(sigs.iter().any(|s| matches!(s, Signal::VolumeSpike { ratio, .. } if *ratio >= 3.0)),
            "should trigger volume spike on 5x burst");

        // 已触发，不重复
        let sigs = engine.process(&make_timed(15, 14000));
        assert!(sigs.iter().all(|s| !matches!(s, Signal::VolumeSpike { .. })),
            "should not repeat while triggered");
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

    #[test]
    fn test_volume_spike_time_normalized() {
        // tick 间隔抖动不应导致误触发：5 秒 tick 的 delta 自然比 2 秒大，
        // 但归一化到量速率后倍数仍为 ~1x
        let config = AnalysisConfig {
            volume_spike_ratio: 3.0,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        let base_time = chrono::Local::now();
        let rate = 1000.0; // 1000 股/秒

        // 基线：均匀 3 秒间隔
        for i in 0..5 {
            let secs = i * 3;
            let mut q = make_quote("00700", 100.0);
            q.volume = (rate * secs as f64) as u64;
            q.timestamp = base_time + chrono::Duration::seconds(secs);
            engine.process(&q);
        }

        // 正常量但 5 秒间隔（delta 比 3 秒 tick 大 1.67x，但归一化后 ~1x）
        let mut q = make_quote("00700", 100.0);
        q.volume = (rate * 17.0) as u64; // 12s→17s, delta=5000 in 5s = 1000/s
        q.timestamp = base_time + chrono::Duration::seconds(17);
        let sigs = engine.process(&q);
        assert!(sigs.iter().all(|s| !matches!(s, Signal::VolumeSpike { .. })),
            "time-normalized rate ~1x should not trigger spike");
    }

    #[test]
    fn test_volume_spike_adv_threshold() {
        // 有 ADV 时，绝对量不足应被过滤
        let config = AnalysisConfig {
            volume_spike_ratio: 3.0,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        // 设置 ADV = 10,000,000 股 → 阈值 = 10,000,000 / 1000 = 10,000 股
        let code = StockCode::new(Market::HK, "00700");
        let mut adv = HashMap::new();
        adv.insert(code.clone(), 10_000_000.0);
        engine.update_adv(adv);

        let base_time = chrono::Local::now();
        // 极低量基线：每 3 秒 10 股（冷门时段）
        for i in 0..5 {
            let secs = i * 3;
            let mut q = make_quote("00700", 100.0);
            q.volume = (10 * i) as u64;
            q.timestamp = base_time + chrono::Duration::seconds(secs);
            engine.process(&q);
        }

        // "放量" 50 股 → ratio 很高但绝对量 50 远低于 ADV 门槛 10,000
        let mut q = make_quote("00700", 100.0);
        q.volume = 90; // delta=50 in 3s
        q.timestamp = base_time + chrono::Duration::seconds(15);
        let sigs = engine.process(&q);
        assert!(sigs.iter().all(|s| !matches!(s, Signal::VolumeSpike { .. })),
            "delta below ADV threshold should not trigger");
    }

    #[test]
    fn test_volume_spike_hysteresis_reset() {
        // 放量触发 → 回落 → 再次放量可重新触发
        let config = AnalysisConfig {
            volume_spike_ratio: 3.0,
            ..default_config()
        };
        let mut engine = AnalysisEngine::new(&config);

        let base_time = chrono::Local::now();
        let rate = 1000.0;

        // 基线
        for i in 0..5 {
            let secs = i * 3;
            let mut q = make_quote("00700", 100.0);
            q.volume = (rate * secs as f64) as u64;
            q.timestamp = base_time + chrono::Duration::seconds(secs);
            engine.process(&q);
        }

        // 放量 5x → 触发
        let mut q = make_quote("00700", 100.0);
        q.volume = (rate * 12.0 + rate * 3.0 * 5.0) as u64;
        q.timestamp = base_time + chrono::Duration::seconds(15);
        let sigs = engine.process(&q);
        assert!(sigs.iter().any(|s| matches!(s, Signal::VolumeSpike { .. })),
            "5x spike should trigger");

        // 回落到正常量（ratio < 1.5 → reset）
        let prev_vol = q.volume;
        let mut q2 = make_quote("00700", 100.0);
        q2.volume = prev_vol + (rate * 3.0) as u64; // 正常量
        q2.timestamp = base_time + chrono::Duration::seconds(18);
        engine.process(&q2);

        // 再次放量 → 可重新触发
        let mut q3 = make_quote("00700", 100.0);
        q3.volume = q2.volume + (rate * 3.0 * 5.0) as u64;
        q3.timestamp = base_time + chrono::Duration::seconds(21);
        let sigs = engine.process(&q3);
        assert!(sigs.iter().any(|s| matches!(s, Signal::VolumeSpike { .. })),
            "should re-trigger after hysteresis reset");
    }

    #[test]
    fn test_vwap_skipped_for_index() {
        // 指数股票不应产生 VWAP 偏离信号
        let config = default_config();
        let mut engine = AnalysisEngine::new(&config);

        // SZ.399006 创业板指：turnover/volume 得到的是成分股均价，与指数点位不可比
        let mut q = make_quote("399006", 3328.0);
        q.code = StockCode::new(Market::SZ, "399006");
        q.turnover = 195_100_000_000.0;
        q.volume = 100_000_000;
        // warmup (config warmup_ticks=0, 但需要数据点)
        for _ in 0..3 {
            engine.process(&q);
        }
        let sigs = engine.process(&q);
        assert!(
            sigs.iter().all(|s| !matches!(s, Signal::VwapDeviation { .. })),
            "index stock should not produce VWAP signal"
        );
    }
}
