# 交易执行器（财富通V5.0）

## 概述

`trading/executor.rs` 实现通过 macOS Accessibility API 自动化操作财富通V5.0 客户端（cft5）进行股票交易。

## 支持市场

| 市场 | 代码格式 | 识别规则 | 交易路径 |
|------|----------|----------|----------|
| 港股 (HK) | 5 位数字 | `\d{5}` | 港股通 → 港股买入/卖出 |
| 沪市 A股 (SH) | 6 位 6xx | `6\d{5}` | 股票 → 证券买入/卖出 |
| 深市 A股 (SZ) | 6 位 0xx/3xx | `0\d{5}\|3\d{5}` | 股票 → 证券买入/卖出 |

## 核心组件

### OrderRequest

```rust
struct OrderRequest {
    stock_code: String,   // 如 "00700"
    price: f64,           // 委托价格
    quantity: u32,        // 委托数量（股）
    side: OrderSide,      // Buy / Sell
    market: TradingMarket, // HK / CN
}
```

### OrderSide

```rust
enum OrderSide {
    Buy,   // 买入
    Sell,  // 卖出
}
```

### TradingMarket

```rust
enum TradingMarket {
    HK,  // 港股
    CN,  // A股
}
```

## 执行流程

```
execute_order(req)
    │
    ├─→ prepare_trading_window()      # 窗口恢复
    │      ├─ 隐藏 → 激活
    │      ├─ 最小化 → 恢复
    │      ├─ 其他桌面 → SetFrontProcess
    │      └─ 托盘图标 → 双击恢复
    │
    ├─→ navigate_to_trading_panel()   # 导航到交易面板
    │      ├─ 港股: "港股通" tab → "港股买入/卖出"
    │      └─ A股: "股票" tab → "证券买入/卖出"
    │      (使用 raise_window + CGEventPost HID 点击)
    │
    ├─→ fill_form()                   # 填写表单
    │      ├─ 代码: AXTextField → SetAttributeValue
    │      ├─ 价格: AXIncrementor → pbcopy + Cmd+V
    │      └─ 数量: AXIncrementor → pbcopy + Cmd+V
    │
    ├─→ submit_order()                # 提交订单
    │      └─ AXButton "提交" → PerformAction
    │
    ├─→ wait_for_confirmation()       # 等待确认弹窗
    │      └─ 轮询查找 AXDialog
    │
    ├─→ verify_dialog_content()       # 验价
    │      └─ 核对股票代码 + 价格
    │
    ├─→ confirm_order()               # 确认
    │      └─ AXButton "委托" → PerformAction
    │
    └─→ check_error_dialog()          # 检查错误
           └─ 如有错误，关闭弹窗并返回错误信息
```

## 表单字段操作

### 代码输入 (AXTextField)

直接使用 `AXUIElementSetAttributeValue`：

```rust
set_attribute_value(&field, "AXValue", &code.into())?;
```

### 价格/数量输入 (AXIncrementor)

Qt 的 `AXIncrementor` SetAttributeValue 不更新内部模型，改用剪贴板粘贴：

```rust
// 1. 写入剪贴板
std::process::Command::new("pbcopy")
    .stdin(std::process::Stdio::piped())
    .spawn()?;

// 2. 聚焦字段
focus_element(&field)?;

// 3. Cmd+A 全选
post_key_event(0x00, true, true);  // kVK_ANSI_A + Cmd down
post_key_event(0x00, false, true);

// 4. Cmd+V 粘贴
post_key_event(0x09, true, true);  // kVK_ANSI_V + Cmd down
post_key_event(0x09, false, true);
```

## 导航点击策略

| 元素类型 | 操作方式 | 说明 |
|----------|----------|------|
| AXRadioButton (tab) | `AXPress` | 正常可用 |
| AXButton | `AXPress` | 正常可用 |
| AXStaticText (菜单项) | `CGEventPost(HID)` | AXPress 无效，需前台鼠标点击 |

## 确认弹窗处理

- 弹窗是独立的 `AXWindow`，`subrole = "AXDialog"`
- 标题为空，需通过子元素识别
- 确认按钮文字是 "委托"（不是 "确认"）
- 验价：收集所有 `AXStaticText`，检查包含股票代码和价格

## 错误处理

| 场景 | 处理 |
|------|------|
| 窗口未找到 | 返回错误，建议检查财富通是否运行 |
| 导航失败 | 返回错误，带路径信息 |
| 表单填写失败 | 返回错误 |
| 验价失败 | 取消订单，返回错误 |
| 交易错误弹窗 | 捕获文本，关闭弹窗，返回错误 |

## 并发控制

```rust
pub struct TradingExecutor {
    app: AXUIElement,
    window_mutex: tokio::sync::Mutex<()>,  // 确保串行
}
```

所有 UI 操作必须通过 `window_mutex.lock().await` 获取锁。

## 调试

```bash
# 检查财富通进程
pgrep -f cft5

# 检查窗口列表
cargo run -- debug

# 测试交易（需要确认）
cargo run -- test-trade
```

## 依赖

- `futu/ax/` - 安全 AX API 封装
- `core-foundation` - macOS 底层 API
- `tokio::sync::Mutex` - 异步锁
