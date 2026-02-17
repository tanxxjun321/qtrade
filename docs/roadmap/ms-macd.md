# MS-MACD 动能拐点买卖信号

## 概述

集成 MS-MACD 自定义指标，在日K线分析中检测 DIFF/DEA 极端区域的动能拐点，生成买入/卖出信号。

## 指标原理

MS-MACD 是标准 MACD (12,26,9) 的变体，将 DIFF 和 DEA 按正负分区显示：

| 区域 | 柱子 | 含义 |
|------|------|------|
| 零轴上方 | 红柱 (UPDIFF) | DIFF 正值部分 |
| 零轴上方 | 蓝柱 (UPDEA) | DEA 正值部分 |
| 零轴下方 | 绿柱 (LOWDIFF) | DIFF 负值部分 |
| 零轴下方 | 灰柱 (LOWDEA) | DEA 负值部分 |

原始公式（通达信语法）：
```
DIFF := (EMA(CLOSE,12) - EMA(CLOSE,26)) * 100;
DEA  := EMA(DIFF, 9);
MACD := 2 * (DIFF - DEA);
UPDIFF := MAX(DIFF, 0);   LOWDIFF := MIN(DIFF, 0);
UPDEA  := MAX(DEA, 0);    LOWDEA  := MIN(DEA, 0);
```

×100 缩放不影响零轴穿越和正负判断，实现时复用现有 MACD (12,26,9) 计算值。

## 买入/卖出条件

### 买入信号（空头区域动能衰减）

同时满足：
1. **DIFF < DEA < 0** — 绿柱比灰柱长（|DIFF| > |DEA|，处于深空头区域）
2. **今日 |DIFF| < 昨日 |DIFF|** — 绿柱缩短（DIFF 负值减小，空头动能开始衰减）

```
curr_dif < 0 && curr_dea < 0
  && curr_dif < curr_dea          // |DIFF| > |DEA|
  && prev_dif < 0
  && curr_dif.abs() < prev_dif.abs()  // 绿柱缩短
```

### 卖出信号（多头区域动能衰减）

同时满足：
1. **DIFF > DEA > 0** — 红柱比蓝柱长（处于深多头区域）
2. **今日 DIFF < 昨日 DIFF** — 红柱缩短（多头动能开始衰减）

```
curr_dif > 0 && curr_dea > 0
  && curr_dif > curr_dea          // DIFF > DEA
  && prev_dif > 0
  && curr_dif < prev_dif          // 红柱缩短
```

## 与现有 MACD 信号的区别

| 信号 | 触发条件 | 特点 |
|------|---------|------|
| MACD 金叉/死叉（现有） | DIFF 穿越 DEA | 灵敏，信号频繁 |
| MS-MACD 买入/卖出（新增） | 极端区域 + 动能拐点 | 滞后但可靠，趋势确认更强 |

两组信号互补，不冲突。

## 实现方案

### 涉及文件

| 文件 | 修改内容 |
|------|---------|
| `src/models.rs` | 添加 `MsMacdBuy` / `MsMacdSell` 信号变体 + sentiment + Display |
| `src/analysis/signals.rs` | 新增 `detect_ms_macd_signals()` 检测函数 + 2 个单元测试 |

### 关键点

- 复用现有 `TechnicalIndicators` 中的 `macd_dif` / `macd_dea`，无需修改指标计算层
- 在 `detect_signals()` 的 `if let Some(prev)` 块中调用新检测函数
- 新信号自动通过现有管线显示为 `[日]MS-MACD 买入` / `[日]MS-MACD 卖出`
- 无需修改 dashboard、config、daily engine 或 indicators

## 验证方式

1. `cargo build` — 编译通过
2. `cargo test` — 92 个测试全部通过（含 MS-MACD 新增测试）
3. `cargo run -- start` → 按 `d` 显示日线信号，确认 MS-MACD 信号正常显示
