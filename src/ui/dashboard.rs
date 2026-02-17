//! ratatui 终端仪表盘

use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::{Duration, Instant};

/// 最大最近提醒数量
pub const MAX_RECENT_ALERTS: usize = 1000;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::*;

use chrono::{DateTime, Local};

use crate::models::{
    AlertEvent, Market, QuoteSnapshot, Sentiment, Signal, StockCode, TechnicalIndicators, TimedSignal,
};

/// 仪表盘状态
pub struct DashboardState {
    /// 当前行情数据
    pub quotes: Vec<QuoteSnapshot>,
    /// 技术指标
    pub indicators: HashMap<StockCode, TechnicalIndicators>,
    /// 最近提醒（循环缓冲区，最多保留 MAX_RECENT_ALERTS 条）
    pub recent_alerts: VecDeque<AlertEvent>,
    /// 数据源状态
    pub source_name: String,
    /// 数据源是否连接
    pub source_connected: bool,
    /// 上次更新时间
    pub last_update: Option<Instant>,
    /// 选中行
    pub selected_row: usize,
    /// 滚动偏移
    pub scroll_offset: usize,
    /// 最近一次数据获取错误
    pub last_error: Option<String>,
    /// 是否显示技术指标
    pub show_indicators: bool,
    /// 排序列
    pub sort_column: SortColumn,
    /// 排序方向
    pub sort_ascending: bool,
    /// 日线技术指标
    pub daily_indicators: HashMap<StockCode, TechnicalIndicators>,
    /// 日线信号
    pub daily_signals: HashMap<StockCode, Vec<TimedSignal>>,
    /// 是否显示日线信号
    pub show_daily_signals: bool,
    /// 日K线获取状态（显示在状态栏）
    pub daily_kline_status: String,
    /// Tick 信号（事件型，带触发时间）
    pub tick_signals: HashMap<StockCode, Vec<(Signal, DateTime<Local>)>>,
    /// 每只股票最大日线信号数量（通常与 daily_kline_days 一致）
    pub max_daily_signals_per_stock: usize,
}

/// 排序列
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortColumn {
    Code,
    Name,
    Price,
    ChangePct,
    Volume,
}

impl DashboardState {
    /// 创建新的仪表盘状态
    ///
    /// # Arguments
    /// * `max_daily_signals` - 每只股票最大日线信号数量，建议与 config.analysis.daily_kline_days 一致
    pub fn new(max_daily_signals: usize) -> Self {
        Self {
            quotes: Vec::new(),
            indicators: HashMap::new(),
            recent_alerts: VecDeque::with_capacity(MAX_RECENT_ALERTS),
            source_name: String::new(),
            source_connected: false,
            last_update: None,
            selected_row: 0,
            scroll_offset: 0,
            last_error: None,
            show_indicators: true,
            sort_column: SortColumn::ChangePct,
            sort_ascending: false,
            daily_indicators: HashMap::new(),
            daily_signals: HashMap::new(),
            show_daily_signals: true,
            daily_kline_status: String::new(),
            tick_signals: HashMap::new(),
            max_daily_signals_per_stock: max_daily_signals,
        }
    }

    /// 更新行情数据（按股票代码合并，不丢失未更新的股票）
    ///
    /// 匹配规则：
    /// 1. market + code 完全匹配 → 直接合并
    /// 2. code 字符串相同，一方 market 为 Unknown → 视为同一只股票，采用非 Unknown 的 market
    pub fn update_quotes(&mut self, new_quotes: Vec<QuoteSnapshot>) {
        if self.quotes.is_empty() {
            // 首次初始化，直接赋值
            self.quotes = new_quotes;
        } else {
            // 合并：更新已有的，添加新的
            for mut new_q in new_quotes {
                // 查找匹配：优先精确匹配，其次 code 相同 + 一方 Unknown
                let found = self.quotes.iter_mut().find(|q| {
                    if q.code == new_q.code {
                        return true;
                    }
                    // code 字符串相同但 market 不同：Unknown 视为通配
                    q.code.code == new_q.code.code
                        && (q.code.market == Market::Unknown || new_q.code.market == Market::Unknown)
                });
                if let Some(existing) = found {
                    // 保留已有的中文名（API 返回的是英文名）
                    if !existing.name.is_empty() && new_q.name.is_empty() {
                        new_q.name = existing.name.clone();
                    }
                    // 采用非 Unknown 的市场（OCR 回写修正）
                    if new_q.code.market == Market::Unknown && existing.code.market != Market::Unknown {
                        new_q.code.market = existing.code.market;
                    }
                    *existing = new_q;
                } else {
                    self.quotes.push(new_q);
                }
            }
        }
        self.last_update = Some(Instant::now());
        self.sort_quotes();
    }

    /// 同步 watchlist 变更：移除已删股票、添加新增股票
    pub fn sync_watchlist(&mut self, new_codes: &[StockCode], new_entries: &[crate::models::WatchlistEntry]) {
        use std::collections::HashSet;

        let new_set: HashSet<&StockCode> = new_codes.iter().collect();

        // 移除不在新列表中的股票
        self.quotes.retain(|q| new_set.contains(&q.code));
        self.indicators.retain(|k, _| new_set.contains(k));
        self.daily_indicators.retain(|k, _| new_set.contains(k));
        self.daily_signals.retain(|k, _| new_set.contains(k));
        self.tick_signals.retain(|k, _| new_set.contains(k));

        // 新增的股票追加空 QuoteSnapshot
        let existing: HashSet<StockCode> = self.quotes.iter().map(|q| q.code.clone()).collect();
        for entry in new_entries {
            if !existing.contains(&entry.code) && new_set.contains(&entry.code) {
                let mut q = QuoteSnapshot::empty(entry.code.clone(), entry.name.clone());
                if let Some(price) = entry.cached_price {
                    q.last_price = price;
                }
                self.quotes.push(q);
            }
        }

        // 清理每只股票过多的日线信号
        for signals in self.daily_signals.values_mut() {
            if signals.len() > self.max_daily_signals_per_stock {
                // 保留最新的信号
                let start = signals.len() - self.max_daily_signals_per_stock;
                *signals = signals.split_off(start);
            }
        }

        // 防越界
        if !self.quotes.is_empty() {
            if self.selected_row >= self.quotes.len() {
                self.selected_row = self.quotes.len() - 1;
            }
        } else {
            self.selected_row = 0;
        }

        self.sort_quotes();
    }

    /// 排序
    fn sort_quotes(&mut self) {
        let asc = self.sort_ascending;
        match self.sort_column {
            SortColumn::Code => {
                self.quotes.sort_by(|a, b| {
                    let cmp = a.code.display_code().cmp(&b.code.display_code());
                    if asc {
                        cmp
                    } else {
                        cmp.reverse()
                    }
                });
            }
            SortColumn::Name => {
                self.quotes.sort_by(|a, b| {
                    let cmp = a.name.cmp(&b.name);
                    if asc {
                        cmp
                    } else {
                        cmp.reverse()
                    }
                });
            }
            SortColumn::Price => {
                self.quotes.sort_by(|a, b| {
                    let cmp = a
                        .last_price
                        .partial_cmp(&b.last_price)
                        .unwrap_or(std::cmp::Ordering::Equal);
                    if asc {
                        cmp
                    } else {
                        cmp.reverse()
                    }
                });
            }
            SortColumn::ChangePct => {
                self.quotes.sort_by(|a, b| {
                    let cmp = a
                        .change_pct
                        .partial_cmp(&b.change_pct)
                        .unwrap_or(std::cmp::Ordering::Equal);
                    if asc {
                        cmp
                    } else {
                        cmp.reverse()
                    }
                });
            }
            SortColumn::Volume => {
                self.quotes.sort_by(|a, b| {
                    let cmp = a.volume.cmp(&b.volume);
                    if asc {
                        cmp
                    } else {
                        cmp.reverse()
                    }
                });
            }
        }
    }
}

/// 初始化终端
pub fn init_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    Terminal::new(backend)
}

/// 恢复终端
pub fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

/// 渲染仪表盘
pub fn render(frame: &mut Frame, state: &DashboardState) {
    let area = frame.area();

    // 布局：标题栏 + 主表格 + 提醒栏 + 状态栏
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // 标题
            Constraint::Min(10),    // 主表格
            Constraint::Length(10), // 提醒（8 条 + 2 行边框）
            Constraint::Length(1),  // 状态栏
        ])
        .split(area);

    // 标题栏
    render_title(frame, chunks[0]);

    // 主行情表格
    render_quote_table(frame, chunks[1], state);

    // 提醒栏
    render_alerts(frame, chunks[2], state);

    // 状态栏
    render_status_bar(frame, chunks[3], state);
}

/// 渲染标题
fn render_title(frame: &mut Frame, area: Rect) {
    let title = Block::default()
        .title(" qtrade 量化盯盘系统 ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(title, area);
}

/// 渲染行情表格
fn render_quote_table(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let header_cells = [
        "代码",
        "名称",
        "现价",
        "涨跌%",
        "涨跌额",
        "成交量",
        "换手率%",
        "振幅%",
        "信号",
    ]
    .iter()
    .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));

    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = state
        .quotes
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let selected = i == state.selected_row;

            // 美股非盘中时段：extended_price 才是实价，涨跌相对收盘价重算
            // 但盘前/盘后价格与收盘价相同（无盘前变动）时回退到显示收盘涨跌
            let use_extended = q.code.market == Market::US
                && crate::models::us_market_session() != crate::models::UsMarketSession::Regular
                && q.extended_price.is_some()
                && (q.extended_price.unwrap() - q.last_price).abs() > 0.001;

            let (display_price, display_change, display_change_pct) = if use_extended {
                let ext = q.extended_price.unwrap();
                let chg = ext - q.last_price;
                let pct = if q.last_price > 0.0 {
                    chg / q.last_price * 100.0
                } else {
                    0.0
                };
                (ext, chg, pct)
            } else {
                (q.last_price, q.change, q.change_pct)
            };

            let change_color = match (display_change_pct > 0.0, display_change_pct < 0.0, selected) {
                (true, _, true) => Color::LightRed,
                (true, _, false) => Color::Red,
                (_, true, true) => Color::LightGreen,
                (_, true, false) => Color::Green,
                (_, _, true) => Color::White,
                _ => Color::Reset,
            };

            let mut signal_spans: Vec<Span> = Vec::new();

            // Tick 信号（事件型，按情绪着色）
            if let Some(sigs) = state.tick_signals.get(&q.code) {
                for (sig, _at) in sigs.iter().rev() {
                    if !signal_spans.is_empty() {
                        signal_spans.push(Span::raw("  "));
                    }
                    let color = sentiment_color(sig.sentiment(), selected);
                    signal_spans.push(Span::styled(
                        format!("[{}]{}", sig.sentiment(), sig),
                        Style::new().fg(color),
                    ));
                }
            }

            // 日线信号（按情绪着色）
            if state.show_daily_signals {
                if let Some(sigs) = state.daily_signals.get(&q.code) {
                    for s in sigs {
                        if !signal_spans.is_empty() {
                            signal_spans.push(Span::raw("  "));
                        }
                        let color = sentiment_color(s.signal.sentiment(), selected);
                        signal_spans.push(Span::styled(s.to_string(), Style::new().fg(color)));
                    }
                }
            }

            // 仅有 plist 缓存数据（未被 OCR/API 更新过）→ 灰色 "-" 代替虚假的 0%
            let is_stale = q.source == crate::models::DataSource::Cache;
            let price_str = if is_stale && q.last_price > 0.0 {
                format!("{:.2}", q.last_price)
            } else if is_stale {
                "-".to_string()
            } else {
                format!("{:.2}", display_price)
            };
            let stale_color = Color::DarkGray;

            // Cell 只设 fg，不设 bg — bg 由 Row style 统一控制
            let signal_cell = Cell::from(Line::from(signal_spans));

            let cells = if is_stale {
                vec![
                    Cell::from(q.code.display_code()),
                    Cell::from(q.name.clone()),
                    Cell::from(price_str).style(Style::new().fg(stale_color)),
                    Cell::from("-").style(Style::new().fg(stale_color)),
                    Cell::from("-").style(Style::new().fg(stale_color)),
                    Cell::from("-").style(Style::new().fg(stale_color)),
                    Cell::from("-").style(Style::new().fg(stale_color)),
                    Cell::from("-").style(Style::new().fg(stale_color)),
                    signal_cell,
                ]
            } else {
                vec![
                    Cell::from(q.code.display_code()),
                    Cell::from(q.name.clone()),
                    Cell::from(price_str).style(Style::new().fg(change_color)),
                    Cell::from(format!("{:+.2}%", display_change_pct)).style(Style::new().fg(change_color)),
                    Cell::from(format!("{:+.2}", display_change)).style(Style::new().fg(change_color)),
                    Cell::from(format_volume(q.volume)),
                    Cell::from(format!("{:.2}", q.turnover_rate)),
                    Cell::from(format!("{:.2}", q.amplitude)),
                    signal_cell,
                ]
            };

            let row_style = if selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };

            Row::new(cells).style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Fill(1),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(format!(" 自选股行情 ({}) ", state.quotes.len()))
                .borders(Borders::ALL),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

    frame.render_widget(table, area);
}

/// 渲染提醒栏
fn render_alerts(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let alerts: Vec<ListItem> = state
        .recent_alerts
        .iter()
        .rev()
        .take(8)
        .map(|a| {
            let color = match a.sentiment {
                Some(Sentiment::Bullish) => Color::Red,
                Some(Sentiment::Bearish) => Color::Green,
                _ => Color::DarkGray,
            };
            let style = Style::default().fg(color);
            ListItem::new(format!(
                "[{}] {} {}",
                a.triggered_at.format("%H:%M:%S"),
                a.code,
                a.message
            ))
            .style(style)
        })
        .collect();

    let alerts_widget = List::new(alerts).block(
        Block::default()
            .title(" 最近提醒 ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );

    frame.render_widget(alerts_widget, area);
}

/// 渲染状态栏
fn render_status_bar(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let update_info = match state.last_update {
        Some(t) => {
            let elapsed = t.elapsed().as_secs();
            if elapsed < 5 {
                "刚刚更新".to_string()
            } else {
                format!("{}秒前", elapsed)
            }
        }
        None => "未更新".to_string(),
    };

    let conn_status = if state.source_connected {
        "已连接"
    } else {
        "未连接"
    };

    let error_info = match &state.last_error {
        Some(e) => {
            format!(" | 错误: {}", if e.len() > 40 { &e[..40] } else { e })
        }
        None => String::new(),
    };

    let daily_info = if state.daily_kline_status.is_empty() {
        String::new()
    } else {
        format!(" | {}", state.daily_kline_status)
    };

    let status = format!(
        " 数据源: {} ({}) | 更新: {}{}{} | ↑↓选择 s排序 d日线 q退出 ",
        state.source_name, conn_status, update_info, error_info, daily_info
    );

    let bar = Paragraph::new(status).style(Style::default().bg(Color::DarkGray).fg(Color::White));

    frame.render_widget(bar, area);
}

/// 处理键盘输入，返回 true 表示退出
pub fn handle_input(state: &mut DashboardState) -> io::Result<bool> {
    if event::poll(Duration::from_millis(100))? {
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                return Ok(false);
            }

            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
                KeyCode::Up | KeyCode::Char('k') => {
                    if state.selected_row > 0 {
                        state.selected_row -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if state.selected_row + 1 < state.quotes.len() {
                        state.selected_row += 1;
                    }
                }
                KeyCode::Char('s') => {
                    // 切换排序列
                    state.sort_column = match state.sort_column {
                        SortColumn::Code => SortColumn::Name,
                        SortColumn::Name => SortColumn::Price,
                        SortColumn::Price => SortColumn::ChangePct,
                        SortColumn::ChangePct => SortColumn::Volume,
                        SortColumn::Volume => SortColumn::Code,
                    };
                    state.sort_quotes();
                }
                KeyCode::Char('S') => {
                    // 切换排序方向
                    state.sort_ascending = !state.sort_ascending;
                    state.sort_quotes();
                }
                KeyCode::Char('i') => {
                    state.show_indicators = !state.show_indicators;
                }
                KeyCode::Char('d') => {
                    state.show_daily_signals = !state.show_daily_signals;
                }
                _ => {}
            }
        }
    }
    Ok(false)
}

/// 根据情绪方向返回颜色
fn sentiment_color(sentiment: Sentiment, selected: bool) -> Color {
    match (sentiment, selected) {
        (Sentiment::Bullish, true) => Color::LightRed,
        (Sentiment::Bullish, false) => Color::Red,
        (Sentiment::Bearish, true) => Color::LightGreen,
        (Sentiment::Bearish, false) => Color::Green,
        (Sentiment::Neutral, true) => Color::Gray,
        (Sentiment::Neutral, false) => Color::DarkGray,
    }
}

/// 格式化成交量
fn format_volume(vol: u64) -> String {
    if vol >= 100_000_000 {
        format!("{:.1}亿", vol as f64 / 100_000_000.0)
    } else if vol >= 10_000 {
        format!("{:.1}万", vol as f64 / 10_000.0)
    } else {
        format!("{}", vol)
    }
}
