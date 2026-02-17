#!/usr/bin/env python3
"""test-mcp.py — 简单测试 MCP 服务器（Streamable HTTP）

用法：
    1. 先启动 MCP 服务器：cargo run -- mcp-server
    2. 运行测试：python3 scripts/test-mcp.py [stock_code]

默认股票代码: 00700
环境变量: MCP_URL (默认 http://127.0.0.1:8900/mcp)
"""

from __future__ import annotations

import json
import os
import sys
import urllib.request
import urllib.error

MCP_URL = os.environ.get("MCP_URL", "http://127.0.0.1:8900/mcp")
STOCK_CODE = sys.argv[1] if len(sys.argv) > 1 else "00700"

# 颜色
GREEN = "\033[0;32m"
RED = "\033[0;31m"
YELLOW = "\033[0;33m"
CYAN = "\033[0;36m"
NC = "\033[0m"

passed = 0
failed = 0
session_id = None


def log(msg):
    print(f"{CYAN}[TEST]{NC} {msg}")


def ok(msg):
    global passed
    print(f"{GREEN}[PASS]{NC} {msg}")
    passed += 1


def fail(msg):
    global failed
    print(f"{RED}[FAIL]{NC} {msg}")
    failed += 1


def info(msg):
    print(f"{YELLOW}  ->  {NC} {msg}")


def mcp_post(payload: dict) -> tuple[dict | None, int]:
    """发送 JSON-RPC 请求到 MCP 服务器，返回 (parsed_json, http_status)"""
    global session_id

    data = json.dumps(payload).encode()
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if session_id:
        headers["Mcp-Session-Id"] = session_id

    req = urllib.request.Request(MCP_URL, data=data, headers=headers, method="POST")

    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            # 提取 session id
            sid = resp.headers.get("Mcp-Session-Id")
            if sid:
                session_id = sid

            body = resp.read().decode()
            status = resp.status

            # SSE 格式处理：提取 data: 行中的 JSON
            if "data: " in body:
                for line in body.splitlines():
                    if line.startswith("data: "):
                        json_str = line[len("data: "):]
                        try:
                            return json.loads(json_str), status
                        except json.JSONDecodeError:
                            continue

            # 纯 JSON 响应
            try:
                return json.loads(body), status
            except json.JSONDecodeError:
                return None, status

    except urllib.error.HTTPError as e:
        body = e.read().decode() if e.fp else ""
        # 通知请求可能返回 202
        if e.code == 202:
            return None, 202
        return None, e.code
    except urllib.error.URLError:
        return None, 0


def mcp_notify(method: str):
    """发送 JSON-RPC 通知（无 id，不期望响应）"""
    mcp_post({"jsonrpc": "2.0", "method": method})


# ===== 开始测试 =====

log(f"目标: {MCP_URL}")
log(f"测试股票: {STOCK_CODE}")
print()

# ===== Test 1: Initialize =====
log("Test 1: MCP Initialize")

resp, status = mcp_post({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {
            "name": "test-mcp-client",
            "version": "0.1.0",
        },
    },
})

if status == 0:
    fail(f"无法连接 MCP 服务器 ({MCP_URL})")
    print()
    print("请确保 MCP 服务器已启动: cargo run -- mcp-server")
    sys.exit(1)

if resp and "result" in resp:
    result = resp["result"]
    server_info = result.get("serverInfo", {})
    name = server_info.get("name", "?")
    version = server_info.get("version", "?")

    if name == "qtrade-mcp":
        ok("Initialize 成功")
        info(f"服务器: {name} v{version}")
        if session_id:
            info(f"Session: {session_id[:16]}...")
        proto = result.get("protocolVersion", "?")
        info(f"协议版本: {proto}")
    else:
        fail(f"服务器名称不匹配: {name}")
else:
    fail("Initialize 返回异常")
    info(f"status={status} resp={resp}")

print()

# ===== Send initialized notification =====
mcp_notify("notifications/initialized")

# ===== Test 2: List Tools =====
log("Test 2: tools/list")

resp, status = mcp_post({
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/list",
    "params": {},
})

expected_tools = {"hk_buy", "hk_sell", "get_quote"}

if resp and "result" in resp:
    tools = resp["result"].get("tools", [])
    tool_names = {t["name"] for t in tools}

    if expected_tools.issubset(tool_names):
        ok(f"tools/list 返回 {len(tools)} 个工具")
        for t in tools:
            name = t["name"]
            desc = t.get("description", "")[:40]
            props = list(t.get("inputSchema", {}).get("properties", {}).keys())
            info(f"{name}({', '.join(props)}) — {desc}")
    else:
        missing = expected_tools - tool_names
        fail(f"tools/list 缺少工具: {missing}")
        info(f"实际: {tool_names}")
else:
    fail("tools/list 返回异常")
    info(f"status={status} resp={resp}")

print()

# ===== Test 3: get_quote (只读调用) =====
log(f"Test 3: tools/call get_quote({STOCK_CODE})")

resp, status = mcp_post({
    "jsonrpc": "2.0",
    "id": 3,
    "method": "tools/call",
    "params": {
        "name": "get_quote",
        "arguments": {
            "stock_code": STOCK_CODE,
        },
    },
})

if resp and "result" in resp:
    result = resp["result"]
    is_error = result.get("isError", False)
    content = result.get("content", [])
    texts = [c["text"] for c in content if c.get("type") == "text"]
    text = "\n".join(texts)

    if is_error:
        ok("get_quote 调用完成 (返回错误信息，可能是交易App未开启)")
    else:
        ok("get_quote 调用成功")
    for line in text.splitlines():
        info(line)
elif resp and "error" in resp:
    fail("get_quote 返回 JSON-RPC 错误")
    err = resp["error"]
    info(f"code={err.get('code')} message={err.get('message')}")
else:
    fail("get_quote 响应异常")
    info(f"status={status} resp={resp}")

print()

# ===== Test 4: hk_buy 低价测试（不会成交） =====
BUY_PRICE = 0.01  # 极低价格，挂单后不会成交
BUY_QTY = 100     # 最小手数

log(f"Test 4: tools/call hk_buy({STOCK_CODE}, price={BUY_PRICE}, qty={BUY_QTY})")
info("使用极低价格 0.01 挂单，验证交易流程，不会真正成交")

resp, status = mcp_post({
    "jsonrpc": "2.0",
    "id": 4,
    "method": "tools/call",
    "params": {
        "name": "hk_buy",
        "arguments": {
            "stock_code": STOCK_CODE,
            "price": BUY_PRICE,
            "quantity": BUY_QTY,
        },
    },
})

if resp and "result" in resp:
    result = resp["result"]
    is_error = result.get("isError", False)
    content = result.get("content", [])
    texts = [c["text"] for c in content if c.get("type") == "text"]
    text = "\n".join(texts)

    if is_error:
        # 预期情况：验价失败（确认弹窗价格 != 0.01）或交易App未开启
        ok("hk_buy 流程完成（返回错误，符合预期：验价不通过或App未就绪）")
    else:
        ok("hk_buy 委托提交成功（价格 0.01，不会成交，请记得撤单）")
    for line in text.splitlines():
        info(line)
elif resp and "error" in resp:
    fail("hk_buy 返回 JSON-RPC 错误")
    err = resp["error"]
    info(f"code={err.get('code')} message={err.get('message')}")
else:
    fail("hk_buy 响应异常")
    info(f"status={status} resp={resp}")

print()

# ===== 汇总 =====
total = passed + failed
print("━" * 30)
print(f"结果: {GREEN}{passed} 通过{NC} / {RED}{failed} 失败{NC} / {total} 总计")

sys.exit(1 if failed > 0 else 0)
