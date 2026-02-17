//! 行情数据解析器
//!
//! 从原始文本（Accessibility API 读取）或其他来源解析为 QuoteSnapshot

use crate::models::{DataSource, Market, QuoteSnapshot, StockCode};
use chrono::Local;

/// 尝试从一行文本中解析行情数据
/// 富途 App 中行情表格行通常格式类似：
/// "腾讯控股  00700  388.00  +2.60  +0.67%  1234万"
/// 或以 tab/空格分隔
pub fn try_parse_quote_text(text: &str) -> Option<QuoteSnapshot> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    // 尝试按多种分隔符拆分
    let parts: Vec<&str> = text
        .split(|c: char| c == '\t' || c == '|')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if parts.len() < 3 {
        // 尝试按连续空格拆分
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() >= 3 {
            return try_parse_parts(&parts);
        }
        return None;
    }

    try_parse_parts(&parts)
}

/// 从拆分后的字段中解析行情
fn try_parse_parts(parts: &[&str]) -> Option<QuoteSnapshot> {
    // 查找包含股票代码的字段
    let (code_idx, code) = find_stock_code(parts)?;

    // 查找价格字段（第一个像数字的字段，在代码之后）
    let mut price = None;
    let mut change = None;
    let mut change_pct = None;
    let mut volume = None;
    let mut name = None;

    for (i, part) in parts.iter().enumerate() {
        if i == code_idx {
            continue;
        }

        // 代码前面的可能是名称
        if i < code_idx && name.is_none() {
            if !looks_like_number(part) {
                name = Some(part.to_string());
                continue;
            }
        }

        if let Some(p) = parse_price(part) {
            if price.is_none() && i > code_idx {
                price = Some(p);
            } else if change.is_none() {
                change = Some(p);
            }
        } else if let Some(pct) = parse_percentage(part) {
            change_pct = Some(pct);
        } else if let Some(vol) = parse_volume(part) {
            volume = Some(vol);
        } else if name.is_none() && !looks_like_number(part) {
            name = Some(part.to_string());
        }
    }

    let last_price = price?;

    Some(QuoteSnapshot {
        code,
        name: name.unwrap_or_default(),
        last_price,
        prev_close: if let (Some(chg), _) = (change, change_pct) {
            last_price - chg
        } else {
            0.0
        },
        open_price: 0.0,
        high_price: 0.0,
        low_price: 0.0,
        volume: volume.unwrap_or(0),
        turnover: 0.0,
        change: change.unwrap_or(0.0),
        change_pct: change_pct.unwrap_or(0.0),
        turnover_rate: 0.0,
        amplitude: 0.0,
        extended_price: None,
        extended_change_pct: None,
        timestamp: Local::now(),
        source: DataSource::Accessibility,
    })
}

/// 在字段列表中查找股票代码
fn find_stock_code(parts: &[&str]) -> Option<(usize, StockCode)> {
    for (i, part) in parts.iter().enumerate() {
        if let Some(code) = parse_stock_code(part) {
            return Some((i, code));
        }
    }
    None
}

/// 解析股票代码字符串
/// 支持格式：00700, HK.00700, 600519, SH.600519, 000001.SZ 等
pub fn parse_stock_code(s: &str) -> Option<StockCode> {
    let s = s.trim();

    // 带市场前缀：HK.00700, SH.600519, SZ.000001
    if let Some((market_str, code)) = s.split_once('.') {
        let market = match market_str.to_uppercase().as_str() {
            "HK" => Some(Market::HK),
            "SH" => Some(Market::SH),
            "SZ" => Some(Market::SZ),
            "US" => Some(Market::US),
            "SG" => Some(Market::SG),
            "FX" => Some(Market::FX),
            _ => None,
        };

        // 也可能是 600519.SH 格式
        if market.is_none() {
            let market2 = match code.to_uppercase().as_str() {
                "HK" => Some(Market::HK),
                "SH" => Some(Market::SH),
                "SZ" => Some(Market::SZ),
                "US" => Some(Market::US),
                "SG" => Some(Market::SG),
                "FX" => Some(Market::FX),
                _ => None,
            };
            if let Some(m) = market2 {
                if market_str.chars().all(|c| c.is_ascii_digit()) {
                    return Some(StockCode::new(m, market_str));
                }
            }
        }

        if let Some(m) = market {
            if !code.is_empty()
                && (code.chars().all(|c| c.is_ascii_digit())
                    || (m == Market::US && code.chars().all(|c| c.is_ascii_alphabetic()))
                    || (m == Market::SG && code.chars().all(|c| c.is_ascii_alphanumeric()))
                    || (m == Market::FX && code.chars().all(|c| c.is_ascii_alphanumeric())))
            {
                // OCR 可能输出混合大小写，US ticker 统一转大写
                let normalized = if m == Market::US {
                    code.to_ascii_uppercase()
                } else {
                    code.to_string()
                };
                return Some(StockCode::new(m, &normalized));
            }
        }
    }

    // 美股字母代码：1-5 个字母（可带前导点号如 .DJI/.IXIC 表示指数，尾随点号为 OCR 噪声）
    // OCR 可能输出混合大小写（如 "Li" → "LI"），统一转大写
    // 排除市场前缀 HK/SH/SZ/US，这些是市场标识不是股票代码
    {
        let stripped = s.trim_end_matches('.');
        let alpha_part = stripped.trim_start_matches('.');
        if !alpha_part.is_empty() && alpha_part.len() <= 5 && alpha_part.chars().all(|c| c.is_ascii_alphabetic()) {
            let upper = alpha_part.to_ascii_uppercase();
            if !matches!(upper.as_str(), "HK" | "SH" | "SZ" | "US" | "SG" | "FX") {
                // 保留前导点号（.DJI/.IXIC 等指数代码）
                let code = if stripped.starts_with('.') {
                    format!(".{}", upper)
                } else {
                    upper
                };
                return Some(StockCode::new(Market::US, &code));
            }
        }
    }

    // 纯数字：根据代码长度和前缀推断市场
    if s.chars().all(|c| c.is_ascii_digit()) && s.len() >= 4 && s.len() <= 6 {
        let code_num: u32 = s.parse().ok()?;
        let market = if s.len() == 5 || (s.len() <= 5 && code_num < 100_000) {
            // 5位或更短 → 港股
            Market::HK
        } else if s.starts_with('6') {
            Market::SH // 沪市主板
        } else if s.starts_with("00") || s.starts_with('3') || s.starts_with('0') {
            Market::SZ // 深市主板/创业板
        } else if s.starts_with('1') {
            Market::SZ // 深市 ETF/基金 (159xxx, 15xxxx)
        } else if s.starts_with('8') {
            Market::HK // 港股指数 (800000=恒生指数, 800700=恒生科技指数 等)
        } else {
            return None;
        };

        return Some(StockCode::new(market, s));
    }

    None
}

/// 解析价格字符串
fn parse_price(s: &str) -> Option<f64> {
    let s = s.trim().trim_start_matches('+');
    s.parse::<f64>().ok()
}

/// 解析百分比字符串 "+0.67%" → 0.67
fn parse_percentage(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.ends_with('%') {
        let num_str = s.trim_end_matches('%').trim_start_matches('+');
        return num_str.parse::<f64>().ok();
    }
    None
}

/// 解析成交量字符串 "1234万" → 12340000, "1.2亿" → 120000000
fn parse_volume(s: &str) -> Option<u64> {
    let s = s.trim();

    if s.ends_with('万') {
        let num_str = s.trim_end_matches('万');
        let num: f64 = num_str.parse().ok()?;
        return Some((num * 10_000.0) as u64);
    }

    if s.ends_with('亿') {
        let num_str = s.trim_end_matches('亿');
        let num: f64 = num_str.parse().ok()?;
        return Some((num * 100_000_000.0) as u64);
    }

    // K/M/B 格式
    if s.ends_with('K') || s.ends_with('k') {
        let num_str = &s[..s.len() - 1];
        let num: f64 = num_str.parse().ok()?;
        return Some((num * 1_000.0) as u64);
    }

    if s.ends_with('M') || s.ends_with('m') {
        let num_str = &s[..s.len() - 1];
        let num: f64 = num_str.parse().ok()?;
        return Some((num * 1_000_000.0) as u64);
    }

    s.replace(',', "").parse::<u64>().ok()
}

/// 判断字符串是否看起来像数字
fn looks_like_number(s: &str) -> bool {
    let s = s.trim().trim_start_matches('+').trim_start_matches('-');
    if s.is_empty() {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == '%')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stock_code() {
        let code = parse_stock_code("HK.00700").unwrap();
        assert_eq!(code.market, Market::HK);
        assert_eq!(code.code, "00700");

        let code = parse_stock_code("SH.600519").unwrap();
        assert_eq!(code.market, Market::SH);
        assert_eq!(code.code, "600519");

        let code = parse_stock_code("600519.SH").unwrap();
        assert_eq!(code.market, Market::SH);
        assert_eq!(code.code, "600519");

        let code = parse_stock_code("00700").unwrap();
        assert_eq!(code.market, Market::HK);

        // US tickers
        let code = parse_stock_code("AAPL").unwrap();
        assert_eq!(code.market, Market::US);
        assert_eq!(code.code, "AAPL");

        let code = parse_stock_code("US.TSLA").unwrap();
        assert_eq!(code.market, Market::US);
        assert_eq!(code.code, "TSLA");

        // Index with dot prefix (preserved)
        let code = parse_stock_code(".DJI").unwrap();
        assert_eq!(code.market, Market::US);
        assert_eq!(code.code, ".DJI");

        let code = parse_stock_code(".IXIC").unwrap();
        assert_eq!(code.market, Market::US);
        assert_eq!(code.code, ".IXIC");

        // Trailing dot is OCR noise, stripped
        let code = parse_stock_code("NVDA.").unwrap();
        assert_eq!(code.market, Market::US);
        assert_eq!(code.code, "NVDA");

        // HK index codes
        let code = parse_stock_code("800000").unwrap();
        assert_eq!(code.market, Market::HK);
        assert_eq!(code.code, "800000");

        let code = parse_stock_code("800700").unwrap();
        assert_eq!(code.market, Market::HK);
        assert_eq!(code.code, "800700");

        // SG market
        let code = parse_stock_code("SG.D05").unwrap();
        assert_eq!(code.market, Market::SG);
        assert_eq!(code.code, "D05");

        // FX should NOT be parsed as US ticker
        assert!(parse_stock_code("FX").is_none());
        // SG as standalone should NOT be parsed as US ticker
        assert!(parse_stock_code("SG").is_none());
    }

    #[test]
    fn test_parse_percentage() {
        assert_eq!(parse_percentage("+0.67%"), Some(0.67));
        assert_eq!(parse_percentage("-1.23%"), Some(-1.23));
        assert_eq!(parse_percentage("0%"), Some(0.0));
        assert_eq!(parse_percentage("abc"), None);
    }

    #[test]
    fn test_parse_volume() {
        assert_eq!(parse_volume("1234万"), Some(12_340_000));
        assert_eq!(parse_volume("1.2亿"), Some(120_000_000));
        assert_eq!(parse_volume("500K"), Some(500_000));
        assert_eq!(parse_volume("1,234,567"), Some(1_234_567));
    }

    #[test]
    fn test_try_parse_quote_text() {
        let text = "腾讯控股\t00700\t388.00\t+2.60\t+0.67%";
        let quote = try_parse_quote_text(text).unwrap();
        assert_eq!(quote.code.code, "00700");
        assert_eq!(quote.last_price, 388.00);
        assert_eq!(quote.change_pct, 0.67);
    }
}
