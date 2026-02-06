# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

qtrade - 量化交易盯盘系统。从 macOS 上的富途牛牛 App 获取实时行情数据，支持港股和A股智能盯盘。

数据源双通道：
1. **macOS Accessibility API** — 直接读取 App 窗口 UI 元素
2. **FutuOpenD + OpenAPI** — TCP protobuf 连接获取结构化行情

详细计划见 `docs/PLAN.md`。

## Build & Development Commands

- `cargo build` - 构建项目
- `cargo test` - 运行所有测试（19 个单元测试）
- `cargo run -- watchlist` - 显示自选股列表（从富途 plist 读取）
- `cargo run -- start` - 启动盯盘系统（ratatui TUI）
- `cargo run -- debug` - 检查 AX 权限并打印 App 元素树
- `cargo check` - 快速类型检查

## Architecture

### 模块结构

```
src/
├── main.rs                  # CLI 入口 (clap)：start / watchlist / debug
├── config.rs                # TOML 配置加载 (serde)
├── models.rs                # 核心数据模型：StockCode, QuoteSnapshot, Signal, AlertEvent
├── futu/
│   ├── watchlist.rs         # 读取 plist 自选股（自动扫描用户目录）
│   ├── accessibility.rs     # macOS AXUIElement 读取 App 窗口
│   └── openapi.rs           # FutuOpenD TCP 客户端（JSON 模式）
├── data/
│   ├── provider.rs          # DataProviderKind 枚举分发（AX / OpenAPI）
│   └── parser.rs            # 文本 → QuoteSnapshot 解析
├── analysis/
│   ├── indicators.rs        # SMA / EMA / MACD / RSI 纯计算
│   ├── engine.rs            # 滚动窗口 + 指标调度
│   └── signals.rs           # 金叉/死叉/超买超卖/放量检测
├── alerts/
│   ├── rules.rs             # 涨跌幅/目标价/信号/放量规则
│   ├── manager.rs           # 规则评估 + 冷却机制
│   └── notify.rs            # 终端 + macOS 通知 + Webhook
├── ui/
│   └── dashboard.rs         # ratatui TUI 仪表盘
└── trading/
    └── paper.rs             # 纸上交易（预留）
```

### 数据流

```
数据源 → DataProviderKind → QuoteSnapshot
  → AnalysisEngine (指标计算)
  → AlertManager (规则评估 + 通知)
  → DashboardState (TUI 渲染)
```

组件间通过 `tokio::sync::mpsc` channel 通信。

### 关键数据路径

- 富途本地数据：`~/Library/Containers/cn.futu.Niuniu/Data/Library/Application Support/{user_id}/watchstockContainer.dat`
- 价格精度：plist 整数 ÷ 10^11
- 股票编码：`1XXXXXX`=沪市, `2XXXXXX`=深市, 其他=港股

### 配置

配置文件：`config/config.toml`（参考 `config/config.toml.example`）

```toml
[data_source]
source = "accessibility"  # 或 "openapi"
refresh_interval_secs = 2

[futu]
opend_host = "127.0.0.1"
opend_port = 11111

[alerts]
change_threshold_pct = 3.0
cooldown_secs = 300
```

### 工具链

- Rust 版本：1.93.0
- ratatui 0.29 + crossterm 0.28
- core-foundation 0.10
- edition 2021
