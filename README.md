# qtrade

量化交易盯盘系统 — 从 macOS 富途牛牛 App 获取实时行情，终端内完成多市场智能监控与技术分析。

## 特性

- **三通道数据源**，按需切换、互为备份：
  - **Accessibility API** — 直接读取 App 窗口 UI 元素，零延迟
  - **FutuOpenD OpenAPI** — TCP protobuf 结构化行情，支持 K 线和实时推送
  - **截图 + Vision OCR** — 窗口截图 + Apple Vision 文字识别，AX 辅助布局检测
- **多市场支持**：港股、沪深 A 股、美股（含盘前/盘后/夜盘时段）、新加坡、外汇
- **技术指标**：MA5/10/20/60、MACD、RSI6/12/24，Tick 级别实时计算
- **日 K 线分析**：自适应增量拉取、JSON 本地缓存、断点续传、MA/MACD/RSI 日线信号
- **智能提醒**：涨跌幅阈值、目标价、指标信号、放量检测，冷却去重，支持 macOS 通知和 Webhook
- **终端仪表盘**：ratatui TUI，排序、指标显示切换、日线信号叠加

## 环境要求

- **macOS**（依赖 Accessibility API / Core Graphics / Vision 框架）
- **Rust 1.70+**
- **富途牛牛 App**（已登录，自选股列表非空）
- FutuOpenD（仅 `openapi` 数据源和日 K 线分析需要）

## 快速开始

```bash
# 克隆并构建
git clone https://github.com/tanxxjun321/qtrade.git
cd qtrade
cargo build --release

# 复制配置文件并按需修改
cp config/config.toml.example config/config.toml

# 查看自选股列表（验证 plist 读取）
cargo run -- watchlist

# 启动盯盘系统
cargo run -- start
```

首次运行时 macOS 会弹出 Accessibility 权限请求，需在「系统设置 → 隐私与安全性 → 辅助功能」中授权终端应用。

## 命令

| 命令 | 说明 |
|------|------|
| `qtrade start` | 启动盯盘系统（TUI 仪表盘） |
| `qtrade watchlist` | 显示自选股列表（从富途 plist 读取） |
| `qtrade debug` | 检查 AX 权限并打印 App 元素树 |
| `qtrade test-api` | 测试 FutuOpenD 连接 |
| `qtrade test-ocr` | 测试截图 + OCR 识别效果 |

通用参数：`-c <path>` 指定配置文件路径。

## 快捷键

| 按键 | 功能 |
|------|------|
| `↑` / `↓` | 选择行 |
| `s` | 切换排序列（代码/名称/价格/涨跌幅/成交量） |
| `d` | 显示/隐藏日线信号 |
| `i` | 显示/隐藏技术指标 |
| `q` | 退出 |

## 配置

配置文件 `config/config.toml`，参考 [`config/config.toml.example`](config/config.toml.example)：

```toml
[data_source]
source = "ocr"                # "accessibility" | "openapi" | "ocr"
refresh_interval_secs = 2

[futu]
opend_host = "127.0.0.1"
opend_port = 11111

[alerts]
change_threshold_pct = 3.0
cooldown_secs = 300

[analysis]
daily_kline_enabled = true
daily_kline_days = 120
daily_kline_refresh_minutes = 30
```

## 架构

```
数据源（AX / OpenAPI / OCR）
  → DataProviderKind → QuoteSnapshot
    → AnalysisEngine（Tick 指标）+ DailyAnalysisEngine（日线信号）
    → AlertManager（规则评估 + 通知）
    → DashboardState（ratatui TUI 渲染）
```

```
src/
├── main.rs              # CLI 入口
├── config.rs            # 配置加载
├── models.rs            # 核心数据模型
├── futu/                # 数据源：watchlist / accessibility / ocr / openapi
├── data/                # 数据分发与解析
├── analysis/            # 技术指标与信号检测（Tick + 日线）
├── alerts/              # 提醒规则、冷却、通知
├── ui/                  # ratatui 仪表盘
└── trading/             # 交易模块（预留）
```

详细设计见 [`docs/PLAN.md`](docs/PLAN.md)。

## 文档

- [实施计划](docs/PLAN.md)
- [日 K 线缓存策略](docs/DAILY_KLINE_CACHE.md)
- [美股交易时段](docs/US_MARKET_SESSIONS.md)

## License

MIT
