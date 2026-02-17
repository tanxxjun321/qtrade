#!/bin/bash
# 检查财富通进程和 AX 状态

echo "=== 财富通进程检查 ==="
pgrep -f cft5 | while read pid; do
    echo "PID: $pid"
    ps -p $pid -o comm=,pid=,state=
done

echo ""
echo "=== GUI 进程检查 ==="
ps aux | grep -i cft5 | grep -v grep | head -5

echo ""
echo "=== 辅助功能权限检查 ==="
sqlite3 ~/Library/Application\ Support/com.apple.TCC/TCC.db \
    "SELECT service, client, allowed FROM access WHERE service = 'kTCCServiceAccessibility' AND client LIKE '%qtrade%';" 2>/dev/null || echo "无法读取 TCC 数据库（需要完整磁盘访问权限）"

echo ""
echo "=== 测试 AX 连接 ==="
# 运行 debug 命令检查
cargo run -- debug 2>&1 | head -30
