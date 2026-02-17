//! 提醒规则定义

use crate::models::{AlertSeverity, QuoteSnapshot, Sentiment};

/// 提醒规则 trait
pub trait AlertRule: Send + Sync {
    /// 规则名称
    fn name(&self) -> String;

    /// 评估规则，返回 (消息, 级别, 情绪方向)
    fn evaluate(&self, quote: &QuoteSnapshot) -> Option<(String, AlertSeverity, Option<Sentiment>)>;
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
    fn name(&self) -> String {
        format!("涨跌幅{}%", self.threshold)
    }

    fn evaluate(&self, quote: &QuoteSnapshot) -> Option<(String, AlertSeverity, Option<Sentiment>)> {
        let abs_change = quote.change_pct.abs();
        if abs_change >= self.threshold {
            let direction = if quote.change_pct > 0.0 { "涨" } else { "跌" };
            let severity = if abs_change >= self.threshold * 2.0 {
                AlertSeverity::Critical
            } else {
                AlertSeverity::Warning
            };
            let sentiment = if quote.change_pct > 0.0 {
                Sentiment::Bullish
            } else {
                Sentiment::Bearish
            };
            Some((
                format!(
                    "{} {} {:.2}% (现价: {:.2})",
                    quote.name, direction, abs_change, quote.last_price
                ),
                severity,
                Some(sentiment),
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
    fn name(&self) -> String {
        "目标价提醒".to_string()
    }

    fn evaluate(&self, quote: &QuoteSnapshot) -> Option<(String, AlertSeverity, Option<Sentiment>)> {
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
                    Some(Sentiment::Bullish),
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
                    Some(Sentiment::Bearish),
                ));
            }
        }

        None
    }
}
