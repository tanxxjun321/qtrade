# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

qtrade - 量化交易盯盘系统。从 macOS 上的富途牛牛 App 获取实时行情数据，支持港股和A股智能盯盘。

数据源三通道：
1. **macOS Accessibility API** — 直接读取 App 窗口 UI 元素
2. **FutuOpenD + OpenAPI** — TCP protobuf 连接获取结构化行情
3. **窗口截图 + Vision OCR** — CGWindowListCreateImage 截图 + VNRecognizeTextRequest 识别文字

详细计划见 `docs/PLAN.md`。

## Build & Development Commands

- `cargo build` - 构建项目
- `cargo test` - 运行所有测试（28 个单元测试）
- `cargo run -- watchlist` - 显示自选股列表（从富途 plist 读取）
- `cargo run -- start` - 启动盯盘系统（ratatui TUI）
- `cargo run -- test-api` - 测试 FutuOpenD 连接
- `cargo run -- debug` - 检查 AX 权限并打印 App 元素树
- `cargo run -- test-ocr` - 测试窗口截图 + Vision OCR 识别效果
- `cargo check` - 快速类型检查

## Architecture

### 模块结构

```
src/
├── main.rs                  # CLI 入口 (clap)：start / watchlist / debug / test-api / test-ocr
├── config.rs                # TOML 配置加载 (serde)
├── models.rs                # 核心数据模型：StockCode, QuoteSnapshot, Signal, DailyKline, TimedSignal, AlertEvent
├── futu/
│   ├── watchlist.rs         # 读取 plist 自选股（自动扫描用户目录）
│   ├── accessibility.rs     # macOS AXUIElement 读取 App 窗口
│   ├── ocr.rs               # 窗口截图 + Vision OCR 文字识别
│   └── openapi.rs           # FutuOpenD TCP 客户端（JSON 模式，含日K线 proto 3103）
├── data/
│   ├── provider.rs          # DataProviderKind 枚举分发（AX / OpenAPI / OCR）
│   └── parser.rs            # 文本 → QuoteSnapshot 解析
├── analysis/
│   ├── daily.rs             # 日K线分析引擎（JSON 缓存 + 增量更新 + MA/MACD/RSI 信号）
│   ├── indicators.rs        # SMA / EMA / MACD / RSI 纯计算
│   ├── engine.rs            # 滚动窗口 + 指标调度（Tick 级别）
│   └── signals.rs           # 金叉/死叉/超买超卖/放量检测
├── alerts/
│   ├── rules.rs             # 涨跌幅/目标价/信号/放量规则
│   ├── manager.rs           # 规则评估 + 冷却机制
│   └── notify.rs            # 终端 + macOS 通知 + Webhook
├── ui/
│   └── dashboard.rs         # ratatui TUI 仪表盘（含日线信号显示）
└── trading/
    └── paper.rs             # 纸上交易（预留）
```

### 数据流

```
数据源 → DataProviderKind → QuoteSnapshot
  → AnalysisEngine (Tick 指标计算)
  → AlertManager (规则评估 + 通知)
  → DashboardState (TUI 渲染)

日K线 → OpenAPI proto 3103 → DailyAnalysisEngine (日线指标 + 信号)
  → JSON 缓存 (~/.config/qtrade/kline_cache.json)
  → DashboardState (日线信号以 [日] 前缀显示)
```

组件间通过 `tokio::sync::mpsc` channel 通信。日K线通过独立 TCP 连接异步获取。

### OCR 数据源

- **截图**：`CGWindowListCreateImage` 截取富途牛牛窗口（支持被遮挡窗口，Retina 分辨率）
- **文字识别**：Apple Vision `VNRecognizeTextRequest`，语言 zh-Hans + en-US，精确模式
- **行分组**：按归一化 Y 坐标聚类（0.5% 容差），行内按 X 排序
- **解析**：拼接为 tab 分隔文本，复用 `try_parse_quote_text()` 解析
- **异步**：CG/Vision 同步 API 通过 `tokio::task::spawn_blocking` 运行

### 日K线分析

- **数据获取**：FutuOpenD proto 3103 (QOT_REQUEST_HISTORY_KL)，前复权，逐只拉取，200ms 间隔防限流
- **本地缓存**：JSON 文件 `~/.config/qtrade/kline_cache.json`，最多保留 150 天
- **增量更新**：缓存 ≤3 天时拉取最近 5 天增量合并；>3 天或无缓存时全量拉取 120 天
- **断点续传**：每拉取 10 只即存盘 + 同步 dashboard
- **市场过滤**：仅拉取已订阅市场的股票，跳过无权限品种
- **信号检测**：MA5/10/20/60 金叉死叉、MACD 金叉死叉、RSI6/12/24 超买超卖

### TUI 快捷键

- `↑↓` 选择行
- `s` 切换排序列
- `d` 切换日线信号显示/隐藏
- `i` 切换指标显示
- `q` 退出

### 关键数据路径

- 富途本地数据：`~/Library/Containers/cn.futu.Niuniu/Data/Library/Application Support/{user_id}/watchstockContainer.dat`
- 日K线缓存：`~/.config/qtrade/kline_cache.json`
- 价格精度：plist 整数 ÷ 10^11
- 股票编码：`1XXXXXX`=沪市, `2XXXXXX`=深市, 其他=港股

### 配置

配置文件：`config/config.toml`（参考 `config/config.toml.example`）

```toml
[data_source]
source = "accessibility"  # 或 "openapi" 或 "ocr"
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

### 工具链

- Rust 版本：1.93.0
- ratatui 0.29 + crossterm 0.28
- core-foundation 0.10
- edition 2021
