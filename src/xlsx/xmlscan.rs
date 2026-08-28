/// 面向机器生成 XML 的字节级轻量扫描器。
/// 只处理本系统写回的两张工作表，输入恒为 Excel/openpyxl 产物的规范输出，
/// 因此用精确的字节扫描替代通用 XML 解析器，保证未触及部分逐字节保留。

pub struct TagInfo {
    pub name: String,
    pub closing: bool,
    pub self_closing: bool,
    /// 属性区字节范围（'<' 与 '>' 之间，含标签名）
    pub inner: (usize, usize),
    /// '>' 之后的位置
    pub after: usize,
}

/// 从 pos 开始找下一个标签，返回 (标签'<'位置, 信息)
pub fn next_tag(b: &[u8], pos: usize) -> Option<(usize, TagInfo)> {
    let mut i = pos;
    loop {
        let lt = find_byte(b, i, b"<")?;
        // 注释 / CDATA 跳过（机器生成的 sheet XML 不会出现，防御性处理）
        if b[lt..].starts_with(b"<!--") {
            i = find_byte(b, lt + 4, b"-->")? + 3;
            continue;
        }
        if b[lt..].starts_with(b"<![CDATA[") {
            i = find_byte(b, lt + 9, b"]]>")? + 3;
            continue;
        }
        let gt = find_gt(b, lt)?;
        let inner_end = if gt > 0 && b[gt - 1] == b'/' { gt - 1 } else { gt };
        let body = &b[lt + 1..inner_end];
        let (closing, name_start) = if body.first() == Some(&b'/') {
            (true, 1)
        } else {
            (false, 0)
        };
        let mut name_end = name_start;
        while name_end < body.len() {
            let c = body[name_end];
            if c == b' ' || c == b'\t' || c == b'\r' || c == b'\n' {
                break;
            }
            name_end += 1;
        }
        let name = String::from_utf8_lossy(&body[name_start..name_end]).into_owned();
        let self_closing = gt > 0 && b[gt - 1] == b'/';
        return Some((
            lt,
            TagInfo {
                name,
                closing,
                self_closing,
                inner: (lt + 1, inner_end),
                after: gt + 1,
            },
        ));
    }
}

/// 找 '>'，跳过属性值内引号中的内容
fn find_gt(b: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b'"' || c == b'\'' {
                    quote = Some(c);
                } else if c == b'>' {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

pub fn find_byte(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= b.len() {
        return None;
    }
    b[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// 从标签属性区提取属性值（已做实体反转义）
pub fn attr_value(inner: &[u8], name: &str) -> Option<String> {
    let needle = format!("{}=", name);
    let mut search_from = 0;
    loop {
        let p = find_byte(inner, search_from, needle.as_bytes())?;
        // 前一个字符必须是空白（防止 r: id 之类误配）
        let ok_prev = p == 0 || matches!(inner[p - 1], b' ' | b'\t' | b'\r' | b'\n');
        if !ok_prev {
            search_from = p + needle.len();
            continue;
        }
        let mut q = p + needle.len();
        if q >= inner.len() {
            return None;
        }
        let quote = inner[q];
        if quote != b'"' && quote != b'\'' {
            search_from = q;
            continue;
        }
        q += 1;
        let end = find_byte(inner, q, &[quote])?;
        let raw = String::from_utf8_lossy(&inner[q..end]).into_owned();
        return Some(unescape(&raw));
    }
}

pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#xA;", "\n")
        .replace("&#xa;", "\n")
        .replace("&amp;", "&")
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// 元素的完整字节范围（开标签'<' 到 闭标签'>' 之后）；自闭合则到 '>' 之后
pub fn element_span(b: &[u8], open_lt: usize, info: &TagInfo) -> Option<(usize, usize)> {
    if info.self_closing {
        return Some((open_lt, info.after));
    }
    let close = format!("</{}>", info.name);
    let p = find_byte(b, info.after, close.as_bytes())?;
    // 简单防同名片段（sheet XML 中 c/row/f/v 均不嵌套同名）
    Some((open_lt, p + close.len()))
}

/// sheetData 内容区间 (content_start, content_end, self_closing)
pub fn sheet_data_span(b: &[u8]) -> Option<(usize, usize, bool)> {
    let (lt, _info) = next_tag(b, 0)?;
    let mut pos = lt;
    loop {
        let (lt2, inf) = next_tag(b, pos)?;
        if inf.name == "sheetData" && !inf.closing {
            let span = element_span(b, lt2, &inf)?;
            if inf.self_closing {
                return Some((span.0, span.0, true));
            }
            return Some((inf.after, span.1 - format!("</{}>", inf.name).len(), false));
        }
        pos = lt2 + 1;
        if pos >= b.len() {
            return None;
        }
    }
}

/// 迭代某内容区间内指定名称的元素，回调 (lt, span)
pub fn for_each_element<F: FnMut(usize, TagInfo, (usize, usize)) -> bool>(
    b: &[u8],
    start: usize,
    end: usize,
    name: &str,
    mut f: F,
) {
    let mut pos = start;
    while pos < end {
        let (lt, info) = match next_tag(b, pos) {
            Some(x) => x,
            None => break,
        };
        if lt >= end {
            break;
        }
        if info.name == name && !info.closing {
            let span = match element_span(b, lt, &info) {
                Some(s) => s,
                None => break,
            };
            let stop = f(lt, info, span);
            if stop {
                return;
            }
            pos = span.1;
        } else {
            pos = lt + 1;
        }
    }
}

/// 解析单元格引用 "AB12" -> (列号, 行号)
pub fn parse_ref(r: &str) -> Option<(u32, u32)> {
    let split = r.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = r.split_at(split);
    let col = crate::model::col_to_num(letters)?;
    let row: u32 = digits.parse().ok()?;
    Some((col, row))
}

/// 工作表名 -> xml 部件路径
pub fn sheet_paths(entries: &[crate::xlsx::zipio::Entry]) -> Result<std::collections::HashMap<String, String>, String> {
    use crate::xlsx::zipio::Entry;
    let get = |n: &str| entries.iter().find(|e: &&Entry| e.name == n).map(|e| e.data.clone());
    let wb = get("xl/workbook.xml").ok_or("BadZipFile: missing xl/workbook.xml")?;
    let rels = get("xl/_rels/workbook.xml.rels").ok_or("BadZipFile: missing workbook rels")?;
    // rels: rId -> Target
    let mut rid_map = std::collections::HashMap::new();
    for_each_element(&rels, 0, rels.len(), "Relationship", |_lt, info, _span| {
        let inner = &rels[info.inner.0..info.inner.1];
        if let (Some(id), Some(target)) = (attr_value(inner, "Id"), attr_value(inner, "Target")) {
            rid_map.insert(id, target);
        }
        false
    });
    let mut out = std::collections::HashMap::new();
    for_each_element(&wb, 0, wb.len(), "sheet", |_lt, info, _span| {
        let inner = &wb[info.inner.0..info.inner.1];
        if let (Some(name), Some(rid)) = (attr_value(inner, "name"), attr_value(inner, "r:id")) {
            if let Some(t) = rid_map.get(&rid) {
                let t = t.trim_start_matches('/');
                let path = if t.starts_with("xl/") {
                    t.to_string()
                } else {
                    format!("xl/{}", t)
                };
                out.insert(name, path);
            }
        }
        false
    });
    Ok(out)
}

pub fn find_entry<'a>(entries: &'a [crate::xlsx::zipio::Entry], name: &str) -> Option<&'a crate::xlsx::zipio::Entry> {
    entries.iter().find(|e| e.name == name)
}

/// 模板单元格：第 N 行各列的样式索引与公式
#[derive(Clone, Debug, Default)]
pub struct CellTpl {
    pub style: Option<String>,
    pub formula: Option<String>,
}

pub fn row_templates(sheet_xml: &[u8], row_num: u32) -> std::collections::BTreeMap<u32, CellTpl> {
    let mut out = std::collections::BTreeMap::new();
    let (sd_s, sd_e, _sc) = match sheet_data_span(sheet_xml) {
        Some(x) => x,
        None => return out,
    };
    for_each_element(sheet_xml, sd_s, sd_e, "row", |_lt, info, span| {
        let inner_attr = &sheet_xml[info.inner.0..info.inner.1];
        let r = attr_value(inner_attr, "r").and_then(|x| x.parse::<u32>().ok());
        if r != Some(row_num) {
            return false;
        }
        // 行内容 = 行开标签后 到 </row> 前
        let content_start = info.after;
        let content_end = span.1 - "</row>".len();
        for_each_element(sheet_xml, content_start, content_end, "c", |_clt, cinfo, cspan| {
            let cattr = &sheet_xml[cinfo.inner.0..cinfo.inner.1];
            if let Some(rr) = attr_value(cattr, "r").and_then(|x| parse_ref(&x)) {
                let style = attr_value(cattr, "s");
                let formula = if let Some(flt) = find_byte(&sheet_xml[cspan.0..cspan.1], 0, b"<f") {
                    let (flt_abs, finfo) = match next_tag(&sheet_xml[cspan.0..cspan.1], flt) {
                        Some(x) => x,
                        None => (0, TagInfo { name: String::new(), closing: false, self_closing: true, inner: (0, 0), after: 0 }),
                    };
                    if !finfo.self_closing && finfo.name == "f" {
                        let close = "</f>";
                        let ftxt_start = finfo.after;
                        let ftxt_end = find_byte(&sheet_xml[cspan.0..cspan.1], ftxt_start, close.as_bytes())
                            .unwrap_or(cspan.1 - cspan.0);
                        Some(unescape(&String::from_utf8_lossy(
                            &sheet_xml[cspan.0 + ftxt_start..cspan.0 + ftxt_end],
                        )))
                    } else {
                        let _ = flt_abs;
                        None
                    }
                } else {
                    None
                };
                out.insert(
                    rr.0,
                    CellTpl { style, formula },
                );
            }
            false
        });
        true // 找到目标行即停
    });
    out
}

/// 更新 <dimension ref="..."> 的最大行号（插入新行后）
pub fn fix_dimension(sheet_xml: &mut Vec<u8>, max_row: u32) {
    let (_lt, info) = match next_tag(sheet_xml, 0) {
        Some(x) => x,
        None => return,
    };
    if info.name != "dimension" || info.self_closing {
        return;
    }
    let inner = sheet_xml[info.inner.0..info.inner.1].to_vec();
    let r = match attr_value(&inner, "ref") {
        Some(x) => x,
        None => return,
    };
    let parts: Vec<&str> = r.split(':').collect();
    if parts.len() != 2 {
        return;
    }
    let (letters, row) = match parse_ref(parts[1]) {
        Some(x) => x,
        None => return,
    };
    if row >= max_row {
        return;
    }
    let new_ref = format!("{}:{}{}", parts[0], letters, max_row.max(row));
    let mut new_tag: Vec<u8> = sheet_xml[info.inner.0..info.inner.1].to_vec();
    let old_val = format!("ref=\"{}\"", r);
    if let Some(p) = find_byte(&new_tag, 0, old_val.as_bytes()) {
        let nv = format!("ref=\"{}\"", new_ref);
        new_tag.splice(p..p + old_val.len(), nv.into_bytes());
        let s = info.inner.0;
        let e = info.inner.1;
        sheet_xml.splice(s..e, new_tag);
    }
}
