//! 提醒规则定义

use crate::models::{AlertSeverity, QuoteSnapshot, Signal, TechnicalIndicators};

/// 提醒规则 trait
pub trait AlertRule: Send + Sync {
    /// 规则名称
    fn name(&self) -> &str;

    /// 评估规则，返回 (是否触发, 消息, 级别)
    fn evaluate(
        &self,
        quote: &QuoteSnapshot,
        indicators: &TechnicalIndicators,
        signals: &[Signal],
    ) -> Option<(String, AlertSeverity)>;
}

/// 涨跌幅阈值规则
pub struct ChangeThresholdRule {
    /// 涨跌幅阈值 (%)
    pub threshold: f64,
}

impl ChangeThresholdRule {
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }
}

impl AlertRule for ChangeThresholdRule {
    fn name(&self) -> &str {
        "涨跌幅提醒"
    }

    fn evaluate(
        &self,
        quote: &QuoteSnapshot,
        _indicators: &TechnicalIndicators,
        _signals: &[Signal],
    ) -> Option<(String, AlertSeverity)> {
        let abs_change = quote.change_pct.abs();
        if abs_change >= self.threshold {
            let direction = if quote.change_pct > 0.0 { "涨" } else { "跌" };
            let severity = if abs_change >= self.threshold * 2.0 {
                AlertSeverity::Critical
            } else {
                AlertSeverity::Warning
            };
            Some((
                format!(
                    "{} {} {:.2}% (现价: {:.2})",
                    quote.name, direction, abs_change, quote.last_price
                ),
                severity,
            ))
        } else {
            None
        }
    }
}

/// 目标价规则
pub struct TargetPriceRule {
    /// 股票代码（用 display_code 匹配）
    pub stock_code: String,
    /// 上限价格
    pub upper: Option<f64>,
    /// 下限价格
    pub lower: Option<f64>,
}

impl TargetPriceRule {
    pub fn new(stock_code: String, upper: Option<f64>, lower: Option<f64>) -> Self {
        Self {
            stock_code,
            upper,
            lower,
        }
    }
}

impl AlertRule for TargetPriceRule {
    fn name(&self) -> &str {
        "目标价提醒"
    }

    fn evaluate(
        &self,
        quote: &QuoteSnapshot,
        _indicators: &TechnicalIndicators,
        _signals: &[Signal],
    ) -> Option<(String, AlertSeverity)> {
        if quote.code.display_code() != self.stock_code {
            return None;
        }

        if let Some(upper) = self.upper {
            if quote.last_price >= upper {
                return Some((
                    format!(
                        "{} 达到目标上限价 {:.2} (现价: {:.2})",
                        quote.name, upper, quote.last_price
                    ),
                    AlertSeverity::Critical,
                ));
            }
        }

        if let Some(lower) = self.lower {
            if quote.last_price <= lower {
                return Some((
                    format!(
                        "{} 跌破目标下限价 {:.2} (现价: {:.2})",
                        quote.name, lower, quote.last_price
                    ),
                    AlertSeverity::Critical,
                ));
            }
        }

        None
    }
}

/// 技术指标信号规则
pub struct SignalRule;

impl AlertRule for SignalRule {
    fn name(&self) -> &str {
        "技术信号提醒"
    }

    fn evaluate(
        &self,
        quote: &QuoteSnapshot,
        _indicators: &TechnicalIndicators,
        signals: &[Signal],
    ) -> Option<(String, AlertSeverity)> {
        if signals.is_empty() {
            return None;
        }

        let signal_strs: Vec<String> = signals.iter().map(|s| s.to_string()).collect();
        let severity = if signals.iter().any(|s| {
            matches!(
                s,
                Signal::MacdGoldenCross | Signal::MacdDeathCross
            )
        }) {
            AlertSeverity::Warning
        } else {
            AlertSeverity::Info
        };

        Some((
            format!("{} 技术信号: {}", quote.name, signal_strs.join(", ")),
            severity,
        ))
    }
}

/// 放量规则
pub struct VolumeSpikeRule {
    /// 放量倍数阈值
    pub ratio_threshold: f64,
}

impl AlertRule for VolumeSpikeRule {
    fn name(&self) -> &str {
        "放量提醒"
    }

    fn evaluate(
        &self,
        quote: &QuoteSnapshot,
        _indicators: &TechnicalIndicators,
        signals: &[Signal],
    ) -> Option<(String, AlertSeverity)> {
        for signal in signals {
            if let Signal::VolumeSpike { ratio } = signal {
                if *ratio >= self.ratio_threshold {
                    return Some((
                        format!(
                            "{} 成交放量 {:.1}倍 (现价: {:.2})",
                            quote.name, ratio, quote.last_price
                        ),
                        AlertSeverity::Warning,
                    ));
                }
            }
        }
        None
    }
}
