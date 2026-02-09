# qtrade 量化交易盯盘系统 - 实施计划

## Context

开发一个量化交易监控程序，从 macOS 上运行的富途牛牛 App 获取实时行情数据，支持港股、A股、美股、新加坡、外汇等多市场智能盯盘。支持三种数据源通道互补。

数据源三通道：
1. **macOS Accessibility API** — 直接从 App 窗口 UI 元素读取实时行情（无需额外软件）
2. **FutuOpenD + OpenAPI** — 通过官方网关获取结构化行情数据（更稳定，支持 K 线/订单簿等丰富数据）
3. **窗口截图 + Vision OCR** — CGWindowListCreateImage 截图 + VNRecognizeTextRequest 识别文字，AX API 辅助布局检测

技术选型：**Rust** + macOS 底层框架（objc2 生态） + ratatui 终端展示。

### 已验证的本地数据

富途牛牛 App 本地数据基础路径：`~/Library/Containers/cn.futu.Niuniu/Data/Library/Application Support/`

自选股文件路径：`{base_path}/{user_id}/watchstockContainer.dat`
- `{user_id}` 是富途用户 ID（数字目录），**不是固定值**
- 当前系统发现多个用户目录：`0`, `27148251`, `35138101`, `542003`
- 程序需**自动扫描**数字目录，找到含有 `watchstockContainer.dat` 的目录
- 策略：选择修改时间最新的那个（即最近活跃的账号），或让用户在配置中指定
- 文件格式：XML plist
- 价格格式：高精度整数（如 `3875530400000` → 实际价格需除以 10^11）
- 股票 ID 编码：`1XXXXXX` = 沪市, `2XXXXXX` = 深市, `800XXX` = 港股指数

## 项目结构

```
qtrade/
├── Cargo.toml
├── CLAUDE.md
├── docs/
│   ├── PLAN.md                     # 本文件
│   ├── DAILY_KLINE_CACHE.md        # 日K线缓存与增量拉取策略
│   └── US_MARKET_SESSIONS.md       # 美股交易时段与显示规则
├── config/
│   └── config.toml.example
├── src/
│   ├── main.rs                     # 入口 + CLI (clap): start(默认) / watchlist / debug / test-api / test-ocr
│   ├── config.rs                   # TOML 配置加载 (serde)
│   ├── models.rs                   # 核心数据模型（Market, StockCode, QuoteSnapshot, Signal(含MsMacdBuy/Sell), UsMarketSession 等）
│   ├── futu/
│   │   ├── mod.rs
│   │   ├── watchlist.rs            # 读取 App 本地 plist 自选股列表
│   │   ├── accessibility.rs        # macOS AXUIElement 读取 App 窗口 + AX 表格 frame 检测
│   │   ├── ocr.rs                  # 窗口截图 + Vision OCR 文字识别（含美股时段 + 防抖）
│   │   └── openapi.rs              # FutuOpenD TCP 客户端（JSON 模式，含日K线 proto 3103）
│   ├── data/
│   │   ├── mod.rs
│   │   ├── provider.rs             # DataProviderKind 枚举分发（AX / OpenAPI / OCR）
│   │   └── parser.rs               # 从原始文本/AX值解析为 QuoteSnapshot
│   ├── analysis/
│   │   ├── mod.rs
│   │   ├── daily.rs                # 日K线分析引擎（JSON 缓存 + 增量更新 + MA/MACD/RSI/MS-MACD 信号）
│   │   ├── engine.rs               # 事件型 tick 信号检测（VWAP偏离/新高新低/急涨急跌/振幅突破/量能突变）
│   │   ├── indicators.rs           # SMA / EMA / MACD / RSI 纯计算
│   │   └── signals.rs              # 信号判定（金叉/死叉、超买超卖、放量、MS-MACD拐点检测，供日线引擎使用）
│   ├── alerts/
│   │   ├── mod.rs
│   │   ├── manager.rs              # 规则评估 + 冷却机制
│   │   ├── rules.rs                # 涨跌幅、目标价规则
│   │   └── notify.rs               # 通知渠道（终端 + Webhook）
│   ├── ui/
│   │   ├── mod.rs
│   │   └── dashboard.rs            # ratatui TUI 仪表盘（含 tick 事件信号 + 日线信号 + 情绪标签显示）
│   └── trading/                    # 预留交易模块（第一阶段仅 trait 定义）
│       ├── mod.rs
│       └── paper.rs
└── tests/                        # 单元测试内嵌于各模块中（37 个）
```

## 数据流架构

```
数据源（可切换，source = "accessibility" | "openapi" | "ocr"）：

方案 A: Accessibility API    方案 B: FutuOpenD + OpenAPI    方案 C: 截图 + OCR
(accessibility.rs)           (openapi.rs)                   (ocr.rs + accessibility.rs)
  │ 读取 App UI 元素文本         │ TCP protobuf localhost:11111   │ CGWindowList 截图
  │ 无需额外软件                 │ K线/订单簿/实时推送              │ AX API 检测 GridFrame
  │                             │                               │ Vision OCR 文字识别
  └──────────┬──────────────────┼───────────────────────────────┘
             │
       DataProviderKind (provider.rs)
             │
       parser.rs → QuoteSnapshot
             │
    ┌────────┼────────┐
    │        │        │
AlertMgr  Analysis  Dashboard
(涨跌幅   Engine    (ratatui)
 提醒)    (事件型tick信号:
           VWAP/新高新低/
           急涨急跌/振幅/量能)
             │
    Dashboard ← tick_signals (带情绪标签+时间衰减)

watchlist.rs：从 plist 读取自选股列表（三种方案共用）
```

### 线程模型（tokio async）

- **数据采集任务**：定时（每 1-2 秒）通过 AX API 或 OpenAPI 获取最新行情
- **分析任务**：收到新数据后计算指标、评估规则
- **UI 渲染**：主线程运行 ratatui 事件循环，通过 channel 接收数据更新
- 组件间通信：`tokio::sync::mpsc` channel

## 关键设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 数据获取方案 A | macOS Accessibility API | 直接读 UI 元素文本，无需额外软件 |
| 数据获取方案 B | FutuOpenD + OpenAPI (TCP protobuf) | 结构化数据，支持 K 线/订单簿/实时推送 |
| 自选股来源 | 读 App 本地 plist 文件 | 已验证可行，免配置 |
| 异步运行时 | tokio | Rust 标准异步运行时 |
| 终端 UI | ratatui + crossterm | Rust 生态最成熟的 TUI 框架 |
| 配置格式 | TOML | Rust 原生支持，serde 集成好 |
| macOS 互操作 | objc2 系列 crate | 当前标准，替代已归档的 icrate |

## 核心依赖 (Cargo.toml)

```toml
[dependencies]
# macOS 系统 API
objc2 = "0.6"
objc2-foundation = "0.3"
objc2-app-kit = "0.3"
objc2-core-foundation = { version = "0.3", features = ["CFCGTypes", "CFArray", "CFDictionary"] }
objc2-core-graphics = { version = "0.3", features = ["CGImage", "CGWindow", "CGGeometry", "CGColorSpace"] }
objc2-vision = { version = "0.3", features = ["VNRecognizeTextRequest", "VNRequestHandler", "VNObservation", "VNRequest", "VNTypes"] }
core-foundation = "0.10"
# FutuOpenD OpenAPI
prost = "0.13"
prost-types = "0.13"
tokio-util = { version = "0.7", features = ["codec"] }
bytes = "1"
# 数据与配置
plist = "1"
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
# 异步 + UI + CLI
tokio = { version = "1", features = ["full"] }
ratatui = "0.29"
crossterm = "0.28"
clap = { version = "4", features = ["derive"] }
reqwest = { version = "0.12", features = ["json"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
# 时间 + 工具
chrono = { version = "0.4", features = ["serde"] }
chrono-tz = "0.10"
anyhow = "1"
sha1 = "0.10"

[build-dependencies]
prost-build = "0.13"
```

## 实施步骤

### Step 1: 项目骨架 + plist 自选股读取 ✅
- `cargo init`，配置 `Cargo.toml`
- 实现 `config.rs`（TOML 加载）和 `models.rs`（QuoteSnapshot, StockCode 等）
- 实现 `futu/watchlist.rs`：解析 `watchstockContainer.dat` plist，提取股票代码和缓存价格
- 验证：运行程序输出当前自选股列表

### Step 2: Accessibility API 数据获取 ✅
- 实现 `futu/accessibility.rs`：
  - 通过 `AXUIElementCreateApplication` 获取富途 App 进程
  - 遍历窗口 → 子元素树，定位股票行情表格区域
  - 读取 AXValue/AXTitle 等属性提取价格文本
  - AX API 检测 FTVGridView frame（辅助 OCR 布局）
- 实现 `data/parser.rs`：从原始文本解析出股票代码、价格、涨跌幅

### Step 3: FutuOpenD OpenAPI 数据源 ✅
- 实现 `futu/openapi.rs`：
  - TCP 连接 FutuOpenD (localhost:11111)
  - Futu 协议帧格式：固定头部 + protobuf body
  - 实现 InitConnect、GetGlobalState、Sub（订阅行情）、GetBasicQot（获取报价）
  - 实时推送回调处理
  - 日K线 proto 3103 (QOT_REQUEST_HISTORY_KL)
- 实现 `data/provider.rs`：DataProvider trait

### Step 4: 终端仪表盘 ✅
- 实现 `ui/dashboard.rs`：ratatui 实时表格展示自选股行情（含日线信号）
- 实现 `main.rs` CLI (clap)

### Step 5: 技术指标分析 ✅
- 实现 `analysis/indicators.rs`：MA（多周期）、MACD、RSI 纯计算函数
- 实现 `analysis/engine.rs`：事件型 tick 信号检测引擎（VWAP偏离、日内新高/新低、急涨急跌、振幅突破、量能突变），带滞后重置防翻转
- 实现 `analysis/signals.rs`：金叉/死叉、超买/超卖、MS-MACD动能拐点信号判定（供日线引擎使用）
- 实现 `analysis/daily.rs`：日K线分析引擎（JSON 缓存 + 逐只自适应拉取 + 断点续传）
- 信号情绪标签：所有信号标注 Sentiment（利多/利空/中性），dashboard 显示 `[利多]日内新高`、`[日利空]MACD 死叉` 等
- MS-MACD 动能拐点信号：扫描 DIF/DEA 序列，仅在拐点首日（最后一根K线）触发买入/卖出信号
- 单元测试（48 个）

### Step 6: 提醒系统 ✅
- 实现 `alerts/rules.rs`：涨跌幅阈值、目标价规则（已移除噪声源 SignalRule / VolumeSpikeRule）
- 实现 `alerts/manager.rs`：规则评估 + 冷却机制（简化签名，不再传入 signals/indicators）
- 实现 `alerts/notify.rs`：终端弹窗 + macOS 通知 + Webhook

### Step 7: 集成 + 多市场支持 ✅
- 在 `main.rs` 中完整串联：数据采集 → 分析 → 提醒 → UI
- 实现 `futu/ocr.rs`：窗口截图 + Vision OCR 数据源（AX 辅助布局 + 防抖 + 截图哈希缓存）
- 美股盘前/盘后/夜盘时段检测与显示（`docs/US_MARKET_SESSIONS.md`）
- 新加坡、外汇市场支持
- 日K线缓存与增量拉取（`docs/DAILY_KLINE_CACHE.md`）
- 预留 `trading/` trait 定义

## 验证方式

1. **自选股读取**：`cargo run -- watchlist` → 打印从 plist 读取的自选股
2. **AX 数据获取**：打开富途牛牛 App → `cargo run -- start` → 终端显示实时价格
3. **OpenAPI 数据源**：启动 FutuOpenD → 配置 `source = "openapi"` → 确认实时推送行情
4. **OCR 数据源**：`cargo run -- test-ocr` → 显示 AX GridFrame 检测结果 + OCR 识别行情；AX 成功时 Pass 1 被跳过
5. **指标计算**：`cargo test` → 验证 MA/MACD/RSI 对已知数据的计算结果
6. **提醒触发**：设置涨跌幅阈值为 0.01% → 确认提醒触发和冷却机制生效
7. **端到端**：App 运行中 → qtrade 持续监控 → 行情变化时终端实时更新 + 提醒

## 未来扩展方向

- 纸上交易（paper trading）模拟
- 更多技术指标（布林带、KDJ 等）
- 多窗口/多账号支持
- Web 界面（axum + htmx）
- 策略回测引擎
