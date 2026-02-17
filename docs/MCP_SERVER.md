# MCP 交易服务器

## 概述

MCP (Model Context Protocol) 服务器通过 Streamable HTTP 暴露交易工具，供大模型（如 Claude）调用，实现语音/对话式交易。

## 架构

```
┌─────────────┐     HTTP      ┌─────────────┐     AX API      ┌─────────────┐
│   Claude    │ ←──────────→ │  MCP Server │ ←────────────→ │   财富通    │
│   (MCP)     │  /mcp (8900)  │  (qtrade)   │   (cft5)      │   V5.0      │
└─────────────┘               └─────────────┘               └─────────────┘
```

## 配置

在 `config/config.toml` 中添加：

```toml
[mcp]
host = "127.0.0.1"     # MCP 服务器绑定地址
port = 8900            # MCP 服务器端口
```

## 启动

```bash
# 方式1：作为独立命令启动
cargo run -- mcp-server

# 方式2：Dashboard 运行时自动启动（如果配置了 [mcp] 段）
cargo run -- start
```

服务地址：`http://127.0.0.1:8900/mcp`

## 可用工具

### buy - 买入委托

```json
{
  "stock_code": "00700",
  "price": 385.50,
  "quantity": 100
}
```

- 股票代码：港股 5 位（如 00700），A股 6 位（如 600519）
- 价格：港股单位 HKD，A股单位 CNY
- 数量：股数（港股一手 = 100 股，A股一手 = 100 股）
- 自动识别市场，提交限价买入订单

### sell - 卖出委托

```json
{
  "stock_code": "00700",
  "price": 390.00,
  "quantity": 100
}
```

- 参数与 buy 相同
- 提交限价卖出订单

### get_quote - 获取行情

```json
{
  "stock_code": "00700"
}
```

- 只读操作，返回当前行情快照
- 包含：最新价、涨跌幅、成交量等

## 工作流程

1. **市场识别**：`TradingMarket::infer()` 根据代码格式自动推断
   - 5 位数字 → 港股 (HK)
   - 6 位数字 (6xx/0xx/3xx) → A股 (CN)

2. **订单构建**：构造 `OrderRequest` { code, price, quantity, side, market }

3. **执行订单**：`TradingExecutor::execute_order()`
   - 准备交易窗口（恢复隐藏/最小化）
   - AX 树导航到对应市场面板
   - 填写表单（代码/价格/数量）
   - 点击提交，等待确认弹窗
   - 验价（核对代码和价格）
   - 点击确认（"委托"按钮）
   - 检测错误弹窗

4. **返回结果**：成功/失败信息

## 安全机制

| 层级 | 机制 | 说明 |
|------|------|------|
| 验价 | 确认弹窗文本核对 | 检查股票代码和价格匹配 |
| 防重 | `tokio::sync::Mutex` | UI 操作严格串行，防止并发冲突 |
| 错误处理 | 自动捕获错误弹窗 | 任何步骤失败自动清理弹窗 |
| 窗口恢复 | `prepare_trading_window()` | 自动处理隐藏/最小化/跨桌面 |

## 依赖

- **rmcp 0.15**：MCP 协议实现
- **axum 0.8**：HTTP 服务
- **schemars 1.0**：JSON Schema 生成

## 调试

```bash
# 启用 MCP 调试日志
RUST_LOG=info cargo run -- mcp-server

# 会看到请求处理日志：
# [INFO] MCP request: tools/call buy {"stock_code": "00700", ...}
```
