# Watchlist 动态同步

**状态：已实现**

## 概述

运行期间监测富途牛牛 plist 文件变化，通过 `tokio::sync::watch` 广播 watchlist 变更，各组件（行情拉取、日K线、Dashboard、分析引擎）自动响应增删股票。

## 实现细节

### 1. plist 文件监测（`src/main.rs` — monitor task）

- `detect_plist_path()` 获取 plist 完整路径
- 独立 tokio task，**3 秒轮询** plist mtime
- mtime 变化时重新 `load_watchlist()` → `filter_stock_codes()` → 计算 added/removed
- 删除流程：`engine.remove_stock()` → `daily_engine.remove_stocks()` → `dash.sync_watchlist()`
- 新增流程：`dash.sync_watchlist()` 添加占位 QuoteSnapshot（带 plist 缓存价格）
- `watch_tx.send(new_codes)` 广播给 fetch loop 和 daily kline loop

### 2. 行情拉取循环（`src/main.rs` — fetch loop）

- `tokio::select!` 同时监听 `interval.tick()` 和 `watch_rx.changed()`
- 收到 changed 时：
  - **removed** → `provider.unsubscribe()` 释放 OpenAPI 订阅 slot
  - **added** → `provider.subscribe()` 订阅新股行情
  - AX/OCR 模式下 subscribe/unsubscribe 为 no-op
- 下一次 tick 即用新列表拉取行情

### 3. 日K线拉取循环（`src/main.rs` — daily kline loop）

- `tokio::select!` 同时监听定时器和 `watch_rx.changed()`
- 收到 changed 时：仅对 **added** 股票调用 `run_daily_kline_cycle()`
- 定时刷新时：使用最新完整列表

### 4. Dashboard 同步（`src/ui/dashboard.rs`）

`sync_watchlist(new_codes, new_entries)` 方法：
- `quotes.retain()` 移除不在新列表中的股票
- `indicators` / `signals` / `daily_indicators` / `daily_signals` 全部 retain
- 新增股票追加空 `QuoteSnapshot`（用 WatchlistEntry 的 name + cached_price 初始化）
- `selected_row` 防越界
- 调用 `sort_quotes()` 重新排序

### 5. 分析引擎清理

**Tick 级（`src/analysis/engine.rs`）**：
- `remove_stock(code)` — 清理 `windows` 和 `prev_indicators`

**日线级（`src/analysis/daily.rs`）**：
- `remove_stocks(codes)` — 从 5 个 HashMap（`klines`、`last_fetched`、`indicators`、`prev_indicators`、`signals`）中移除，并调用 `save_cache()` 持久化

### 6. OpenAPI 退订

**`src/futu/openapi.rs`**：
- `unsubscribe(stocks, sub_types)` — 按市场分组，调用 `unsubscribe_batch()`
- `unsubscribe_batch()` — 发送 QotSub 请求，`isSubOrUnSub: false`、`isRegOrUnRegPush: false`

**`src/data/provider.rs`**：
- `DataProviderKind::unsubscribe(codes)` — 枚举分发，OpenAPI 调用 `client.unsubscribe(codes, &[1])`，AX/OCR 为 no-op

### 7. 辅助函数

**`src/futu/watchlist.rs`**：
- `detect_plist_path(data_path, user_id)` — 复用 `detect_futu_data_path()` + `find_user_dir()`，返回 plist 完整路径

**`src/main.rs`**：
- `filter_stock_codes(watchlist)` — 过滤 800xxx 和 Unknown 市场，提取为公共函数避免重复

## 涉及文件

| 文件 | 改动 |
|------|------|
| `src/futu/watchlist.rs` | 新增 `detect_plist_path()` |
| `src/analysis/engine.rs` | 新增 `remove_stock()` |
| `src/analysis/daily.rs` | 新增 `remove_stocks()` |
| `src/ui/dashboard.rs` | 新增 `sync_watchlist()` |
| `src/futu/openapi.rs` | 新增 `unsubscribe()` + `unsubscribe_batch()` |
| `src/data/provider.rs` | 新增 `unsubscribe()` 枚举分发 |
| `src/main.rs` | watch channel + monitor task + fetch/daily loop 适配 + `filter_stock_codes()` |

## 验证方式

1. `cargo build` — 编译通过
2. `cargo test` — 34 个测试全部通过
3. 手动测试：`cargo run -- start`，在富途 App 中增删自选股，观察 TUI 自动同步
4. 日志关键词：`Plist mtime changed`、`Added stock`、`Removed stock`、`Fetch loop updated`
5. 删除股票后检查 `~/.config/qtrade/kline_cache.json` 中该股票已清除
