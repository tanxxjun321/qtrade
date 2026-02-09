# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

qtrade - 量化交易盯盘系统。从 macOS 上的富途牛牛 App 获取实时行情数据，支持港股、A股、美股、新加坡、外汇等多市场智能盯盘。

数据源三通道：
1. **macOS Accessibility API** — 直接读取 App 窗口 UI 元素
2. **FutuOpenD + OpenAPI** — TCP protobuf 连接获取结构化行情
3. **窗口截图 + Vision OCR** — CGWindowListCreateImage 截图 + VNRecognizeTextRequest 识别文字

详细计划见 `docs/PLAN.md`。

## Build & Development Commands

- `cargo build` - 构建项目
- `cargo test` - 运行所有测试（37 个单元测试）
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
├── models.rs                # 核心数据模型：StockCode, Market, QuoteSnapshot, Signal, Sentiment, DailyKline, TimedSignal, AlertEvent, UsMarketSession
├── futu/
│   ├── watchlist.rs         # 读取 plist 自选股（自动扫描用户目录）
│   ├── accessibility.rs     # macOS AXUIElement 读取 App 窗口 + AX 表格 frame 检测
│   ├── ocr.rs               # 窗口截图 + Vision OCR 文字识别
│   └── openapi.rs           # FutuOpenD TCP 客户端（JSON 模式，含日K线 proto 3103）
├── data/
│   ├── provider.rs          # DataProviderKind 枚举分发（AX / OpenAPI / OCR）
│   └── parser.rs            # 文本 → QuoteSnapshot 解析
├── analysis/
│   ├── daily.rs             # 日K线分析引擎（JSON 缓存 + 增量更新 + MA/MACD/RSI 信号）
│   ├── indicators.rs        # SMA / EMA / MACD / RSI 纯计算
│   ├── engine.rs            # 事件型 tick 信号检测（VWAP偏离/新高新低/急涨急跌/振幅突破/量能突变）
│   └── signals.rs           # 金叉/死叉/超买超卖/放量检测（供日线引擎使用）
├── alerts/
│   ├── rules.rs             # 涨跌幅/目标价规则
│   ├── manager.rs           # 规则评估 + 冷却机制
│   └── notify.rs            # 终端 + macOS 通知 + Webhook
├── ui/
│   └── dashboard.rs         # ratatui TUI 仪表盘（含 tick 事件信号 + 日线信号 + 情绪标签显示）
└── trading/
    └── paper.rs             # 纸上交易（预留）
```

### 数据流

```
数据源 → DataProviderKind → QuoteSnapshot
  → AnalysisEngine (事件型 tick 信号：VWAP偏离/新高新低/急涨急跌/振幅突破/量能突变)
  → AlertManager (涨跌幅规则评估 + 通知)
  → DashboardState (TUI 渲染：tick 信号带 5 分钟时间衰减)

Tick 信号设计原则：
  事件型（触发一次后保持显示），非状态型（避免每 tick 翻转）
  所有信号标注情绪方向：[利多]/[利空]/[中性]
  滞后重置机制防止噪声（如 VWAP 偏离回到 reset 阈值才可再次触发）

OCR 管线（OcrProvider）：
  CGWindowList → owner_pid → AX API → GridFrame（归一化坐标）
  截图 → [有 GridFrame: 跳过 Pass 1, 裁剪 X+Y] / [无: Pass 1 快速 OCR → 裁剪 X]
  → Pass 2 精确 OCR → 分行 → 解析 → QuoteSnapshot

日K线 → OpenAPI proto 3103 → DailyAnalysisEngine (日线指标 + 信号)
  → JSON 缓存 (~/.config/qtrade/kline_cache.json)
  → DashboardState (日线信号以 [日利多]/[日利空]/[日中性] 前缀显示)
```

组件间通过 `tokio::sync::mpsc` channel 通信。日K线通过独立 TCP 连接异步获取。

### OCR 数据源

- **布局检测**：优先通过 AX API 获取 FTVGridView 精确 frame（identifier: `accessibility.futu.FTQWatchStocksViewController`），跳过 Pass 1 快速 OCR；AX 失败时降级为 Pass 1 关键词布局检测
- **截图**：`CGWindowListCreateImage` 截取富途牛牛窗口（支持被遮挡窗口，Retina 分辨率）
- **裁剪**：有 AX frame 时同时裁剪 X + Y（排除表头和侧边栏噪声），无 AX 时仅裁剪 X
- **文字识别**：Apple Vision `VNRecognizeTextRequest`，语言 zh-Hans + en-US，精确模式
- **行分组**：按归一化 Y 坐标聚类（0.5% 容差），行内按 X 排序
- **解析**：拼接为 tab 分隔文本，复用 `try_parse_quote_text()` 解析
- **异步**：CG/Vision 同步 API 通过 `tokio::task::spawn_blocking` 运行
- **PID 处理**：`pgrep` 可能找到辅助进程 PID，通过 `CGWindowListCopyWindowInfo` 获取实际 GUI `owner_pid` 用于 AX API

### 日K线分析

- **数据获取**：FutuOpenD proto 3103 (QOT_REQUEST_HISTORY_KL)，前复权，逐只拉取，200ms 间隔防限流
- **本地缓存**：JSON 文件 `~/.config/qtrade/kline_cache.json`，最多保留 150 天
- **逐只自适应拉取**：每只股票独立判断 — 无缓存→全量；有缓存→按 gap 自适应天数拉取，拉取后验证与缓存尾部日期重叠确认连续性；无重叠→丢弃旧缓存，全量重拉
- **断点续传**：每拉取 10 只即存盘 + 同步 dashboard
- **市场权限**：运行时检测（非依赖订阅状态），无权限市场整体跳过
- **信号检测**：MA5/10/20/60 金叉死叉、MACD 金叉死叉、RSI6/12/24 超买超卖
- **详细策略**：见 `docs/DAILY_KLINE_CACHE.md`

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
- 股票编码：`1XXXXXX`=沪市, `2XXXXXX`=深市, 其他=港股；美股/新加坡/外汇由 OCR 代码模式推断

### 配置

配置文件：`config/config.toml`（参考 `config/config.toml.example`）

```toml
[data_source]
source = "ocr"  # "accessibility" | "openapi" | "ocr"
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
# Tick 信号阈值
vwap_deviation_pct = 2.0        # VWAP 偏离触发阈值 (%)
vwap_reset_pct = 1.0            # VWAP 偏离重置阈值 (%)
rapid_move_pct = 1.0            # 急涨急跌阈值 (%)
rapid_move_window = 5           # 急涨急跌检测窗口 (快照数)
amplitude_breakout_pct = 5.0    # 振幅突破阈值 (%)
volume_spike_ratio = 3.0        # 量能突变倍数阈值
tick_signal_display_minutes = 5 # 信号显示保持时间 (分钟)
```

### 支持市场

- **港股 (HK)** — 5 位代码（如 00700）
- **沪市 A 股 (SH)** — 6 位 6xx（如 600519）
- **深市 A 股 (SZ)** — 6 位 0xx/3xx（如 000001）
- **美股 (US)** — 字母代码（如 AAPL、.IXIC）；支持盘前/盘后/夜盘时段显示（详见 `docs/US_MARKET_SESSIONS.md`）
- **新加坡 (SG)** — 字母+数字代码
- **外汇 (FX)** — 货币对代码（如 USDCNH）

### 工具链

- Rust 版本：1.93.0
- ratatui 0.29 + crossterm 0.28
- core-foundation 0.10 + objc2 0.6
- chrono 0.4 + chrono-tz 0.10（美股时段 DST 处理）
- edition 2021
