use crate::model::{fmt_num, num_to_col};
use crate::xlsx::xmlscan::{
    attr_value, element_span, escape, find_byte, fix_dimension, next_tag, parse_ref, sheet_data_span,
    TagInfo,
};
use std::collections::BTreeMap;

/// 写入值（含公式；日期由调用方转换为串行数值并带模板样式）
#[derive(Clone, Debug)]
pub enum CellVal {
    Num(f64),
    Text(String),
    Formula(String),
}

#[derive(Clone, Debug)]
pub struct CellWrite {
    pub col: u32,
    pub val: CellVal,
    pub style: Option<String>,
}

/// 对一个行的写入指令
#[derive(Clone, Debug)]
pub struct RowWrite {
    pub row: u32,
    pub cells: Vec<CellWrite>,
    pub clear_cols: Vec<u32>,
    /// (列, 已按目标行重编号的公式, 样式)——仅当该列现有单元格无公式时写入
    pub ensure_formulas: Vec<(u32, String, Option<String>)>,
    /// 行不存在于 XML 时新建
    pub insert_if_absent: bool,
}

impl RowWrite {
    pub fn new(row: u32) -> Self {
        RowWrite { row, cells: Vec::new(), clear_cols: Vec::new(), ensure_formulas: Vec::new(), insert_if_absent: false }
    }
}

enum Action<'a> {
    Keep((usize, usize)),
    Write(&'a CellWrite),
    Ensure(&'a (u32, String, Option<String>)),
    Remove,
}

fn emit_cell(out: &mut Vec<u8>, col: u32, row: u32, val: &CellVal, style: Option<&str>) {
    let r = format!("{}{}", num_to_col(col), row);
    out.extend_from_slice(b"<c r=\"");
    out.extend_from_slice(r.as_bytes());
    out.push(b'"');
    if let Some(s) = style {
        out.extend_from_slice(b" s=\"");
        out.extend_from_slice(s.as_bytes());
        out.push(b'"');
    }
    match val {
        CellVal::Num(f) => {
            out.extend_from_slice(b"><v>");
            out.extend_from_slice(fmt_num(*f).as_bytes());
            out.extend_from_slice(b"</v></c>");
        }
        CellVal::Text(t) => {
            out.extend_from_slice(b" t=\"inlineStr\"><is><t xml:space=\"preserve\">");
            out.extend_from_slice(escape(t).as_bytes());
            out.extend_from_slice(b"</t></is></c>");
        }
        CellVal::Formula(f) => {
            out.extend_from_slice(b"><f>");
            out.extend_from_slice(escape(f).as_bytes());
            out.extend_from_slice(b"</f></c>");
        }
    }
}

/// 行标签属性区原样保留（含 r/spans/s 等属性）
fn row_start_bytes(orig: &[u8], info: &TagInfo) -> Vec<u8> {
    let mut out = vec![b'<'];
    out.extend_from_slice(&orig[info.inner.0..info.inner.1]);
    out.push(b'>');
    out
}

/// 重建目标行：保留未触及单元格，替换/删除/补公式指定列
fn rebuild_row(orig: &[u8], _lt: usize, info: &TagInfo, span: (usize, usize), rw: &RowWrite) -> Vec<u8> {
    let content_start = if info.self_closing { info.after } else { info.after };
    let content_end = if info.self_closing { info.after } else { span.1 - "</row>".len() };

    // 解析现有单元格
    let mut cells: BTreeMap<u32, Action> = BTreeMap::new();
    let mut pos = content_start;
    while pos < content_end {
        if orig[pos] != b'<' {
            pos += 1;
            continue;
        }
        let (clt, cinfo) = match next_tag(orig, pos) {
            Some(x) => x,
            None => break,
        };
        if cinfo.name != "c" || cinfo.closing {
            pos = clt + 1;
            continue;
        }
        let cspan = match element_span(orig, clt, &cinfo) {
            Some(x) => x,
            None => break,
        };
        let cattr = &orig[cinfo.inner.0..cinfo.inner.1];
        if let Some(rr) = attr_value(cattr, "r").and_then(|x| parse_ref(&x)) {
            cells.insert(rr.0, Action::Keep((cspan.0, cspan.1)));
        }
        pos = cspan.1;
    }
    for c in &rw.clear_cols {
        cells.insert(*c, Action::Remove);
    }
    for cw in &rw.cells {
        cells.insert(cw.col, Action::Write(cw));
    }
    for ef in &rw.ensure_formulas {
        match cells.get(&ef.0) {
            Some(Action::Keep((s, e))) => {
                // 已有公式则保留
                if find_byte(&orig[*s..*e], 0, b"<f").is_some() {
                    // keep
                } else {
                    cells.insert(ef.0, Action::Ensure(ef));
                }
            }
            _ => {
                cells.insert(ef.0, Action::Ensure(ef));
            }
        }
    }

    let mut out = row_start_bytes(orig, info);
    for (col, action) in &cells {
        match action {
            Action::Keep((s, e)) => out.extend_from_slice(&orig[*s..*e]),
            Action::Write(cw) => emit_cell(&mut out, *col, rw.row, &cw.val, cw.style.as_deref()),
            Action::Ensure(ef) => emit_cell(
                &mut out,
                *col,
                rw.row,
                &CellVal::Formula(ef.1.clone()),
                ef.2.as_deref(),
            ),
            Action::Remove => {}
        }
    }
    out.extend_from_slice(b"</row>");
    out
}

/// 新建行（模板样式由调用方通过 ensure_formulas/cells 带入）
fn build_new_row(rw: &RowWrite) -> Vec<u8> {
    // 值写入优先；补公式列仅当该列没有值写入时生效
    let mut cells: BTreeMap<u32, (CellVal, Option<String>)> = BTreeMap::new();
    for cw in &rw.cells {
        cells.insert(cw.col, (cw.val.clone(), cw.style.clone()));
    }
    for (col, f, st) in &rw.ensure_formulas {
        cells
            .entry(*col)
            .or_insert_with(|| (CellVal::Formula(f.clone()), st.clone()));
    }
    let mut out = format!("<row r=\"{}\">", rw.row).into_bytes();
    for (col, (val, style)) in &cells {
        emit_cell(&mut out, *col, rw.row, val, style.as_deref());
    }
    out.extend_from_slice(b"</row>");
    out
}

fn is_row_tag(b: &[u8], lt: usize) -> bool {
    let rest = &b[lt + 1..];
    let name = b"row";
    rest.starts_with(name)
        && matches!(rest.get(name.len()), Some(b' ') | Some(b'>') | Some(b'/') | Some(b'\t') | Some(b'\r') | Some(b'\n'))
}

/// 对工作表 XML 应用行级补丁（其余字节原样保留）
pub fn apply_patch(sheet_xml: &[u8], writes: &[RowWrite]) -> Result<Vec<u8>, String> {
    let (sd_s, sd_e, self_closing) = sheet_data_span(sheet_xml)
        .ok_or_else(|| "BadZipFile: sheetData not found".to_string())?;

    let mut targets: BTreeMap<u32, &RowWrite> = BTreeMap::new();
    for w in writes {
        targets.insert(w.row, w);
    }

    let mut out: Vec<u8> = sheet_xml[..sd_s].to_vec();

    if self_closing {
        // 空表：直接构造
        out.truncate(sd_s.saturating_sub("<sheetData/>".len()));
        out.extend_from_slice(b"<sheetData>");
        for w in writes {
            if w.insert_if_absent {
                out.extend_from_slice(&build_new_row(w));
            }
        }
        out.extend_from_slice(b"</sheetData>");
        let max_row = writes.iter().map(|w| w.row).max().unwrap_or(1);
        fix_dimension(&mut out, max_row);
        return Ok(out);
    }

    let mut pos = sd_s;
    let mut inserts: Vec<&RowWrite> = writes.iter().filter(|w| w.insert_if_absent).collect();
    inserts.sort_by_key(|w| w.row);

    while pos < sd_e {
        if !is_row_tag(sheet_xml, pos) {
            out.push(sheet_xml[pos]);
            pos += 1;
            continue;
        }
        let (lt, info) = match next_tag(sheet_xml, pos) {
            Some(x) => x,
            None => break,
        };
        let span = match element_span(sheet_xml, lt, &info) {
            Some(x) => x,
            None => break,
        };
        let inner = &sheet_xml[info.inner.0..info.inner.1];
        let rnum = attr_value(inner, "r").and_then(|x| x.parse::<u32>().ok());

        // 先落插队的新行
        loop {
            let flush = match inserts.first() {
                Some(w) => rnum.map(|r| w.row < r).unwrap_or(false),
                None => false,
            };
            if flush {
                let w = inserts.remove(0);
                out.extend_from_slice(&build_new_row(w));
            } else {
                break;
            }
        }

        match rnum.and_then(|r| targets.get(&r)) {
            Some(rw) => {
                out.extend_from_slice(&rebuild_row(sheet_xml, lt, &info, span, rw));
            }
            None => {
                out.extend_from_slice(&sheet_xml[span.0..span.1]);
            }
        }
        pos = span.1;
    }
    // 剩余插入行（大于所有现有行）
    for w in inserts {
        out.extend_from_slice(&build_new_row(w));
    }

    out.extend_from_slice(&sheet_xml[sd_e..]);
    let max_row = writes.iter().map(|w| w.row).max().unwrap_or(1);
    fix_dimension(&mut out, max_row);
    Ok(out)
}

/// 公式中模板行相对引用重编号（等价 Python _ensure_formulas 的正则替换）：
/// 仅替换形如 K2 的相对引用；$K$2、AB20（K2 前有字母）不受影响
pub fn renumber_formula(f: &str, from_row: u32, to_row: u32) -> String {
    let chars: Vec<char> = f.chars().collect();
    let from_s = from_row.to_string();
    let to_s = to_row.to_string();
    let n = chars.len();
    let mut out = String::with_capacity(n + 8);
    let mut i = 0;
    while i < n {
        let prev_ok = i == 0
            || !matches!(chars[i - 1], '0'..='9' | 'A'..='Z' | '$' | '.' | 'a'..='z');
        let mut matched = false;
        if prev_ok && chars[i].is_ascii_uppercase() {
            for take in (1..=2).rev() {
                if i + take > n {
                    continue;
                }
                let letters: Vec<char> = chars[i..i + take].to_vec();
                if !letters.iter().all(|c| c.is_ascii_uppercase()) {
                    continue;
                }
                let j = i + take;
                if j + from_s.len() <= n && chars[j..j + from_s.len()].iter().collect::<String>() == from_s {
                    let k = j + from_s.len();
                    let after_ok = k >= n
                        || !matches!(chars[k], '0'..='9' | 'A'..='Z' | 'a'..='z' | '_');
                    if after_ok {
                        out.extend(letters.iter());
                        out.push_str(&to_s);
                        i = k;
                        matched = true;
                        break;
                    }
                }
            }
        }
        if !matched {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_renumber() {
        assert_eq!(
            renumber_formula("=K2-J2", 2, 6),
            "=K6-J6"
        );
        assert_eq!(
            renumber_formula("=IF(F2=\"次卡\",MAX(0,M2-N2),\"—\")", 2, 504),
            "=IF(F504=\"次卡\",MAX(0,M504-N504),\"—\")"
        );
        // 绝对引用不动
        assert_eq!(renumber_formula("=SUM($B$2:$B$9)", 2, 6), "=SUM($B$2:$B$9)");
        // 长标识符不误伤（AB20 不应把 A20 部分匹配）
        assert_eq!(renumber_formula("=AB2+1", 2, 9), "=AB9+1");
        assert_eq!(renumber_formula("=TODAY()-S2", 2, 7), "=TODAY()-S7");
        // A2A 这种后接字母的不动
        assert_eq!(renumber_formula("=A2AB", 2, 6), "=A2AB");
    }
}
