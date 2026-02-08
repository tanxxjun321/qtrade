# FutuOpenD 使用指南

## 什么是 FutuOpenD

FutuOpenD 是富途官方提供的 API 网关程序，运行在本地或服务器上，通过 TCP 协议（默认 `localhost:11111`）向第三方程序提供结构化行情数据。

qtrade 在以下场景依赖 FutuOpenD：

| 场景 | 是否需要 OpenD |
|------|---------------|
| `source = "ocr"` + 日 K 线分析开启 | **需要** — OCR 负责实时行情，日 K 线通过 OpenD 拉取 |
| `source = "openapi"` | **需要** — 实时行情和日 K 线均通过 OpenD |
| `source = "accessibility"` + 日 K 线分析开启 | **需要** — AX API 负责实时行情，日 K 线通过 OpenD |
| 任意数据源 + `daily_kline_enabled = false` | **不需要** |

> 简言之：只要开启日 K 线分析，就必须运行 OpenD。

## 下载与安装

**下载地址**：[https://www.futunn.com/download/OpenAPI](https://www.futunn.com/download/OpenAPI)

提供两种版本：
- **可视化版（推荐）** — 带 GUI，适合日常使用
- **命令行版** — 适合服务器 / 无头部署

**官方文档**：[https://openapi.futunn.com/futu-api-doc/](https://openapi.futunn.com/futu-api-doc/)

### macOS 安装注意

macOS 系统保护机制会给 OpenD.app 分配随机运行路径，导致找不到配置文件。解决方法二选一：

1. 运行安装包内的 `fixrun.sh` 脚本
2. 启动时指定配置文件路径：`./OpenD.app/Contents/MacOS/OpenD -cfg_file=./OpenD.xml`

## 配置 OpenD

### 可视化版

1. 启动 FutuOpenD.app
2. 使用富途账号登录（手机号 / 邮箱 / 牛牛号）
3. 确认监听地址和端口：
   - **IP**：`127.0.0.1`（仅本机访问）
   - **端口**：`11111`（默认）
4. 保持 OpenD 在后台运行

### 命令行版

编辑 `OpenD.xml`：

```xml
<login_account>你的账号</login_account>
<login_pwd>你的密码</login_pwd>
<ip>127.0.0.1</ip>
<api_port>11111</api_port>
<log_level>info</log_level>
```

启动：

```bash
./OpenD.app/Contents/MacOS/OpenD
```

## 配置 qtrade

确保 `config/config.toml` 中 OpenD 地址与实际一致：

```toml
[futu]
opend_host = "127.0.0.1"
opend_port = 11111

[analysis]
daily_kline_enabled = true      # 需要 OpenD
daily_kline_days = 120
daily_kline_refresh_minutes = 30
```

### 数据源选择

```toml
[data_source]
# 推荐：OCR 负责实时行情，OpenD 仅用于日 K 线
source = "ocr"

# 或：实时行情也走 OpenD（需要足够的订阅配额）
# source = "openapi"
```

**`source = "openapi"` 限制**：
- 实时行情需要消耗**订阅配额**，每只股票的每种订阅类型占 1 个配额
- 基础账户仅 100 个配额，自选股多时可能不够用
- 部分市场可能无行情权限

**`source = "ocr"` 推荐原因**：
- 实时行情通过截图获取，不消耗 OpenD 配额
- OpenD 仅在启动时拉取日 K 线（历史数据），配额压力小

## 验证连接

```bash
# 测试 OpenD 连接是否正常
cargo run -- test-api
```

成功时会输出连接信息和行情数据。失败时检查：

1. OpenD 是否在运行
2. 是否已登录
3. 端口是否匹配（默认 11111）

## 不使用 OpenD

如果不想安装 OpenD，关闭日 K 线分析即可：

```toml
[analysis]
daily_kline_enabled = false
```

此时 qtrade 仅使用 OCR 或 AX API 获取实时行情，不会连接 OpenD。日线技术信号（MA 金叉死叉、MACD、RSI 等）将不可用。

## 参考链接

- [OpenD 下载](https://www.futunn.com/download/OpenAPI)
- [官方 API 文档](https://openapi.futunn.com/futu-api-doc/)
- [可视化 OpenD 指南](https://openapi.futunn.com/futu-api-doc/en/quick/opend-base.html)
- [命令行 OpenD 指南](https://openapi.futunn.com/futu-api-doc/en/opend/opend-cmd.html)
- [订阅配额与权限](https://openapi.futunn.com/futu-api-doc/en/intro/authority.html)
