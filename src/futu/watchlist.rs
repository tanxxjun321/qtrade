use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::models::{Market, StockCode, WatchlistEntry};

/// 富途牛牛 App 本地数据基础路径
const FUTU_BASE_PATH: &str =
    "Library/Containers/cn.futu.Niuniu/Data/Library/Application Support";

/// 自选股文件名
const WATCHLIST_FILENAME: &str = "watchstockContainer.dat";

/// 股票名称数据库路径（相对于 FUTU_BASE_PATH）
const STOCK_DB_PATH: &str = "StockDB/appdatav82.db";

/// 价格精度除数（plist 中的高精度整数 → 实际价格）
/// 从 key 名 `KFLStockKeyClosePriceOneYear10x9` 中的 "10x9" 可知精度为 10^9
const PRICE_DIVISOR: f64 = 1_000_000_000.0; // 10^9

/// 自动检测富途数据路径
pub fn detect_futu_data_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    let base = PathBuf::from(&home).join(FUTU_BASE_PATH);

    if !base.exists() {
        anyhow::bail!(
            "富途牛牛数据目录不存在: {}\n请确认已安装富途牛牛 App",
            base.display()
        );
    }

    Ok(base)
}

/// 查找包含自选股数据的用户目录
/// 策略：扫描所有数字命名的子目录，选择 watchlist 文件修改时间最新的
pub fn find_user_dir(base_path: &Path, user_id: Option<&str>) -> Result<PathBuf> {
    // 如果指定了 user_id，直接使用
    if let Some(uid) = user_id {
        let dir = base_path.join(uid);
        let watchlist_file = dir.join(WATCHLIST_FILENAME);
        if watchlist_file.exists() {
            info!("Using specified user directory: {}", uid);
            return Ok(dir);
        }
        anyhow::bail!(
            "指定的用户目录 {} 中未找到 {}",
            uid,
            WATCHLIST_FILENAME
        );
    }

    // 自动扫描：找所有数字命名的目录
    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    let entries = std::fs::read_dir(base_path)
        .with_context(|| format!("Failed to read directory: {}", base_path.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        // 检查目录名是否是数字
        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        if !dir_name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let watchlist_file = path.join(WATCHLIST_FILENAME);
        if watchlist_file.exists() {
            let modified = watchlist_file
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            debug!(
                "Found watchlist in user dir: {} (modified: {:?})",
                dir_name, modified
            );
            candidates.push((path, modified));
        }
    }

    if candidates.is_empty() {
        anyhow::bail!(
            "未找到包含 {} 的用户目录\n基础路径: {}",
            WATCHLIST_FILENAME,
            base_path.display()
        );
    }

    // 选择修改时间最新的
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    let chosen = &candidates[0].0;
    info!(
        "Auto-selected user directory: {} (most recently modified)",
        chosen.file_name().unwrap_or_default().to_string_lossy()
    );

    Ok(chosen.clone())
}

/// 从 plist 文件读取自选股列表
pub fn read_watchlist(plist_path: &Path) -> Result<Vec<WatchlistEntry>> {
    info!("Reading watchlist from: {}", plist_path.display());

    let content = std::fs::read(plist_path)
        .with_context(|| format!("Failed to read plist file: {}", plist_path.display()))?;

    let value: plist::Value = plist::from_bytes(&content)
        .with_context(|| "Failed to parse plist data")?;

    parse_watchlist_plist(&value)
}

/// 轻量读取：只返回 plist 路径和股票代码集合（不读 StockDB），供白名单过滤用
pub fn load_watchlist_codes() -> Result<(PathBuf, Vec<StockCode>)> {
    let base_path = detect_futu_data_path()?;
    let user_dir = find_user_dir(&base_path, None)?;
    let plist_path = user_dir.join(WATCHLIST_FILENAME);
    let entries = read_watchlist(&plist_path)?;
    let codes = entries.into_iter().map(|e| e.code).collect();
    Ok((plist_path, codes))
}

/// 一站式读取：自动检测路径 + 读取自选股 + 填充名称
pub fn load_watchlist(
    data_path: Option<&str>,
    user_id: Option<&str>,
) -> Result<Vec<WatchlistEntry>> {
    let base_path = match data_path {
        Some(p) => PathBuf::from(p),
        None => detect_futu_data_path()?,
    };

    let user_dir = find_user_dir(&base_path, user_id)?;
    let plist_path = user_dir.join(WATCHLIST_FILENAME);

    let mut entries = read_watchlist(&plist_path)?;

    // 从 StockDB 填充股票名称
    let db_path = base_path.join(STOCK_DB_PATH);
    if let Some(names) = db_path.exists()
        .then(|| load_stock_names(&db_path))
        .and_then(|r| r.map_err(|e| warn!("Failed to read StockDB: {}", e)).ok())
    {
        let matched = entries.iter_mut()
            .filter_map(|e| names.get(&e.stock_id).map(|n| e.name = n.clone()))
            .count();
        info!("Filled {} / {} stock names from StockDB", matched, entries.len());
    }

    Ok(entries)
}

/// 从 StockDB SQLite 读取 stock_id → 中文名 映射
fn load_stock_names(db_path: &Path) -> Result<HashMap<u64, String>> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("Failed to open StockDB: {}", db_path.display()))?;

    let mut stmt = conn.prepare("SELECT ID, zh FROM Stock WHERE zh IS NOT NULL AND zh != ''")?;
    let mut names = HashMap::new();

    let rows = stmt.query_map([], |row| {
        let id: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        Ok((id as u64, name))
    })?;

    for row in rows.flatten() {
        names.insert(row.0, row.1);
    }

    info!("Loaded {} stock names from StockDB", names.len());
    Ok(names)
}

/// 解析 plist 数据结构，提取自选股信息
///
/// 富途 watchstockContainer.dat 实际结构:
/// ```text
/// {
///   "ReservedGroups" => [
///     {
///       "FLWatchGroupKeyName" => "全部"
///       "FLWatchGroupKeyID" => 1000
///       "FLWatchGroupKeyStocks" => [
///         {
///           "FLStockKeyCode" => "00700"
///           "FLStockKeyID" => 54047868453564
///           "FLStockKeyPriceHighPrecision" => 606000000000
///           "FLStockKeyLastClosePriceHighPrecision" => 605000000000
///         }
///         ...
///       ]
///     }
///     ...
///   ]
/// }
/// ```
fn parse_watchlist_plist(value: &plist::Value) -> Result<Vec<WatchlistEntry>> {
    let dict = match value {
        plist::Value::Dictionary(dict) => dict,
        _ => {
            anyhow::bail!("Unexpected plist top-level type: {:?}", value_type_name(value));
        }
    };

    // 查找 ReservedGroups 数组
    let groups = match dict.get("ReservedGroups") {
        Some(plist::Value::Array(arr)) => arr,
        _ => {
            anyhow::bail!("plist 中未找到 ReservedGroups 数组");
        }
    };

    // 优先使用 "全部" 分组 (ID=1000)，它包含所有自选股
    // 如果找不到就合并所有分组并去重
    let mut entries = Vec::new();
    let mut all_group_found = false;

    for group_value in groups {
        let group_dict = match group_value {
            plist::Value::Dictionary(d) => d,
            _ => continue,
        };

        let group_name = extract_string(group_dict, &["FLWatchGroupKeyName"])
            .unwrap_or_default();
        let group_id = extract_integer(group_dict, &["FLWatchGroupKeyID"])
            .unwrap_or(0);

        debug!("Found watchlist group: {} (ID={})", group_name, group_id);

        // "全部" 分组 (ID=1000) 包含所有股票
        if group_id == 1000 || group_name == "全部" {
            entries = parse_group_stocks(group_dict)?;
            all_group_found = true;
            info!(
                "Using '全部' group (ID={}): {} stocks",
                group_id,
                entries.len()
            );
            break;
        }
    }

    // 如果没找到 "全部" 分组，合并所有分组
    if !all_group_found {
        warn!("未找到 '全部' 分组，合并所有分组");
        let mut seen_codes = std::collections::HashSet::new();
        for group_value in groups {
            let group_dict = match group_value {
                plist::Value::Dictionary(d) => d,
                _ => continue,
            };
            if let Ok(group_entries) = parse_group_stocks(group_dict) {
                for entry in group_entries {
                    let key = entry.code.display_code();
                    if seen_codes.insert(key) {
                        entries.push(entry);
                    }
                }
            }
        }
    }

    info!("Parsed {} watchlist entries", entries.len());
    Ok(entries)
}

/// 从一个分组字典中解析股票列表
fn parse_group_stocks(group_dict: &plist::Dictionary) -> Result<Vec<WatchlistEntry>> {
    let stocks_arr = match group_dict.get("FLWatchGroupKeyStocks") {
        Some(plist::Value::Array(arr)) => arr,
        _ => return Ok(Vec::new()),
    };

    let mut entries = Vec::new();
    for (idx, stock_value) in stocks_arr.iter().enumerate() {
        let stock_dict = match stock_value {
            plist::Value::Dictionary(d) => d,
            _ => continue,
        };

        if let Some(entry) = parse_futu_stock_entry(stock_dict, idx) {
            entries.push(entry);
        }
    }

    Ok(entries)
}

/// 从单个股票字典中提取信息
///
/// 实际 key:
/// - `FLStockKeyCode`: 股票代码字符串，如 "00700", "000001", "CNmain"
/// - `FLStockKeyID`: 内部数字 ID（HK 股票用大数字，A 股用 1/2 前缀）
/// - `FLStockKeyPriceHighPrecision`: 最新价（高精度整数）
/// - `FLStockKeyLastClosePriceHighPrecision`: 昨收价（高精度整数）
fn parse_futu_stock_entry(
    dict: &plist::Dictionary,
    sort_index: usize,
) -> Option<WatchlistEntry> {
    // 优先用 FLStockKeyCode（直接的代码字符串）
    let code_str = extract_string(dict, &["FLStockKeyCode"])?;

    // 获取内部 ID（用于推断市场）
    let stock_id = extract_integer(dict, &["FLStockKeyID"]);

    // 推断市场
    let market = infer_market_from_id_and_code(stock_id, &code_str);

    // 提取价格
    let price_raw = extract_integer(
        dict,
        &["FLStockKeyPriceHighPrecision"],
    );
    let prev_close_raw = extract_integer(
        dict,
        &["FLStockKeyLastClosePriceHighPrecision"],
    );

    let cached_price = price_raw
        .filter(|&p| p > 0)
        .map(|p| p as f64 / PRICE_DIVISOR);
    let _prev_close = prev_close_raw
        .filter(|&p| p > 0)
        .map(|p| p as f64 / PRICE_DIVISOR);

    debug!(
        "Stock: {} (ID={:?}, market={:?}, price={:?})",
        code_str, stock_id, market, cached_price
    );

    Some(WatchlistEntry {
        code: StockCode::new(market, &code_str),
        stock_id: stock_id.unwrap_or(0),
        name: String::new(), // 名称后续从 StockDB 填充
        cached_price,
        sort_index,
    })
}

/// 根据 FLStockKeyID 和代码字符串推断市场
fn infer_market_from_id_and_code(stock_id: Option<u64>, code_str: &str) -> Market {
    // 先用 FLStockKeyID 前缀判断（对 A 股指数有效）
    if let Some(id) = stock_id {
        if id >= 1_000_000 && id < 2_000_000 {
            return Market::SH;
        }
        if id >= 2_000_000 && id < 3_000_000 {
            return Market::SZ;
        }
        // 200xxx = 美股（.DJI=200001, TSLA=201335 等）
        if id >= 200_000 && id < 300_000 {
            return Market::US;
        }
        // 800xxx = 港股指数 (恒指等)
        if id >= 800_000 && id < 900_000 {
            return Market::HK;
        }
    }

    // 根据代码字符串模式推断
    if code_str.chars().all(|c| c.is_ascii_digit()) {
        let len = code_str.len();
        // 800xxx = 港股指数
        if len == 6 && code_str.starts_with("800") {
            return Market::HK;
        }
        if len == 5 {
            // 5 位 → 港股
            return Market::HK;
        }
        if len == 6 {
            // 6 位 → A 股，根据首位区分
            if code_str.starts_with('6') || code_str.starts_with('5') {
                return Market::SH;
            }
            if code_str.starts_with('0') || code_str.starts_with('3') || code_str.starts_with('1') {
                return Market::SZ;
            }
        }
        // 4 位或更短的数字 → 港股
        if len <= 4 {
            return Market::HK;
        }
    }

    // 含字母的代码
    // 美股代码：1-5个大写字母（如 TSLA, AAPL, NIO, BA, EDU）
    // 美股指数：以.开头（如 .DJI, .IXIC）
    if code_str.starts_with('.') && code_str.len() >= 2 {
        return Market::US;
    }
    if code_str.len() <= 5 && code_str.chars().all(|c| c.is_ascii_uppercase()) {
        return Market::US;
    }

    // 其他含字母的代码（CNmain, USDCNH 等）→ Unknown
    Market::Unknown
}

/// 从字典中提取整数值（尝试多个可能的 key）
fn extract_integer(dict: &plist::Dictionary, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(val) = dict.get(*key) {
            match val {
                plist::Value::Integer(i) => {
                    return i.as_unsigned().or_else(|| i.as_signed().map(|v| v as u64));
                }
                plist::Value::Real(f) => return Some(*f as u64),
                plist::Value::String(s) => return s.parse::<u64>().ok(),
                _ => {}
            }
        }
    }
    None
}

/// 从字典中提取字符串值
fn extract_string(dict: &plist::Dictionary, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(plist::Value::String(s)) = dict.get(*key) {
            if !s.is_empty() {
                return Some(s.clone());
            }
        }
    }
    None
}

fn value_type_name(value: &plist::Value) -> &'static str {
    match value {
        plist::Value::Array(_) => "Array",
        plist::Value::Dictionary(_) => "Dictionary",
        plist::Value::Boolean(_) => "Boolean",
        plist::Value::Data(_) => "Data",
        plist::Value::Date(_) => "Date",
        plist::Value::Integer(_) => "Integer",
        plist::Value::Real(_) => "Real",
        plist::Value::String(_) => "String",
        plist::Value::Uid(_) => "Uid",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Market;

    #[test]
    fn test_stock_code_from_futu_id() {
        // 沪市
        let code = StockCode::from_futu_id(1600519);
        assert_eq!(code.market, Market::SH);
        assert_eq!(code.code, "600519");

        // 深市
        let code = StockCode::from_futu_id(2000001);
        assert_eq!(code.market, Market::SZ);
        assert_eq!(code.code, "000001");

        // 港股
        let code = StockCode::from_futu_id(700);
        assert_eq!(code.market, Market::HK);
        assert_eq!(code.code, "00700");
    }

    #[test]
    fn test_price_conversion() {
        // 上证指数: raw 4117947600000 / 10^9 = 4117.95
        let raw_price: u64 = 4117947600000;
        let actual = raw_price as f64 / PRICE_DIVISOR;
        assert!((actual - 4117.9476).abs() < 0.001);

        // 腾讯 00700: raw 606000000000 / 10^9 = 606.0
        let raw_price: u64 = 606000000000;
        let actual = raw_price as f64 / PRICE_DIVISOR;
        assert!((actual - 606.0).abs() < 0.01);

        // 恒指: raw 27387110000000 / 10^9 = 27387.11
        let raw_price: u64 = 27387110000000;
        let actual = raw_price as f64 / PRICE_DIVISOR;
        assert!((actual - 27387.11).abs() < 0.01);

        // USDCNH: raw 6958100000 / 10^9 = 6.9581
        let raw_price: u64 = 6958100000;
        let actual = raw_price as f64 / PRICE_DIVISOR;
        assert!((actual - 6.9581).abs() < 0.001);
    }

    #[test]
    fn test_infer_market_from_id_and_code() {
        // A 股沪市指数 — ID 以 1 开头
        assert_eq!(infer_market_from_id_and_code(Some(1000001), "000001"), Market::SH);
        // A 股深市指数 — ID 以 2 开头
        assert_eq!(infer_market_from_id_and_code(Some(2399006), "399006"), Market::SZ);
        // 港股 — 5 位数字
        assert_eq!(infer_market_from_id_and_code(Some(54047868453564), "00700"), Market::HK);
        assert_eq!(infer_market_from_id_and_code(None, "09988"), Market::HK);
        // 港股指数 — 800xxx
        assert_eq!(infer_market_from_id_and_code(Some(800000), "800000"), Market::HK);
        assert_eq!(infer_market_from_id_and_code(None, "800700"), Market::HK);
        // A 股 — 6 位以 6 开头
        assert_eq!(infer_market_from_id_and_code(None, "600519"), Market::SH);
        // A 股 — 6 位以 0 开头
        assert_eq!(infer_market_from_id_and_code(None, "000858"), Market::SZ);
        // A 股 — 6 位以 3 开头（创业板）
        assert_eq!(infer_market_from_id_and_code(None, "300750"), Market::SZ);
        // 美股 — 大写字母代码
        assert_eq!(infer_market_from_id_and_code(None, "TSLA"), Market::US);
        assert_eq!(infer_market_from_id_and_code(None, "AAPL"), Market::US);
        assert_eq!(infer_market_from_id_and_code(None, "NIO"), Market::US);
        assert_eq!(infer_market_from_id_and_code(None, "BA"), Market::US);
        // 美股指数 — 以.开头
        assert_eq!(infer_market_from_id_and_code(None, ".DJI"), Market::US);
        assert_eq!(infer_market_from_id_and_code(None, ".IXIC"), Market::US);
        // 美股 — ID 范围 200000-299999
        assert_eq!(infer_market_from_id_and_code(Some(201335), "TSLA"), Market::US);
        // 特殊代码（非股票）
        assert_eq!(infer_market_from_id_and_code(None, "CNmain"), Market::Unknown);
        assert_eq!(infer_market_from_id_and_code(None, "USDCNH"), Market::Unknown);
    }

    #[test]
    fn test_parse_watchlist_plist_structure() {
        // 构造一个模拟的 plist 结构
        let mut stock1 = plist::Dictionary::new();
        stock1.insert("FLStockKeyCode".into(), plist::Value::String("00700".into()));
        stock1.insert("FLStockKeyID".into(), plist::Value::Integer(54047868453564_i64.into()));
        stock1.insert("FLStockKeyPriceHighPrecision".into(), plist::Value::Integer(606000000000_i64.into()));

        let mut stock2 = plist::Dictionary::new();
        stock2.insert("FLStockKeyCode".into(), plist::Value::String("600519".into()));
        stock2.insert("FLStockKeyID".into(), plist::Value::Integer(1600519_i64.into()));
        stock2.insert("FLStockKeyPriceHighPrecision".into(), plist::Value::Integer(1500000000000_i64.into()));

        let mut group = plist::Dictionary::new();
        group.insert("FLWatchGroupKeyName".into(), plist::Value::String("全部".into()));
        group.insert("FLWatchGroupKeyID".into(), plist::Value::Integer(1000_i64.into()));
        group.insert("FLWatchGroupKeyStocks".into(), plist::Value::Array(vec![
            plist::Value::Dictionary(stock1),
            plist::Value::Dictionary(stock2),
        ]));

        let mut root = plist::Dictionary::new();
        root.insert("ReservedGroups".into(), plist::Value::Array(vec![
            plist::Value::Dictionary(group),
        ]));

        let entries = parse_watchlist_plist(&plist::Value::Dictionary(root)).unwrap();
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].code.code, "00700");
        assert_eq!(entries[0].code.market, Market::HK);
        assert!(entries[0].cached_price.is_some());

        assert_eq!(entries[1].code.code, "600519");
        assert_eq!(entries[1].code.market, Market::SH);
        assert!(entries[1].cached_price.is_some());
    }
}
