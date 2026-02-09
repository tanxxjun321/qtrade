//! 提醒管理器：规则评估 + 冷却机制

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{debug, info};

use crate::models::{AlertEvent, QuoteSnapshot};

use super::notify::Notifier;
use super::rules::AlertRule;

/// 提醒管理器
pub struct AlertManager {
    /// 注册的规则
    rules: Vec<Box<dyn AlertRule>>,
    /// 冷却记录：(股票代码, 规则名) → 上次触发时间
    cooldowns: HashMap<(String, String), Instant>,
    /// 冷却时间
    cooldown_duration: Duration,
    /// 通知器
    notifier: Notifier,
    /// 提醒历史
    history: Vec<AlertEvent>,
    /// 是否启用
    enabled: bool,
}

impl AlertManager {
    pub fn new(cooldown_secs: u64, notifier: Notifier) -> Self {
        Self {
            rules: Vec::new(),
            cooldowns: HashMap::new(),
            cooldown_duration: Duration::from_secs(cooldown_secs),
            notifier,
            history: Vec::new(),
            enabled: true,
        }
    }

    /// 添加规则
    pub fn add_rule(&mut self, rule: Box<dyn AlertRule>) {
        self.rules.push(rule);
    }

    /// 设置启用/禁用
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// 评估所有规则
    pub async fn evaluate(
        &mut self,
        quote: &QuoteSnapshot,
    ) -> Vec<AlertEvent> {
        if !self.enabled {
            return Vec::new();
        }

        let mut events = Vec::new();

        for rule in &self.rules {
            if let Some((message, severity)) = rule.evaluate(quote) {
                let key = (quote.code.display_code(), rule.name().to_string());

                // 检查冷却
                if let Some(last_trigger) = self.cooldowns.get(&key) {
                    if last_trigger.elapsed() < self.cooldown_duration {
                        debug!(
                            "Alert cooldown active for {} / {}",
                            quote.code, rule.name()
                        );
                        continue;
                    }
                }

                // 触发提醒
                let event = AlertEvent {
                    code: quote.code.clone(),
                    name: quote.name.clone(),
                    rule_name: rule.name().to_string(),
                    message,
                    triggered_at: chrono::Local::now(),
                    severity,
                };

                info!("Alert triggered: {} - {}", event.rule_name, event.message);

                // 发送通知
                self.notifier.send(&event).await;

                // 更新冷却
                self.cooldowns.insert(key, Instant::now());

                // 记录历史
                self.history.push(event.clone());
                events.push(event);
            }
        }

        // 清理过期的冷却记录
        self.cleanup_cooldowns();

        events
    }

    /// 获取最近的提醒历史
    pub fn recent_history(&self, count: usize) -> &[AlertEvent] {
        let start = self.history.len().saturating_sub(count);
        &self.history[start..]
    }

    /// 清理过期冷却记录
    fn cleanup_cooldowns(&mut self) {
        let duration = self.cooldown_duration * 2; // 保留 2 倍冷却时间后清理
        self.cooldowns
            .retain(|_, instant| instant.elapsed() < duration);
    }
}
