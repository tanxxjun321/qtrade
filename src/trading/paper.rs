//! 纸上交易（模拟交易）模块 - 预留

use crate::models::{QuoteSnapshot, StockCode};
use anyhow::Result;
use std::collections::HashMap;

/// 交易方向
#[derive(Debug, Clone, Copy)]
pub enum Side {
    Buy,
    Sell,
}

/// 模拟持仓
#[derive(Debug, Clone)]
pub struct Position {
    pub code: StockCode,
    pub quantity: u64,
    pub avg_cost: f64,
    pub market_value: f64,
    pub unrealized_pnl: f64,
}

/// 纸上交易引擎（预留接口）
pub struct PaperTradingEngine {
    /// 初始资金
    pub initial_capital: f64,
    /// 可用资金
    pub available_cash: f64,
    /// 持仓
    pub positions: HashMap<StockCode, Position>,
}

impl PaperTradingEngine {
    pub fn new(initial_capital: f64) -> Self {
        Self {
            initial_capital,
            available_cash: initial_capital,
            positions: HashMap::new(),
        }
    }

    /// 下单（预留）
    pub fn place_order(
        &mut self,
        _code: &StockCode,
        _side: Side,
        _quantity: u64,
        _price: f64,
    ) -> Result<()> {
        // TODO: 实现模拟交易逻辑
        anyhow::bail!("Paper trading not yet implemented")
    }

    /// 更新持仓市值
    pub fn update_market_value(&mut self, quotes: &[QuoteSnapshot]) {
        for quote in quotes {
            if let Some(pos) = self.positions.get_mut(&quote.code) {
                pos.market_value = pos.quantity as f64 * quote.last_price;
                pos.unrealized_pnl = pos.market_value - (pos.quantity as f64 * pos.avg_cost);
            }
        }
    }

    /// 总资产
    pub fn total_equity(&self) -> f64 {
        self.available_cash
            + self
                .positions
                .values()
                .map(|p| p.market_value)
                .sum::<f64>()
    }
}
