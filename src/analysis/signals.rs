//! 信号检测：金叉/死叉、超买/超卖

use crate::models::{Signal, TechnicalIndicators};

/// RSI 超买阈值
const RSI_OVERBOUGHT: f64 = 70.0;
/// RSI 超卖阈值
const RSI_OVERSOLD: f64 = 30.0;
/// 放量倍数阈值
const VOLUME_SPIKE_RATIO: f64 = 2.0;

/// 检测所有信号
pub fn detect_signals(
    current: &TechnicalIndicators,
    previous: Option<&TechnicalIndicators>,
    _prices: &[f64],
    volumes: &[u64],
) -> Vec<Signal> {
    let mut signals = Vec::new();

    if let Some(prev) = previous {
        // MA 金叉/死叉
        detect_ma_cross(current, prev, 5, 10, &mut signals);
        detect_ma_cross(current, prev, 5, 20, &mut signals);
        detect_ma_cross(current, prev, 10, 20, &mut signals);

        // MACD 金叉/死叉
        detect_macd_cross(current, prev, &mut signals);

    }

    // RSI 超买/超卖
    detect_rsi_signals(current, &mut signals);

    // 放量检测
    detect_volume_spike(volumes, &mut signals);

    signals
}

/// 检测 MA 金叉/死叉
fn detect_ma_cross(
    current: &TechnicalIndicators,
    previous: &TechnicalIndicators,
    short: usize,
    long: usize,
    signals: &mut Vec<Signal>,
) {
    let (cur_short, cur_long) = get_ma_pair(current, short, long);
    let (prev_short, prev_long) = get_ma_pair(previous, short, long);

    if let (Some(cs), Some(cl), Some(ps), Some(pl)) = (cur_short, cur_long, prev_short, prev_long)
    {
        // 金叉：短线从下方穿过长线
        if ps <= pl && cs > cl {
            signals.push(Signal::MaGoldenCross { short, long });
        }
        // 死叉：短线从上方穿过长线
        if ps >= pl && cs < cl {
            signals.push(Signal::MaDeathCross { short, long });
        }
    }
}

/// 获取指定周期的 MA 值对
fn get_ma_pair(
    ti: &TechnicalIndicators,
    short: usize,
    long: usize,
) -> (Option<f64>, Option<f64>) {
    let short_val = match short {
        5 => ti.ma5,
        10 => ti.ma10,
        20 => ti.ma20,
        60 => ti.ma60,
        _ => None,
    };
    let long_val = match long {
        5 => ti.ma5,
        10 => ti.ma10,
        20 => ti.ma20,
        60 => ti.ma60,
        _ => None,
    };
    (short_val, long_val)
}

/// 检测 MACD 金叉/死叉
fn detect_macd_cross(
    current: &TechnicalIndicators,
    previous: &TechnicalIndicators,
    signals: &mut Vec<Signal>,
) {
    if let (Some(cur_dif), Some(cur_dea), Some(prev_dif), Some(prev_dea)) = (
        current.macd_dif,
        current.macd_dea,
        previous.macd_dif,
        previous.macd_dea,
    ) {
        // 金叉：DIF 从下方穿过 DEA
        if prev_dif <= prev_dea && cur_dif > cur_dea {
            signals.push(Signal::MacdGoldenCross);
        }
        // 死叉：DIF 从上方穿过 DEA
        if prev_dif >= prev_dea && cur_dif < cur_dea {
            signals.push(Signal::MacdDeathCross);
        }
    }
}

/// 检测 RSI 超买/超卖
fn detect_rsi_signals(current: &TechnicalIndicators, signals: &mut Vec<Signal>) {
    if let Some(rsi6) = current.rsi6 {
        if rsi6 >= RSI_OVERBOUGHT {
            signals.push(Signal::RsiOverbought {
                period: 6,
                value: rsi6,
            });
        } else if rsi6 <= RSI_OVERSOLD {
            signals.push(Signal::RsiOversold {
                period: 6,
                value: rsi6,
            });
        }
    }

    if let Some(rsi12) = current.rsi12 {
        if rsi12 >= RSI_OVERBOUGHT {
            signals.push(Signal::RsiOverbought {
                period: 12,
                value: rsi12,
            });
        } else if rsi12 <= RSI_OVERSOLD {
            signals.push(Signal::RsiOversold {
                period: 12,
                value: rsi12,
            });
        }
    }
}

/// 扫描 MACD DIF/DEA 序列，检测最近一次 MS-MACD 动能拐点首日
///
/// 从后往前扫描最近 `lookback` 根 K 线，找到拐点的**首次发生日**：
/// - 卖出：当天满足 DIFF > DEA > 0 且 DIFF 缩短，但前一天不满足
/// - 买入：当天满足 DIFF < DEA < 0 且 |DIFF| 缩短，但前一天不满足
pub fn detect_ms_macd_from_series(
    dif: &[Option<f64>],
    dea: &[Option<f64>],
    lookback: usize,
) -> Vec<Signal> {
    let mut signals = Vec::new();
    let n = dif.len().min(dea.len());
    if n < 3 {
        return signals;
    }

    let start = (n.saturating_sub(lookback)).max(2);

    // 卖出条件：DIFF > DEA > 0 且 DIFF 缩短
    let is_sell = |i: usize| -> bool {
        match (dif[i], dea[i], dif.get(i.wrapping_sub(1)).copied().flatten()) {
            (Some(cd), Some(ce), Some(pd)) => {
                cd > 0.0 && ce > 0.0 && cd > ce && pd > 0.0 && cd < pd
            }
            _ => false,
        }
    };

    // 买入条件：DIFF < DEA < 0 且 |DIFF| 缩短
    let is_buy = |i: usize| -> bool {
        match (dif[i], dea[i], dif.get(i.wrapping_sub(1)).copied().flatten()) {
            (Some(cd), Some(ce), Some(pd)) => {
                cd < 0.0 && ce < 0.0 && cd < ce && pd < 0.0 && cd.abs() < pd.abs()
            }
            _ => false,
        }
    };

    // 从后往前扫描，找最近一次拐点首日（当天满足 + 前一天不满足）
    // 只在拐点首日恰好是最后一根K线（今天）时才触发信号
    let last = n - 1;
    for i in (start..n).rev() {
        if is_sell(i) && !is_sell(i - 1) {
            if i == last {
                signals.push(Signal::MsMacdSell);
            }
            break;
        }
    }

    for i in (start..n).rev() {
        if is_buy(i) && !is_buy(i - 1) {
            if i == last {
                signals.push(Signal::MsMacdBuy);
            }
            break;
        }
    }

    signals
}

/// 检测放量（最近一根 vs 前 N 根平均）
fn detect_volume_spike(volumes: &[u64], signals: &mut Vec<Signal>) {
    if volumes.len() < 6 {
        return;
    }

    let last = *volumes.last().unwrap() as f64;
    let avg: f64 = volumes[volumes.len() - 6..volumes.len() - 1]
        .iter()
        .map(|v| *v as f64)
        .sum::<f64>()
        / 5.0;

    if avg > 0.0 && last / avg >= VOLUME_SPIKE_RATIO {
        signals.push(Signal::VolumeSpike {
            ratio: last / avg,
            price: 0.0,
            delta: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ma_golden_cross() {
        let prev = TechnicalIndicators {
            ma5: Some(10.0),
            ma10: Some(11.0), // 短线在下
            ..Default::default()
        };
        let current = TechnicalIndicators {
            ma5: Some(12.0),
            ma10: Some(11.5), // 短线在上 → 金叉
            ..Default::default()
        };

        let signals = detect_signals(&current, Some(&prev), &[], &[]);
        assert!(signals.iter().any(|s| matches!(s, Signal::MaGoldenCross { short: 5, long: 10 })));
    }

    #[test]
    fn test_ma_death_cross() {
        let prev = TechnicalIndicators {
            ma5: Some(12.0),
            ma10: Some(11.0), // 短线在上
            ..Default::default()
        };
        let current = TechnicalIndicators {
            ma5: Some(10.0),
            ma10: Some(11.0), // 短线在下 → 死叉
            ..Default::default()
        };

        let signals = detect_signals(&current, Some(&prev), &[], &[]);
        assert!(signals.iter().any(|s| matches!(s, Signal::MaDeathCross { short: 5, long: 10 })));
    }

    #[test]
    fn test_rsi_overbought() {
        let current = TechnicalIndicators {
            rsi6: Some(75.0),
            ..Default::default()
        };

        let signals = detect_signals(&current, None, &[], &[]);
        assert!(signals.iter().any(|s| matches!(s, Signal::RsiOverbought { period: 6, .. })));
    }

    #[test]
    fn test_volume_spike() {
        let volumes = vec![100, 100, 100, 100, 100, 300]; // 3x 放量
        let current = TechnicalIndicators::default();

        let signals = detect_signals(&current, None, &[], &volumes);
        assert!(signals.iter().any(|s| matches!(s, Signal::VolumeSpike { .. })));
    }

    #[test]
    fn test_ms_macd_buy_on_turning_day() {
        // 拐点首日恰好是最后一根K线 → 应触发
        // bar 0: DIF=-2, DEA=-1  (|DIF|扩大中)
        // bar 1: DIF=-4, DEA=-2  (|DIF|扩大中)
        // bar 2: DIF=-5, DEA=-3  (|DIF|扩大中，峰值)
        // bar 3: DIF=-4, DEA=-3.5 (|DIF|缩短！首次拐点，且是最后一根) ← 触发
        let dif = vec![Some(-2.0), Some(-4.0), Some(-5.0), Some(-4.0)];
        let dea = vec![Some(-1.0), Some(-2.0), Some(-3.0), Some(-3.5)];

        let signals = detect_ms_macd_from_series(&dif, &dea, 5);
        assert!(signals.iter().any(|s| matches!(s, Signal::MsMacdBuy)));
        assert_eq!(signals.iter().filter(|s| matches!(s, Signal::MsMacdBuy)).count(), 1);
    }

    #[test]
    fn test_ms_macd_buy_not_on_turning_day() {
        // 拐点首日是 bar 3，但最后一根是 bar 4 → 不触发
        // bar 3 是首日，bar 4 继续缩短但不是首日
        let dif = vec![Some(-2.0), Some(-4.0), Some(-5.0), Some(-4.0), Some(-3.0)];
        let dea = vec![Some(-1.0), Some(-2.0), Some(-3.0), Some(-3.5), Some(-3.2)];

        let signals = detect_ms_macd_from_series(&dif, &dea, 5);
        assert!(!signals.iter().any(|s| matches!(s, Signal::MsMacdBuy)));
    }

    #[test]
    fn test_ms_macd_sell_on_turning_day() {
        // 拐点首日恰好是最后一根K线 → 应触发
        // bar 0: DIF=2, DEA=1
        // bar 1: DIF=4, DEA=2   (DIFF 扩大)
        // bar 2: DIF=5, DEA=3   (DIFF 扩大，峰值)
        // bar 3: DIF=4.5, DEA=3.5 (DIFF 缩短！首次拐点，且是最后一根) ← 触发
        let dif = vec![Some(2.0), Some(4.0), Some(5.0), Some(4.5)];
        let dea = vec![Some(1.0), Some(2.0), Some(3.0), Some(3.5)];

        let signals = detect_ms_macd_from_series(&dif, &dea, 5);
        assert!(signals.iter().any(|s| matches!(s, Signal::MsMacdSell)));
        assert_eq!(signals.iter().filter(|s| matches!(s, Signal::MsMacdSell)).count(), 1);
    }

    #[test]
    fn test_ms_macd_sell_not_on_turning_day() {
        // 拐点首日是 bar 3，最后一根是 bar 4 → 不触发
        let dif = vec![Some(2.0), Some(4.0), Some(5.0), Some(4.5), Some(4.0)];
        let dea = vec![Some(1.0), Some(2.0), Some(3.0), Some(3.5), Some(3.8)];

        let signals = detect_ms_macd_from_series(&dif, &dea, 5);
        assert!(!signals.iter().any(|s| matches!(s, Signal::MsMacdSell)));
    }

    #[test]
    fn test_ms_macd_no_signal_continuous_shrink() {
        // 全部连续缩短，没有拐点首日在 lookback 窗口内
        let dif = vec![Some(5.0), Some(4.0), Some(3.5)];
        let dea = vec![Some(3.0), Some(3.0), Some(2.8)];

        let signals = detect_ms_macd_from_series(&dif, &dea, 5);
        assert!(!signals.iter().any(|s| matches!(s, Signal::MsMacdSell)));
    }
}
