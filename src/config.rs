use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// 通用配置
    #[serde(default)]
    pub general: GeneralConfig,

    /// 数据源配置
    #[serde(default)]
    pub data_source: DataSourceConfig,

    /// 富途配置
    #[serde(default)]
    pub futu: FutuConfig,

    /// 提醒配置
    #[serde(default)]
    pub alerts: AlertsConfig,

    /// UI 配置
    #[serde(default)]
    pub ui: UiConfig,

    /// 分析配置
    #[serde(default)]
    pub analysis: AnalysisConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// 日志级别
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSourceConfig {
    /// 数据源类型: "accessibility" | "openapi"
    #[serde(default = "default_source")]
    pub source: String,

    /// 数据刷新间隔（秒）
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval_secs: u64,
}

impl Default for DataSourceConfig {
    fn default() -> Self {
        Self {
            source: default_source(),
            refresh_interval_secs: default_refresh_interval(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FutuConfig {
    /// 富途牛牛本地数据路径（留空则自动检测）
    pub data_path: Option<String>,

    /// 指定用户 ID（留空则自动选择最近活跃的）
    pub user_id: Option<String>,

    /// FutuOpenD 连接地址
    #[serde(default = "default_opend_host")]
    pub opend_host: String,

    /// FutuOpenD 连接端口
    #[serde(default = "default_opend_port")]
    pub opend_port: u16,
}

impl Default for FutuConfig {
    fn default() -> Self {
        Self {
            data_path: None,
            user_id: None,
            opend_host: default_opend_host(),
            opend_port: default_opend_port(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertsConfig {
    /// 是否启用提醒
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// 冷却时间（秒），同一规则不重复触发
    #[serde(default = "default_cooldown")]
    pub cooldown_secs: u64,

    /// 涨跌幅提醒阈值 (%)
    #[serde(default = "default_change_threshold")]
    pub change_threshold_pct: f64,

    /// Webhook URL（可选）
    pub webhook_url: Option<String>,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cooldown_secs: default_cooldown(),
            change_threshold_pct: default_change_threshold(),
            webhook_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// 表格每页显示行数
    #[serde(default = "default_page_size")]
    pub page_size: usize,

    /// 是否显示技术指标列
    #[serde(default = "default_true")]
    pub show_indicators: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            page_size: default_page_size(),
            show_indicators: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisConfig {
    /// 是否启用日K线分析
    #[serde(default = "default_true")]
    pub daily_kline_enabled: bool,

    /// 日K线获取天数（需 >= 120 以满足 MA60 + MACD 预热）
    #[serde(default = "default_daily_kline_days")]
    pub daily_kline_days: u32,

    /// 日K线刷新间隔（分钟），0 表示仅启动时获取
    #[serde(default = "default_daily_kline_refresh_minutes")]
    pub daily_kline_refresh_minutes: u64,

    /// VWAP 偏离触发阈值 (%)
    #[serde(default = "default_vwap_deviation_pct")]
    pub vwap_deviation_pct: f64,

    /// VWAP 偏离重置阈值 (%)
    #[serde(default = "default_vwap_reset_pct")]
    pub vwap_reset_pct: f64,

    /// 急涨急跌阈值 (%)
    #[serde(default = "default_rapid_move_pct")]
    pub rapid_move_pct: f64,

    /// 急涨急跌检测窗口 (快照数, ×2s)
    #[serde(default = "default_rapid_move_window")]
    pub rapid_move_window: u32,

    /// 振幅突破阈值 (%)
    #[serde(default = "default_amplitude_breakout_pct")]
    pub amplitude_breakout_pct: f64,

    /// 量能突变倍数阈值（增量成交量 / 窗口均值）
    #[serde(default = "default_volume_spike_ratio")]
    pub volume_spike_ratio: f64,

    /// 信号显示保持时间 (分钟)
    #[serde(default = "default_tick_signal_display_minutes")]
    pub tick_signal_display_minutes: u64,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            daily_kline_enabled: true,
            daily_kline_days: default_daily_kline_days(),
            daily_kline_refresh_minutes: default_daily_kline_refresh_minutes(),
            vwap_deviation_pct: default_vwap_deviation_pct(),
            vwap_reset_pct: default_vwap_reset_pct(),
            rapid_move_pct: default_rapid_move_pct(),
            rapid_move_window: default_rapid_move_window(),
            amplitude_breakout_pct: default_amplitude_breakout_pct(),
            volume_spike_ratio: default_volume_spike_ratio(),
            tick_signal_display_minutes: default_tick_signal_display_minutes(),
        }
    }
}

fn default_daily_kline_days() -> u32 {
    120
}

fn default_daily_kline_refresh_minutes() -> u64 {
    30
}

fn default_vwap_deviation_pct() -> f64 {
    2.0
}

fn default_vwap_reset_pct() -> f64 {
    1.0
}

fn default_rapid_move_pct() -> f64 {
    1.0
}

fn default_rapid_move_window() -> u32 {
    5
}

fn default_amplitude_breakout_pct() -> f64 {
    5.0
}

fn default_volume_spike_ratio() -> f64 {
    3.0
}

fn default_tick_signal_display_minutes() -> u64 {
    5
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_source() -> String {
    "accessibility".to_string()
}

fn default_refresh_interval() -> u64 {
    2
}

fn default_opend_host() -> String {
    "127.0.0.1".to_string()
}

fn default_opend_port() -> u16 {
    11111
}

fn default_true() -> bool {
    true
}

fn default_cooldown() -> u64 {
    300
}

fn default_change_threshold() -> f64 {
    3.0
}

fn default_page_size() -> usize {
    50
}

impl AppConfig {
    /// 从文件加载配置
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: AppConfig =
            toml::from_str(&content).with_context(|| "Failed to parse config TOML")?;
        Ok(config)
    }

    /// 从默认位置加载，如果不存在则使用默认配置
    pub fn load_or_default() -> Self {
        let candidates = [
            PathBuf::from("config/config.toml"),
            PathBuf::from("config.toml"),
            dirs_config_path(),
        ];

        for path in &candidates {
            if path.exists() {
                match Self::load(path) {
                    Ok(config) => {
                        tracing::info!("Loaded config from {}", path.display());
                        return config;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load config from {}: {}", path.display(), e);
                    }
                }
            }
        }

        tracing::info!("Using default configuration");
        Self::default()
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            data_source: DataSourceConfig::default(),
            futu: FutuConfig::default(),
            alerts: AlertsConfig::default(),
            ui: UiConfig::default(),
            analysis: AnalysisConfig::default(),
        }
    }
}

fn dirs_config_path() -> PathBuf {
    dirs_home().join(".config/qtrade/config.toml")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
