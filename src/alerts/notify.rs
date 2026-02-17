//! 通知渠道：终端输出 + Webhook

use tracing::{error, info};

use crate::models::{AlertEvent, AlertSeverity};

/// 通知器
pub struct Notifier {
    /// Webhook URL
    webhook_url: Option<String>,
    /// HTTP 客户端
    http_client: reqwest::Client,
}

impl Notifier {
    pub fn new(webhook_url: Option<String>) -> Self {
        Self {
            webhook_url,
            http_client: reqwest::Client::new(),
        }
    }

    /// 发送通知
    pub async fn send(&self, event: &AlertEvent) {
        // 终端通知（总是启用）
        self.send_terminal(event);

        // macOS 系统通知
        self.send_macos_notification(event);

        // Webhook 通知
        if let Some(url) = &self.webhook_url {
            self.send_webhook(url, event).await;
        }
    }

    /// 终端输出通知
    fn send_terminal(&self, event: &AlertEvent) {
        let severity_icon = match event.severity {
            AlertSeverity::Info => "ℹ️ ",
            AlertSeverity::Warning => "⚠️ ",
            AlertSeverity::Critical => "🚨",
        };

        // 终端响铃
        if event.severity == AlertSeverity::Critical {
            print!("\x07"); // BEL
        }

        info!(
            "{} [{}] {} | {}",
            severity_icon, event.code, event.rule_name, event.message
        );
    }

    /// macOS 系统通知
    fn send_macos_notification(&self, event: &AlertEvent) {
        let title = format!("qtrade - {}", event.rule_name);
        let message = &event.message;

        // 使用 osascript 发送系统通知
        let script = format!(
            r#"display notification "{}" with title "{}""#,
            message.replace('"', r#"\""#),
            title.replace('"', r#"\""#),
        );

        let _ = std::process::Command::new("osascript").args(["-e", &script]).spawn();
    }

    /// Webhook 通知（支持飞书/Slack 格式）
    async fn send_webhook(&self, url: &str, event: &AlertEvent) {
        let severity_text = match event.severity {
            AlertSeverity::Info => "信息",
            AlertSeverity::Warning => "警告",
            AlertSeverity::Critical => "紧急",
        };

        // 通用 JSON payload（兼容飞书和 Slack）
        let payload = if url.contains("feishu") || url.contains("lark") {
            // 飞书格式
            serde_json::json!({
                "msg_type": "text",
                "content": {
                    "text": format!(
                        "[{}] {} | {} | {}\n时间: {}",
                        severity_text,
                        event.code,
                        event.rule_name,
                        event.message,
                        event.triggered_at.format("%Y-%m-%d %H:%M:%S")
                    )
                }
            })
        } else {
            // Slack 格式
            serde_json::json!({
                "text": format!(
                    "*[{}]* `{}` | {} | {}\n_时间: {}_",
                    severity_text,
                    event.code,
                    event.rule_name,
                    event.message,
                    event.triggered_at.format("%Y-%m-%d %H:%M:%S")
                )
            })
        };

        match self.http_client.post(url).json(&payload).send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    error!("Webhook failed: HTTP {}", resp.status());
                }
            }
            Err(e) => {
                error!("Webhook error: {}", e);
            }
        }
    }
}
