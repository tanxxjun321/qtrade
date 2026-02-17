//! 提醒管理器：穿越检测 + 日内去重 + 通知
//!
//! 只在 change_pct 从 < 阈值 穿越到 >= 阈值时触发，
//! 同股票 + 同规则 + 同方向一天只报一次，不会反复报警。

use std::collections::{HashMap, VecDeque};

use chrono::NaiveDate;
use tracing::{debug, info};

use crate::models::{AlertEvent, QuoteSnapshot, StockCode};

use super::notify::Notifier;
use super::rules::AlertRule;

/// 最大提醒历史记录数
const MAX_HISTORY: usize = 1000;

/// 提醒管理器
pub struct AlertManager {
    /// 注册的规则
    rules: Vec<Box<dyn AlertRule>>,
    /// 每只股票上一次的 change_pct（用于穿越检测）
    prev_change_pct: HashMap<StockCode, f64>,
    /// 日内去重：(股票, "规则名_方向") → 已触发日期
    fired_today: HashMap<(StockCode, String), NaiveDate>,
    /// 通知器
    notifier: Notifier,
    /// 提醒历史（循环缓冲区，最多保留 MAX_HISTORY 条）
    history: VecDeque<AlertEvent>,
    /// 是否启用
    enabled: bool,
}

impl AlertManager {
    pub fn new(notifier: Notifier) -> Self {
        Self {
            rules: Vec::new(),
            prev_change_pct: HashMap::new(),
            fired_today: HashMap::new(),
            notifier,
            history: VecDeque::with_capacity(MAX_HISTORY),
            enabled: true,
        }
    }

    /// 清理过期的 fired_today 条目（非今日日期）
    fn cleanup_old_entries(&mut self) {
        let today = chrono::Local::now().date_naive();
        self.fired_today.retain(|_, date| *date == today);
    }

    /// 添加规则
    pub fn add_rule(&mut self, rule: Box<dyn AlertRule>) {
        self.rules.push(rule);
    }

    /// 设置启用/禁用
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// 评估所有规则（穿越检测：仅在 change_pct 跨越阈值时触发）
    pub async fn evaluate(
        &mut self,
        quote: &QuoteSnapshot,
    ) -> Vec<AlertEvent> {
        if !self.enabled {
            return Vec::new();
        }

        // 清理过期条目（每 100 次评估清理一次，避免频繁操作）
        if self.history.len() % 100 == 0 {
            self.cleanup_old_entries();
        }

        let prev = self.prev_change_pct.insert(quote.code.clone(), quote.change_pct);
        let mut events = Vec::new();

        for rule in &self.rules {
            if let Some((message, severity, sentiment)) = rule.evaluate(quote) {
                // 穿越检测：首次见到的股票（无 prev）不触发，
                // 只有上一次 rule 不命中 → 本次命中 才算穿越
                let was_triggered = match prev {
                    Some(prev_pct) => {
                        // 构造一个伪快照用上一次的 change_pct 检测
                        let mut prev_quote = quote.clone();
                        prev_quote.change_pct = prev_pct;
                        rule.evaluate(&prev_quote).is_some()
                    }
                    None => true, // 首次见到，视为"已在阈值内"，不触发
                };

                if was_triggered {
                    debug!(
                        "Alert already active for {} / {}, skipping",
                        quote.code, rule.name()
                    );
                    continue;
                }

                // 日内去重：同股票 + 同规则 + 同方向，一天只报一次
                let today = chrono::Local::now().date_naive();
                let direction = match &sentiment {
                    Some(s) => format!("{}", s),
                    None => "none".to_string(),
                };
                let fire_key = (quote.code.clone(), format!("{}_{}", rule.name(), direction));
                if self.fired_today.get(&fire_key) == Some(&today) {
                    debug!(
                        "日内已报过 {} / {}，跳过",
                        quote.code, fire_key.1
                    );
                    continue;
                }

                // 穿越确认：上次未命中 → 本次命中 → 触发
                let event = AlertEvent {
                    code: quote.code.clone(),
                    name: quote.name.clone(),
                    rule_name: rule.name().to_string(),
                    message,
                    triggered_at: chrono::Local::now(),
                    severity,
                    sentiment,
                };

                info!("Alert triggered: {} - {}", event.rule_name, event.message);

                // 发送通知
                self.notifier.send(&event).await;

                // 记录历史 + 标记日内已触发
                self.fired_today.insert(fire_key, today);
                // 循环缓冲区：超过容量时移除最旧的
                if self.history.len() >= MAX_HISTORY {
                    self.history.pop_front();
                }
                self.history.push_back(event.clone());
                events.push(event);
            }
        }

        events
    }

    /// 获取最近的提醒历史
    pub fn recent_history(&self, count: usize) -> Vec<&AlertEvent> {
        self.history.iter().rev().take(count).collect()
    }

    /// 移除指定股票的所有数据（watchlist 变更时调用）
    pub fn remove_stock(&mut self, code: &StockCode) {
        self.prev_change_pct.remove(code);
        // 同时清理 fired_today 中该股票的所有条目
        self.fired_today.retain(|(c, _), _| c != code);
    }
}
