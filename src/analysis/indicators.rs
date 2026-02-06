//! 技术指标纯计算函数
//!
//! MA (Simple Moving Average), EMA, MACD, RSI

/// 计算简单移动平均线 (SMA)
/// 返回最后一个完整周期的 MA 值
pub fn sma(data: &[f64], period: usize) -> Option<f64> {
    if data.len() < period || period == 0 {
        return None;
    }
    let sum: f64 = data[data.len() - period..].iter().sum();
    Some(sum / period as f64)
}

/// 计算完整的 SMA 序列
pub fn sma_series(data: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut result = Vec::with_capacity(data.len());

    for i in 0..data.len() {
        if i + 1 < period {
            result.push(None);
        } else {
            let sum: f64 = data[i + 1 - period..=i].iter().sum();
            result.push(Some(sum / period as f64));
        }
    }

    result
}

/// 计算指数移动平均线 (EMA)
/// multiplier = 2 / (period + 1)
pub fn ema_series(data: &[f64], period: usize) -> Vec<Option<f64>> {
    if data.is_empty() || period == 0 {
        return vec![None; data.len()];
    }

    let mut result = Vec::with_capacity(data.len());
    let multiplier = 2.0 / (period as f64 + 1.0);

    // 前 period-1 个值为 None
    for _ in 0..period.saturating_sub(1).min(data.len()) {
        result.push(None);
    }

    if data.len() < period {
        return result;
    }

    // 第一个 EMA = 前 period 个值的 SMA
    let first_sma: f64 = data[..period].iter().sum::<f64>() / period as f64;
    result.push(Some(first_sma));

    let mut prev_ema = first_sma;
    for i in period..data.len() {
        let ema = (data[i] - prev_ema) * multiplier + prev_ema;
        result.push(Some(ema));
        prev_ema = ema;
    }

    result
}

/// 计算最新 EMA 值
pub fn ema(data: &[f64], period: usize) -> Option<f64> {
    let series = ema_series(data, period);
    series.last().copied().flatten()
}

/// MACD 计算结果
#[derive(Debug, Clone)]
pub struct MacdResult {
    /// DIF (快线 - 慢线 EMA)
    pub dif: Vec<Option<f64>>,
    /// DEA / Signal (DIF 的 EMA)
    pub dea: Vec<Option<f64>>,
    /// MACD 柱状图 (DIF - DEA) * 2
    pub histogram: Vec<Option<f64>>,
}

/// 计算 MACD
/// 标准参数：fast=12, slow=26, signal=9
pub fn macd(data: &[f64], fast: usize, slow: usize, signal: usize) -> MacdResult {
    let ema_fast = ema_series(data, fast);
    let ema_slow = ema_series(data, slow);

    // DIF = EMA(fast) - EMA(slow)
    let mut dif_values: Vec<f64> = Vec::new();
    let mut dif: Vec<Option<f64>> = Vec::with_capacity(data.len());

    for i in 0..data.len() {
        match (ema_fast.get(i).copied().flatten(), ema_slow.get(i).copied().flatten()) {
            (Some(f), Some(s)) => {
                let d = f - s;
                dif.push(Some(d));
                dif_values.push(d);
            }
            _ => {
                dif.push(None);
            }
        }
    }

    // DEA = EMA(DIF, signal)
    let dea_series = ema_series(&dif_values, signal);

    // 将 DEA 对齐回原始长度
    let dif_start = data.len() - dif_values.len();
    let mut dea: Vec<Option<f64>> = vec![None; dif_start];
    dea.extend(dea_series);

    // Histogram = (DIF - DEA) * 2
    let mut histogram: Vec<Option<f64>> = Vec::with_capacity(data.len());
    for i in 0..data.len() {
        match (dif.get(i).copied().flatten(), dea.get(i).copied().flatten()) {
            (Some(d), Some(e)) => histogram.push(Some((d - e) * 2.0)),
            _ => histogram.push(None),
        }
    }

    MacdResult {
        dif,
        dea,
        histogram,
    }
}

/// 计算最新 MACD 值 (dif, dea, histogram)
pub fn macd_latest(
    data: &[f64],
    fast: usize,
    slow: usize,
    signal: usize,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    let result = macd(data, fast, slow, signal);
    let dif = result.dif.last().copied().flatten();
    let dea = result.dea.last().copied().flatten();
    let hist = result.histogram.last().copied().flatten();
    (dif, dea, hist)
}

/// 计算 RSI (Relative Strength Index)
pub fn rsi_series(data: &[f64], period: usize) -> Vec<Option<f64>> {
    if data.len() < 2 || period == 0 {
        return vec![None; data.len()];
    }

    let mut result = vec![None; data.len()];

    // 计算价格变动
    let mut gains = Vec::with_capacity(data.len() - 1);
    let mut losses = Vec::with_capacity(data.len() - 1);

    for i in 1..data.len() {
        let change = data[i] - data[i - 1];
        if change > 0.0 {
            gains.push(change);
            losses.push(0.0);
        } else {
            gains.push(0.0);
            losses.push(-change);
        }
    }

    if gains.len() < period {
        return result;
    }

    // 第一个 RSI：简单平均
    let avg_gain: f64 = gains[..period].iter().sum::<f64>() / period as f64;
    let avg_loss: f64 = losses[..period].iter().sum::<f64>() / period as f64;

    let first_rsi = if avg_gain == 0.0 && avg_loss == 0.0 {
        None // 无波动，RSI 无意义
    } else if avg_loss == 0.0 {
        Some(100.0)
    } else {
        Some(100.0 - 100.0 / (1.0 + avg_gain / avg_loss))
    };
    result[period] = first_rsi;

    // 后续 RSI：指数平滑
    let mut prev_avg_gain = avg_gain;
    let mut prev_avg_loss = avg_loss;

    for i in period..gains.len() {
        prev_avg_gain = (prev_avg_gain * (period as f64 - 1.0) + gains[i]) / period as f64;
        prev_avg_loss = (prev_avg_loss * (period as f64 - 1.0) + losses[i]) / period as f64;

        let rsi = if prev_avg_gain == 0.0 && prev_avg_loss == 0.0 {
            None
        } else if prev_avg_loss == 0.0 {
            Some(100.0)
        } else {
            Some(100.0 - 100.0 / (1.0 + prev_avg_gain / prev_avg_loss))
        };
        result[i + 1] = rsi;
    }

    result
}

/// 计算最新 RSI 值
pub fn rsi(data: &[f64], period: usize) -> Option<f64> {
    let series = rsi_series(data, period);
    series.last().copied().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sma() {
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(sma(&data, 3), Some(4.0)); // (3+4+5)/3
        assert_eq!(sma(&data, 5), Some(3.0)); // (1+2+3+4+5)/5
        assert_eq!(sma(&data, 6), None);       // 数据不足
    }

    #[test]
    fn test_sma_series() {
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let result = sma_series(&data, 3);
        assert_eq!(result[0], None);
        assert_eq!(result[1], None);
        assert_eq!(result[2], Some(2.0));
        assert_eq!(result[3], Some(3.0));
        assert_eq!(result[4], Some(4.0));
    }

    #[test]
    fn test_ema() {
        let data = vec![10.0, 11.0, 12.0, 11.0, 13.0, 14.0, 12.0, 15.0];
        let result = ema(&data, 3);
        assert!(result.is_some());
    }

    #[test]
    fn test_macd() {
        // 生成足够的测试数据
        let data: Vec<f64> = (0..50).map(|i| 100.0 + (i as f64 * 0.5).sin() * 10.0).collect();
        let (dif, dea, hist) = macd_latest(&data, 12, 26, 9);
        assert!(dif.is_some());
        assert!(dea.is_some());
        assert!(hist.is_some());
    }

    #[test]
    fn test_rsi() {
        // 全部上涨 → RSI 应接近 100
        let data: Vec<f64> = (0..20).map(|i| 100.0 + i as f64).collect();
        let rsi_val = rsi(&data, 14).unwrap();
        assert!(rsi_val > 90.0);

        // 全部下跌 → RSI 应接近 0
        let data: Vec<f64> = (0..20).map(|i| 100.0 - i as f64).collect();
        let rsi_val = rsi(&data, 14).unwrap();
        assert!(rsi_val < 10.0);
    }

    #[test]
    fn test_rsi_range() {
        // RSI 应在 0-100 之间
        let data: Vec<f64> = vec![
            44.34, 44.09, 44.15, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08,
            45.89, 46.03, 44.72, 44.07, 44.17, 43.56, 44.65, 44.83,
        ];
        let rsi_val = rsi(&data, 14);
        if let Some(v) = rsi_val {
            assert!(v >= 0.0 && v <= 100.0, "RSI out of range: {}", v);
        }
    }
}
