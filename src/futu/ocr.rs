//! 窗口截图 + Apple Vision OCR 数据获取
//!
//! 通过 CGWindowListCreateImage 截取富途牛牛窗口，
//! 使用 Vision 框架的 VNRecognizeTextRequest 识别文字，
//! 按 Y 坐标聚类成行后复用现有 parser 解析为 QuoteSnapshot。

#![allow(deprecated)] // CGWindowListCreateImage / CGWindowListCopyWindowInfo

use anyhow::{Context, Result};
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGImage, CGImageCreateWithImageInRect, CGWindowID, CGWindowImageOption,
    CGWindowListCopyWindowInfo, CGWindowListCreateImage, CGWindowListOption,
};
use objc2_foundation::{NSArray, NSDictionary, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation,
    VNRequest, VNRequestTextRecognitionLevel,
};
use tracing::{debug, info, warn};

use crate::models::{DataSource, Market, QuoteSnapshot, StockCode};

/// OCR 识别出的文字块
#[derive(Debug, Clone)]
pub struct OcrTextBlock {
    pub text: String,
    pub confidence: f32,
    /// 归一化 bounding box (x, y, w, h)，原点左下角
    pub bbox: (f64, f64, f64, f64),
}

/// 窗口信息（ID + 尺寸 + 所属进程 PID）
#[derive(Debug, Clone, Copy)]
pub struct WindowInfo {
    pub id: u32,
    pub width: f64,
    pub height: f64,
    pub owner_pid: i32,
}

/// 查找富途牛牛 App 的主窗口 ID 和尺寸
///
/// 通过 CGWindowListCopyWindowInfo 获取所有窗口，
/// 按 owner name 匹配 "Futu" / "Niuniu" / "牛牛"，选面积最大的。
/// 不依赖单一 PID，避免多进程场景找不到窗口。
pub fn find_futu_window(pid: i32) -> Result<WindowInfo> {
    let info_list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionAll,
        0, // kCGNullWindowID
    )
    .context("CGWindowListCopyWindowInfo returned null")?;

    let cf_arr_ptr = CFRetained::as_ptr(&info_list).as_ptr() as *const _;
    let count = unsafe { core_foundation::array::CFArrayGetCount(cf_arr_ptr) };
    debug!("CGWindowListCopyWindowInfo: {} windows total", count);

    let mut best: Option<(u32, f64, f64, i32)> = None;
    let mut best_area: f64 = 0.0;

    for i in 0..count {
        let dict_ptr = unsafe {
            core_foundation::array::CFArrayGetValueAtIndex(cf_arr_ptr, i)
        };
        if dict_ptr.is_null() {
            continue;
        }

        // 优先按 PID 匹配，也按 owner name 匹配（覆盖多进程场景）
        let owner_pid = unsafe { dict_get_i32(dict_ptr, "kCGWindowOwnerPID") };
        let owner_name = unsafe { dict_get_string(dict_ptr, "kCGWindowOwnerName") };

        let pid_match = owner_pid == Some(pid);
        let name_match = owner_name
            .as_ref()
            .map(|n| n.contains("Futu") || n.contains("Niuniu") || n.contains("牛牛"))
            .unwrap_or(false);

        if !pid_match && !name_match {
            continue;
        }

        // 读取 kCGWindowNumber
        let window_id = unsafe { dict_get_i32(dict_ptr, "kCGWindowNumber") };

        // 读取 kCGWindowBounds
        let (w, h) = unsafe { dict_get_window_bounds(dict_ptr) }.unwrap_or((0.0, 0.0));
        let area = w * h;

        // 过滤太小的窗口（菜单、浮层等）
        if area < 10000.0 {
            continue;
        }

        debug!(
            "  owner={:?} pid={:?} window_id={:?} size={}x{} area={}",
            owner_name, owner_pid, window_id, w, h, area
        );

        if area > best_area {
            best_area = area;
            if let (Some(wid), Some(opid)) = (window_id, owner_pid) {
                best = Some((wid as u32, w, h, opid));
            }
        }
    }

    best.map(|(id, width, height, owner_pid)| WindowInfo { id, width, height, owner_pid })
        .context("未找到富途牛牛窗口。请确认 App 已启动且窗口未最小化。")
}

/// 兼容旧接口：只返回窗口 ID
pub fn find_futu_window_id(pid: i32) -> Result<u32> {
    find_futu_window(pid).map(|w| w.id)
}

/// 检查是否拥有屏幕录制权限
pub fn check_screen_capture_permission() -> bool {
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 请求屏幕录制权限（会弹出系统对话框）
pub fn request_screen_capture_permission() -> bool {
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    unsafe { CGRequestScreenCaptureAccess() }
}

/// 截取指定窗口的截图
pub fn capture_window(window_id: u32) -> Result<CFRetained<CGImage>> {
    // CGRectNull: origin=(inf,inf), size=(0,0) — 表示自动适配窗口边界
    let null_rect = CGRect {
        origin: CGPoint {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        size: CGSize {
            width: 0.0,
            height: 0.0,
        },
    };

    // 优先 BestResolution | BoundsIgnoreFraming，失败则降级到 NominalResolution | BoundsIgnoreFraming
    let options: &[(CGWindowImageOption, &str)] = &[
        (CGWindowImageOption::from_bits_retain(0x09), "BestResolution"),
        (CGWindowImageOption::from_bits_retain(0x11), "NominalResolution"),
    ];

    for (opt, label) in options {
        if let Some(image) = CGWindowListCreateImage(
            null_rect,
            CGWindowListOption::OptionIncludingWindow,
            window_id as CGWindowID,
            *opt,
        ) {
            debug!(
                "Captured window {} ({}) -> image {}x{}",
                window_id,
                label,
                CGImage::width(Some(&image)),
                CGImage::height(Some(&image)),
            );
            return Ok(image);
        }
        debug!("CGWindowListCreateImage failed with {} for window {}", label, window_id);
    }

    // 全部失败，生成详细错误
    if !check_screen_capture_permission() {
        anyhow::bail!(
            "CGWindowListCreateImage returned null: 缺少屏幕录制权限。\n\
             请前往 系统设置 → 隐私与安全性 → 屏幕录制 中授权当前终端 App"
        );
    }
    anyhow::bail!(
        "CGWindowListCreateImage returned null (window_id={}): 窗口可能已最小化、不可见或正在调整大小",
        window_id
    );
}

/// 使用 Apple Vision 框架识别图片中的文字
///
/// `accurate` 为 true 时使用精确模式（慢），false 时使用快速模式（用于布局检测）
fn recognize_text_with_level(image: &CGImage, accurate: bool) -> Result<Vec<OcrTextBlock>> {
    unsafe {
        let empty_dict: Retained<NSDictionary<NSString>> = NSDictionary::new();
        let handler = VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            image,
            &empty_dict,
        );

        let request = VNRecognizeTextRequest::new();

        // Accurate = 0, Fast = 1
        let level = if accurate { 0 } else { 1 };
        request.setRecognitionLevel(VNRequestTextRecognitionLevel(level));

        let zh = NSString::from_str("zh-Hans");
        let en = NSString::from_str("en-US");
        let languages = NSArray::from_retained_slice(&[zh, en]);
        request.setRecognitionLanguages(&languages);

        let request_as_vn: Retained<VNRequest> = Retained::cast_unchecked(request.clone());
        let requests = NSArray::from_retained_slice(&[request_as_vn]);
        handler
            .performRequests_error(&requests)
            .map_err(|e| anyhow::anyhow!("Vision OCR failed: {}", e))?;

        let mut blocks = Vec::new();
        if let Some(results) = request.results() {
            let count = results.count();
            for i in 0..count {
                let obs: &VNRecognizedTextObservation = &results.objectAtIndex(i);
                let candidates = obs.topCandidates(1);
                if candidates.count() == 0 {
                    continue;
                }

                let candidate = candidates.objectAtIndex(0);
                let text = candidate.string().to_string();
                let confidence = candidate.confidence();

                let bbox = obs.boundingBox();
                blocks.push(OcrTextBlock {
                    text,
                    confidence,
                    bbox: (
                        bbox.origin.x,
                        bbox.origin.y,
                        bbox.size.width,
                        bbox.size.height,
                    ),
                });
            }
        }

        Ok(blocks)
    }
}

/// 精确模式 OCR（用于最终识别）
pub fn recognize_text(image: &CGImage) -> Result<Vec<OcrTextBlock>> {
    recognize_text_with_level(image, true)
}

/// 快速模式 OCR（用于布局检测）
pub fn recognize_text_fast(image: &CGImage) -> Result<Vec<OcrTextBlock>> {
    recognize_text_with_level(image, false)
}

/// 窗口布局信息（归一化坐标 0.0-1.0）
#[derive(Debug, Clone)]
pub struct WindowLayout {
    /// 自选股列表区域 x 范围
    pub watchlist_x: (f64, f64),
    /// 报价详情区域 x 范围
    pub quote_x: Option<(f64, f64)>,
}

/// 通过快速 OCR 检测窗口布局
///
/// 查找关键地标文字来确定各区域边界：
/// - 自选股：`名称代码`/`名称` + `最新价` + `涨跌幅` 表头
/// - 报价：`报价` 标签
pub fn detect_layout(blocks: &[OcrTextBlock]) -> WindowLayout {
    // 自选股表头关键词（在窗口左半部分查找）
    let watchlist_keywords = ["涨跌幅", "涨跌", "最新价", "名称代码", "名称"];
    let mut watchlist_right: f64 = 0.0;
    let mut found_watchlist = false;

    for block in blocks {
        if block.bbox.0 > 0.4 {
            continue; // 只在左 40% 查找自选股表头
        }
        for kw in &watchlist_keywords {
            if block.text.contains(kw) {
                let right = block.bbox.0 + block.bbox.2;
                if right > watchlist_right {
                    watchlist_right = right;
                    found_watchlist = true;
                }
                break;
            }
        }
    }

    // 自选股右边界 + 3% 余量（数值比表头宽）
    let watchlist_x = if found_watchlist {
        (0.0, (watchlist_right + 0.03).min(0.40))
    } else {
        (0.0, 0.22) // 默认
    };

    // 报价区域：查找 "报价" 标签位置
    let mut quote_left: Option<f64> = None;
    for block in blocks {
        if block.bbox.0 > 0.5 && block.text.contains("报价") {
            quote_left = Some(block.bbox.0);
            break;
        }
    }

    // 如果没找到 "报价"，尝试找 "最高价"/"开盘价" 等详情字段
    if quote_left.is_none() {
        let detail_keywords = ["最高价", "开盘价", "昨收价", "成交量", "市盈率"];
        let mut min_x: f64 = 1.0;
        for block in blocks {
            if block.bbox.0 > 0.5 {
                for kw in &detail_keywords {
                    if block.text.contains(kw) {
                        if block.bbox.0 < min_x {
                            min_x = block.bbox.0;
                        }
                        break;
                    }
                }
            }
        }
        if min_x < 1.0 {
            quote_left = Some((min_x - 0.02).max(0.5));
        }
    }

    let quote_x = quote_left.map(|left| (left, 1.0));

    debug!(
        "Layout detected: watchlist=({:.3}, {:.3}), quote={:?}",
        watchlist_x.0, watchlist_x.1, quote_x
    );

    WindowLayout {
        watchlist_x,
        quote_x,
    }
}

/// 按归一化坐标裁剪图像区域
///
/// `x_range`: (left, right) 归一化 X 范围 0.0-1.0
/// `y_range`: 可选 (top, bottom) 归一化 Y 范围 0.0-1.0
/// 返回裁剪后的子图像
pub fn crop_image(
    image: &CGImage,
    x_range: (f64, f64),
) -> Result<CFRetained<CGImage>> {
    crop_image_xy(image, x_range, None)
}

/// 按归一化坐标裁剪图像区域（支持 X + Y 同时裁剪）
///
/// `x_range`: (left, right) 归一化 X 范围 0.0-1.0
/// `y_range`: 可选 (top, bottom) 归一化 Y 范围 0.0-1.0
pub fn crop_image_xy(
    image: &CGImage,
    x_range: (f64, f64),
    y_range: Option<(f64, f64)>,
) -> Result<CFRetained<CGImage>> {
    let w = CGImage::width(Some(image)) as f64;
    let h = CGImage::height(Some(image)) as f64;

    let (y_start, crop_h) = match y_range {
        Some((top, bottom)) => ((top * h).floor(), ((bottom - top) * h).ceil()),
        None => (0.0, h),
    };

    let rect = CGRect {
        origin: CGPoint {
            x: (x_range.0 * w).floor(),
            y: y_start,
        },
        size: CGSize {
            width: ((x_range.1 - x_range.0) * w).ceil(),
            height: crop_h,
        },
    };

    debug!(
        "Cropping image {}x{} to rect ({:.0}, {:.0}, {:.0}, {:.0})",
        w, h, rect.origin.x, rect.origin.y, rect.size.width, rect.size.height
    );

    CGImageCreateWithImageInRect(Some(image), rect)
        .context("CGImageCreateWithImageInRect returned null")
}

/// 将 OCR 文字块按 Y 坐标聚类成行
///
/// Vision 坐标原点在左下角，y=1.0 是顶部。
/// 转换为从上到下排序，Y 容差 0.5% 内归为同一行。
pub fn group_into_rows(blocks: &[OcrTextBlock]) -> Vec<Vec<&OcrTextBlock>> {
    if blocks.is_empty() {
        return Vec::new();
    }

    // 按 y 坐标排序（翻转：y 大的在上面，先排）
    let mut sorted: Vec<&OcrTextBlock> = blocks.iter().collect();
    sorted.sort_by(|a, b| {
        let ay = a.bbox.1 + a.bbox.3; // top edge (origin + height)
        let by = b.bbox.1 + b.bbox.3;
        by.partial_cmp(&ay).unwrap_or(std::cmp::Ordering::Equal)
    });

    let tolerance = 0.005; // 0.5% 容差
    let mut rows: Vec<Vec<&OcrTextBlock>> = Vec::new();

    for block in sorted {
        let block_y = block.bbox.1 + block.bbox.3; // top edge

        // 查找可归入的行
        let mut found = false;
        for row in rows.iter_mut() {
            let row_y = row[0].bbox.1 + row[0].bbox.3;
            if (block_y - row_y).abs() < tolerance {
                row.push(block);
                found = true;
                break;
            }
        }

        if !found {
            rows.push(vec![block]);
        }
    }

    // 行内按 X 坐标从左到右排序
    for row in rows.iter_mut() {
        row.sort_by(|a, b| {
            a.bbox.0.partial_cmp(&b.bbox.0).unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    rows
}

/// 从 OCR 行中解析自选股行情（两行配对格式）
///
/// 富途牛牛自选股每条目占两行：
///   第一行: `[HK/SH/SZ] [名称]  [价格]  [涨跌%]`
///   第二行: `[股票代码]`
///
/// 遇到股票代码时，将前面积累的名称/价格组装为 QuoteSnapshot。
pub fn parse_watchlist_from_ocr(rows: &[Vec<&OcrTextBlock>]) -> Vec<QuoteSnapshot> {
    use crate::data::parser::parse_stock_code;

    let mut quotes = Vec::new();
    let mut pending_market: Option<Market> = None;
    let mut pending_name: Option<String> = None;
    let mut pending_price: Option<f64> = None;
    let mut pending_change_pct: Option<f64> = None;

    for row in rows {
        let mut row_code: Option<StockCode> = None;
        let mut row_price: Option<f64> = None;
        let mut row_change_pct: Option<f64> = None;
        let mut row_market: Option<Market> = None;
        let mut row_name: Option<String> = None;

        for block in row {
            let text = block.text.trim();

            // 尝试解析为股票代码
            if row_code.is_none() {
                if let Some(sc) = parse_stock_code(text) {
                    row_code = Some(sc);
                    continue;
                }
            }

            // 尝试解析为涨跌百分比
            if row_change_pct.is_none() {
                if let Some(pct) = ocr_parse_pct(text) {
                    row_change_pct = Some(pct);
                    continue;
                }
            }

            // 尝试解析为 "HK 名称" / "SH 名称" / "SZ 名称" / "us 波音"
            if row_market.is_none() {
                if let Some((m, n)) = ocr_parse_market_name(text) {
                    row_market = Some(m);
                    // 单字名称可能是 OCR 噪声（"HK。" → "。"），至少 2 字才设名
                    if n.chars().count() >= 2 {
                        row_name = Some(n);
                    }
                    continue;
                }
            }

            // 独立市场前缀 "HK"/"SH"/"SZ"/"US" 及其 OCR 变体 "US："/"US）"
            // （OCR 有时将前缀和名称拆为独立块）
            if row_market.is_none() {
                let cleaned = text.to_uppercase()
                    .trim_end_matches(|c: char| !c.is_alphanumeric())
                    .to_string();
                if matches!(cleaned.as_str(), "HK" | "SH" | "SZ" | "US") {
                    row_market = Some(match cleaned.as_str() {
                        "HK" => Market::HK,
                        "SH" => Market::SH,
                        "SZ" => Market::SZ,
                        "US" => Market::US,
                        _ => unreachable!(),
                    });
                    continue;
                }
            }

            // 尝试解析为价格（正数）
            // 兼容 OCR 尾部噪声点号 "1.190." → 1.190
            // 过滤 sidebar 噪声小整数（"6"/"14"/"20"等）：小于 100 的价格必须含小数点
            if row_price.is_none() {
                let price_text = text.trim_end_matches('.');
                if let Ok(p) = price_text.parse::<f64>() {
                    if p > 0.0 && (p >= 100.0 || price_text.contains('.')) {
                        row_price = Some(p);
                        continue;
                    }
                }
            }

            // 纯中文名称（已有市场前缀但名称在独立块中）
            if row_market.is_some() && row_name.is_none() {
                if text.chars().next().map(|c| c > '\x7f').unwrap_or(false) {
                    row_name = Some(text.to_string());
                    continue;
                }
            }
        }

        // 如果本行有股票代码 → 与 pending 信息配对，生成 QuoteSnapshot
        if let Some(code) = row_code {
            let market = pending_market.take()
                .or(row_market.take())
                .unwrap_or(code.market);
            let name = pending_name.take()
                .or(row_name.take())
                .unwrap_or_default();

            // 主价格优先用 pending（上一行的名称行数据）
            // 代码行自身的价格/涨跌幅作为盘前/盘后扩展数据（美股）
            // 仅当代码行同时有价格和涨跌幅时才识别为扩展数据（排除噪声数字）
            let (price, change_pct, ext_price, ext_pct) = if let Some(pp) = pending_price.take() {
                let pct = pending_change_pct.take().unwrap_or(0.0);
                let has_extended = row_price.is_some() && row_change_pct.is_some();
                if has_extended {
                    (pp, pct, row_price.take(), row_change_pct.take())
                } else {
                    (pp, pct, None, None)
                }
            } else {
                // 无 pending → 代码行数据作为主价格（HK/A股单行格式）
                (row_price.take().unwrap_or(0.0), row_change_pct.take().unwrap_or(0.0), None, None)
            };

            if price > 0.0 {
                // 从涨跌幅反推涨跌额：change = price - price / (1 + pct/100)
                let change = if change_pct.abs() > f64::EPSILON {
                    price - price / (1.0 + change_pct / 100.0)
                } else {
                    0.0
                };

                quotes.push(QuoteSnapshot {
                    code: StockCode::new(market, &code.code),
                    name,
                    last_price: price,
                    prev_close: price - change,
                    open_price: 0.0,
                    high_price: 0.0,
                    low_price: 0.0,
                    volume: 0,
                    turnover: 0.0,
                    change,
                    change_pct,
                    turnover_rate: 0.0,
                    amplitude: 0.0,
                    extended_price: ext_price,
                    extended_change_pct: ext_pct,
                    timestamp: chrono::Local::now(),
                    source: DataSource::Ocr,
                });
            }
        } else {
            // 本行无代码 → 积累信息供下一行配对
            if row_market.is_some() {
                // 新条目开始（有市场前缀）
                pending_market = row_market;
                pending_name = row_name;
                pending_price = row_price;
                pending_change_pct = row_change_pct;
            } else if row_price.is_some() && row_change_pct.is_some() {
                // 价格+涨跌幅同行 → 可信的价格行（选中股价格可能单独一行）
                pending_price = row_price;
                pending_change_pct = row_change_pct;
            }
            // 忽略只有价格没有涨跌幅的独立数字行（避免图表噪声覆盖正确价格）
        }
    }

    quotes
}

/// 解析百分比文本: "+0.67%" → 0.67, "-1.23%" → -1.23
/// 兼容 OCR 将小数点误识别为逗号的情况 ("-2,33%" → -2.33)
fn ocr_parse_pct(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.ends_with('%') {
        let num = s.trim_end_matches('%').trim_start_matches('+').replace(',', ".");
        num.parse::<f64>().ok()
    } else {
        None
    }
}

/// 解析 "HK 商汤-W" / "SH 中国建筑" / "us 波音" / "HK世茂集团" / "牛牛圈 SH 中国建筑" 格式
/// 大小写不敏感（OCR 可能把 SH 识别为 sH, US 识别为 uS）
/// 兼容 US 的 OCR 噪声："US：", "US）", "US |" → 先清理再匹配
fn ocr_parse_market_name(s: &str) -> Option<(Market, String)> {
    let s = s.trim();
    let upper = s.to_uppercase();

    // 在文本中查找市场前缀
    let markets: &[(&str, Market)] = &[
        ("HK ", Market::HK),
        ("SH ", Market::SH),
        ("SZ ", Market::SZ),
        ("US ", Market::US),
    ];

    for (marker, market) in markets {
        // 开头匹配（可能有 OCR 噪声字符在前缀和名称之间）
        // "US：金龙中国" / "US）蔚来" → 清理前缀后的非字母数字/非中文字符
        if upper.starts_with(marker) {
            let rest = s[marker.len()..].trim();
            // 跳过前导标点噪声（OCR 产物：：、）、|、.、，等）
            let name = rest.trim_start_matches(|c: char| {
                !c.is_alphanumeric() && c <= '\x7f'
            }).trim().to_string();
            if !name.is_empty() && name.chars().next().map(|c| c > '\x7f').unwrap_or(false) {
                return Some((*market, name));
            }
        }
        // 内部匹配："牛牛圈 SH 中国建筑" → 找到 " SH " 后的中文名
        let search = &format!(" {}", marker); // " SH "
        if let Some(pos) = upper.find(search) {
            let name_start = pos + search.len();
            let name = s[name_start..].trim().to_string();
            if !name.is_empty() && name.chars().next().map(|c| c > '\x7f').unwrap_or(false) {
                return Some((*market, name));
            }
        }
    }

    // "US：" / "US）" 开头但无空格：OCR 将 "US 蔚来" 识别为 "US）蔚来"
    // 也处理 "HK名称"（无空格，但名称以中文开头）
    if let Some(prefix) = s.get(..2) {
        let market = match prefix.to_uppercase().as_str() {
            "HK" => Some(Market::HK),
            "SH" => Some(Market::SH),
            "SZ" => Some(Market::SZ),
            "US" => Some(Market::US),
            _ => None,
        };
        if let Some(m) = market {
            let rest = &s[2..];
            // 跳过前导标点噪声
            let name = rest.trim_start_matches(|c: char| {
                !c.is_alphanumeric() && c <= '\x7f'
            }).trim();
            if !name.is_empty() && name.starts_with(|c: char| c > '\x7f') {
                return Some((m, name.to_string()));
            }
        }
    }
    None
}

/// OCR 结果（含窗口尺寸和图像哈希，供调用方做 resize/去重检测）
pub struct OcrResult {
    pub quotes: Vec<QuoteSnapshot>,
    pub window_width: f64,
    pub window_height: f64,
    /// 截图 SHA1 哈希（hex），用于跳过未变化的帧
    pub image_hash: String,
    /// 是否因图像未变化而跳过了 OCR（复用上一轮结果）
    pub skipped: bool,
}

/// 计算 CGImage 的 SHA1 哈希（采样像素，避免全量读取大图）
fn compute_image_hash(image: &CGImage) -> String {
    use sha1::{Digest, Sha1};

    let w = CGImage::width(Some(image));
    let h = CGImage::height(Some(image));
    let bpr = CGImage::bytes_per_row(Some(image));

    // 获取像素数据
    let data_provider = CGImage::data_provider(Some(image));
    let cf_data = data_provider.and_then(|dp| {
        objc2_core_graphics::CGDataProviderCopyData(Some(&dp))
    });

    let mut hasher = Sha1::new();
    // 图像尺寸也参与哈希
    hasher.update(w.to_le_bytes());
    hasher.update(h.to_le_bytes());

    if let Some(ref data) = cf_data {
        let len = objc2_core_foundation::CFData::length(data) as usize;
        let ptr = objc2_core_foundation::CFData::byte_ptr(data);
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };

        // 每隔 N 行采样一行，加速哈希
        let step = (h / 32).max(1);
        for row in (0..h).step_by(step) {
            let start = row * bpr;
            let end = (start + bpr).min(len);
            if start < len {
                hasher.update(&bytes[start..end]);
            }
        }
    }

    format!("{:x}", hasher.finalize())
}

/// 完整的 OCR 数据管线：找窗口 → 截图 → 哈希比对 → OCR → 分行 → 解析
///
/// 如果 `prev_hash` 不为空且与当前截图哈希相同，跳过 OCR 返回空 quotes + skipped=true。
/// 调用方应在 skipped=true 时复用上一轮结果。
///
/// 如果提供了 `grid_frame`（来自 AX API 检测），直接按该区域裁剪，跳过 Pass 1 快速 OCR。
pub fn ocr_capture_and_parse(
    pid: i32,
    prev_hash: &str,
    grid_frame: Option<crate::futu::accessibility::GridFrame>,
) -> Result<OcrResult> {
    const MAX_RETRIES: u32 = 2;
    const RETRY_DELAY_MS: u64 = 200;

    let mut last_err = None;

    for attempt in 0..=MAX_RETRIES {
        // 每次重试都重新查找窗口（含尺寸，供 resize 检测）
        let win = match find_futu_window(pid) {
            Ok(w) => w,
            Err(e) => {
                last_err = Some(e);
                if attempt < MAX_RETRIES {
                    warn!("find_futu_window failed (attempt {}), retrying...", attempt + 1);
                    std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS));
                }
                continue;
            }
        };
        debug!("Using window ID: {} size={}x{} (attempt {})", win.id, win.width, win.height, attempt + 1);

        // 截图
        let image = match capture_window(win.id) {
            Ok(img) => img,
            Err(e) => {
                last_err = Some(e);
                if attempt < MAX_RETRIES {
                    warn!("capture_window failed (attempt {}), retrying...", attempt + 1);
                    std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS));
                }
                continue;
            }
        };

        // 计算图像哈希，与上一轮比对
        let hash = compute_image_hash(&image);
        if !prev_hash.is_empty() && hash == prev_hash {
            debug!("Image unchanged (hash={}), skipping OCR", &hash[..8]);
            return Ok(OcrResult {
                quotes: Vec::new(),
                window_width: win.width,
                window_height: win.height,
                image_hash: hash,
                skipped: true,
            });
        }

        // 有 AX GridFrame → 跳过 Pass 1，直接按 grid frame 裁剪
        // 无 GridFrame → 降级到 Pass 1 快速 OCR 检测布局
        let watchlist_crop = if let Some(gf) = grid_frame {
            debug!(
                "Using AX grid frame: ({:.3},{:.3},{:.3},{:.3}), skipping Pass 1",
                gf.x, gf.y, gf.width, gf.height
            );
            let x_range = (gf.x, (gf.x + gf.width).min(1.0));
            let y_range = Some((gf.y, (gf.y + gf.height).min(1.0)));
            crop_image_xy(&image, x_range, y_range)?
        } else {
            // Pass 1: 快速 OCR 全图 → 检测布局
            let fast_blocks = recognize_text_fast(&image)?;
            if fast_blocks.is_empty() {
                return Ok(OcrResult {
                    quotes: Vec::new(),
                    window_width: win.width,
                    window_height: win.height,
                    image_hash: hash,
                    skipped: false,
                });
            }
            let layout = detect_layout(&fast_blocks);
            debug!("Fast OCR: {} blocks, layout: {:?}", fast_blocks.len(), layout);
            crop_image(&image, layout.watchlist_x)?
        };
        let blocks = recognize_text(&watchlist_crop)?;
        debug!("Watchlist crop OCR: {} blocks", blocks.len());

        // 分行 + 两行配对解析
        let rows = group_into_rows(&blocks);
        let quotes = parse_watchlist_from_ocr(&rows);
        info!("OCR parsed {} quotes from {} rows", quotes.len(), rows.len());
        return Ok(OcrResult {
            quotes,
            window_width: win.width,
            window_height: win.height,
            image_hash: hash,
            skipped: false,
        });
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("OCR capture failed after {} retries", MAX_RETRIES + 1)))
}

// ---- 内部辅助函数 ----

/// 从 CFDictionary 中读取字符串值
unsafe fn dict_get_string(dict: *const std::ffi::c_void, key: &str) -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    let cf_key = CFString::new(key);
    let mut value: *const std::ffi::c_void = std::ptr::null();
    let found = core_foundation::dictionary::CFDictionaryGetValueIfPresent(
        dict as core_foundation::dictionary::CFDictionaryRef,
        cf_key.as_concrete_TypeRef() as *const _,
        &mut value,
    );
    if found == 0 || value.is_null() {
        return None;
    }

    // CFString → Rust String
    let cf_str: CFString =
        core_foundation::base::TCFType::wrap_under_get_rule(value as core_foundation::string::CFStringRef);
    Some(cf_str.to_string())
}

/// 从 CFDictionary 中读取 i32 值
unsafe fn dict_get_i32(dict: *const std::ffi::c_void, key: &str) -> Option<i32> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    let cf_key = CFString::new(key);
    let mut value: *const std::ffi::c_void = std::ptr::null();
    let found = core_foundation::dictionary::CFDictionaryGetValueIfPresent(
        dict as core_foundation::dictionary::CFDictionaryRef,
        cf_key.as_concrete_TypeRef() as *const _,
        &mut value,
    );
    if found == 0 || value.is_null() {
        return None;
    }

    // CFNumber → i32
    let mut result: i32 = 0;
    let ok = core_foundation::number::CFNumberGetValue(
        value as core_foundation::number::CFNumberRef,
        core_foundation::number::kCFNumberSInt32Type,
        &mut result as *mut i32 as *mut std::ffi::c_void,
    );
    if ok {
        Some(result)
    } else {
        None
    }
}

/// 从 CFDictionary 中读取窗口 bounds (width, height)
unsafe fn dict_get_window_bounds(dict: *const std::ffi::c_void) -> Option<(f64, f64)> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    let cf_key = CFString::new("kCGWindowBounds");
    let mut value: *const std::ffi::c_void = std::ptr::null();
    let found = core_foundation::dictionary::CFDictionaryGetValueIfPresent(
        dict as core_foundation::dictionary::CFDictionaryRef,
        cf_key.as_concrete_TypeRef() as *const _,
        &mut value,
    );
    if found == 0 || value.is_null() {
        return None;
    }

    // kCGWindowBounds 是一个 CFDictionary，含 Width/Height
    let w_key = CFString::new("Width");
    let h_key = CFString::new("Height");

    let mut w_val: *const std::ffi::c_void = std::ptr::null();
    let mut h_val: *const std::ffi::c_void = std::ptr::null();

    core_foundation::dictionary::CFDictionaryGetValueIfPresent(
        value as core_foundation::dictionary::CFDictionaryRef,
        w_key.as_concrete_TypeRef() as *const _,
        &mut w_val,
    );
    core_foundation::dictionary::CFDictionaryGetValueIfPresent(
        value as core_foundation::dictionary::CFDictionaryRef,
        h_key.as_concrete_TypeRef() as *const _,
        &mut h_val,
    );

    let mut w: f64 = 0.0;
    let mut h: f64 = 0.0;

    if !w_val.is_null() {
        core_foundation::number::CFNumberGetValue(
            w_val as core_foundation::number::CFNumberRef,
            core_foundation::number::kCFNumberFloat64Type,
            &mut w as *mut f64 as *mut std::ffi::c_void,
        );
    }
    if !h_val.is_null() {
        core_foundation::number::CFNumberGetValue(
            h_val as core_foundation::number::CFNumberRef,
            core_foundation::number::kCFNumberFloat64Type,
            &mut h as *mut f64 as *mut std::ffi::c_void,
        );
    }

    Some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_into_rows_empty() {
        let blocks: Vec<OcrTextBlock> = vec![];
        let rows = group_into_rows(&blocks);
        assert!(rows.is_empty());
    }

    #[test]
    fn test_group_into_rows_basic() {
        let blocks = vec![
            OcrTextBlock {
                text: "00700".to_string(),
                confidence: 0.95,
                bbox: (0.1, 0.8, 0.1, 0.02), // 行 1（y=0.82 顶边）
            },
            OcrTextBlock {
                text: "388.00".to_string(),
                confidence: 0.90,
                bbox: (0.3, 0.8, 0.1, 0.02), // 行 1
            },
            OcrTextBlock {
                text: "09988".to_string(),
                confidence: 0.93,
                bbox: (0.1, 0.7, 0.1, 0.02), // 行 2（y=0.72 顶边）
            },
            OcrTextBlock {
                text: "100.50".to_string(),
                confidence: 0.88,
                bbox: (0.3, 0.7, 0.1, 0.02), // 行 2
            },
        ];

        let rows = group_into_rows(&blocks);
        assert_eq!(rows.len(), 2);
        // 第一行（y=0.82）应该在上面
        assert_eq!(rows[0].len(), 2);
        assert_eq!(rows[0][0].text, "00700");
        assert_eq!(rows[0][1].text, "388.00");
        // 第二行（y=0.72）
        assert_eq!(rows[1].len(), 2);
        assert_eq!(rows[1][0].text, "09988");
        assert_eq!(rows[1][1].text, "100.50");
    }

    #[test]
    fn test_group_into_rows_within_tolerance() {
        // 同一行的块有微小 y 差异（< 0.5%）
        let blocks = vec![
            OcrTextBlock {
                text: "A".to_string(),
                confidence: 0.9,
                bbox: (0.1, 0.500, 0.1, 0.02), // y top = 0.52
            },
            OcrTextBlock {
                text: "B".to_string(),
                confidence: 0.9,
                bbox: (0.3, 0.502, 0.1, 0.02), // y top = 0.522（差 0.002 < 0.005）
            },
        ];

        let rows = group_into_rows(&blocks);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 2);
    }

    #[test]
    fn test_parse_watchlist_two_line_format() {
        // 两行配对格式：第一行 "HK 腾讯控股 | 388.00 | +0.67%", 第二行 "00700"
        let blocks = vec![
            // 行 1 (y=0.8)
            OcrTextBlock {
                text: "HK 腾讯控股".to_string(),
                confidence: 0.95,
                bbox: (0.0, 0.80, 0.2, 0.02),
            },
            OcrTextBlock {
                text: "388.00".to_string(),
                confidence: 0.92,
                bbox: (0.4, 0.80, 0.1, 0.02),
            },
            OcrTextBlock {
                text: "+0.67%".to_string(),
                confidence: 0.91,
                bbox: (0.6, 0.80, 0.1, 0.02),
            },
            // 行 2 (y=0.77)
            OcrTextBlock {
                text: "00700".to_string(),
                confidence: 0.98,
                bbox: (0.0, 0.77, 0.1, 0.02),
            },
        ];

        let rows = group_into_rows(&blocks);
        let quotes = parse_watchlist_from_ocr(&rows);
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].code.code, "00700");
        assert_eq!(quotes[0].code.market, Market::HK);
        assert_eq!(quotes[0].name, "腾讯控股");
        assert_eq!(quotes[0].last_price, 388.00);
        assert_eq!(quotes[0].change_pct, 0.67);
        assert_eq!(quotes[0].source, DataSource::Ocr);
    }

    #[test]
    fn test_parse_watchlist_standalone_market_prefix() {
        // OCR 将市场前缀和名称拆为独立块："HK" | "融创中国" | "1.220" | "-0.81%"
        let blocks = vec![
            // 行 1 (y=0.8)
            OcrTextBlock {
                text: "HK".to_string(),
                confidence: 0.9,
                bbox: (0.0, 0.80, 0.05, 0.02),
            },
            OcrTextBlock {
                text: "融创中国".to_string(),
                confidence: 0.95,
                bbox: (0.1, 0.80, 0.15, 0.02),
            },
            OcrTextBlock {
                text: "1.220".to_string(),
                confidence: 0.92,
                bbox: (0.4, 0.80, 0.1, 0.02),
            },
            OcrTextBlock {
                text: "-0.81%".to_string(),
                confidence: 0.91,
                bbox: (0.6, 0.80, 0.1, 0.02),
            },
            // 行 2 (y=0.77)
            OcrTextBlock {
                text: "01918".to_string(),
                confidence: 0.98,
                bbox: (0.0, 0.77, 0.1, 0.02),
            },
        ];

        let rows = group_into_rows(&blocks);
        let quotes = parse_watchlist_from_ocr(&rows);
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].code.code, "01918");
        assert_eq!(quotes[0].name, "融创中国");
        assert_eq!(quotes[0].change_pct, -0.81);
    }
}
