use crate::model::{RawClass, RawMember, RawVal};
use calamine::{Data, Reader, Xlsx};
use chrono::NaiveDate;
use std::io::Cursor;

fn data_to_raw(d: &Data) -> RawVal {
    match d {
        Data::Empty => RawVal::Empty,
        Data::String(s) => RawVal::Text(s.clone()),
        Data::Int(i) => RawVal::Num(*i as f64),
        Data::Float(f) => RawVal::Num(*f),
        Data::Bool(b) => RawVal::Text(b.to_string()),
        Data::Error(_) => RawVal::Empty,
        Data::DateTime(dt) => {
            let f = dt.as_f64();
            if !dt.is_duration() && f.abs() < 1.0 {
                RawVal::Time(f)
            } else if let Some(ndt) = dt.as_datetime() {
                RawVal::Date(ndt.date())
            } else {
                RawVal::Num(f)
            }
        }
        Data::DateTimeIso(s) => match NaiveDate::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
            Ok(x) => RawVal::Date(x),
            Err(_) => RawVal::Text(s.clone()),
        },
        Data::DurationIso(s) => RawVal::Text(s.clone()),
    }
}

fn s(d: Option<&Data>) -> String {
    match d {
        None => String::new(),
        Some(x) => data_to_raw(x).to_display().unwrap_or_default(),
    }
}

fn raw(d: Option<&Data>) -> RawVal {
    match d {
        None => RawVal::Empty,
        Some(x) => data_to_raw(x),
    }
}

fn num(d: Option<&Data>) -> Option<f64> {
    raw(d).num()
}

/// 从 xlsx 字节读取两张业务表的原始行（等价 Python _reload_raw）
/// 返回 (members, classes, 退卡表头 V1 是否已存在)
pub fn read_raw(bytes: &[u8]) -> Result<(Vec<RawMember>, Vec<RawClass>, bool), String> {
    let cursor = Cursor::new(bytes.to_vec());
    let mut wb: Xlsx<Cursor<Vec<u8>>> =
        Xlsx::new(cursor).map_err(|e| format!("BadZipFile: {}", e))?;

    let arch = wb
        .worksheet_range(crate::SHEET_ARCHIVE)
        .map_err(|_| format!("该表格缺少工作表：{}", crate::SHEET_ARCHIVE))?;
    let recs = wb
        .worksheet_range(crate::SHEET_RECORDS)
        .map_err(|_| format!("该表格缺少工作表：{}", crate::SHEET_RECORDS))?;

    let mut members = Vec::new();
    for r in 1..crate::ARCHIVE_MAX_ROW {
        // calamine Range 以 (行, 列) 0 基访问
        let get = |c: u32| arch.get_value((r, c - 1));
        let id = s(get(1));
        if id.trim().is_empty() {
            continue;
        }
        let row = r + 1; // 工作表实际行号
        members.push(RawMember {
            row,
            id: id.trim().to_string(),
            name: s(get(2)),
            phone: s(get(3)),
            teacher: s(get(4)),
            member_type: s(get(5)),
            card_type: s(get(6)),
            purchase: raw(get(7)),
            activation: raw(get(8)),
            expiry: raw(get(9)),
            amount: num(get(10)),
            receivable: num(get(11)),
            total: num(get(13)),
            refund: s(get(22)) == "是",
            remark: s(get(23)),
        });
    }
    let refund_header_present = !s(arch.get_value((0, 21))).trim().is_empty();

    let mut classes = Vec::new();
    for r in 1..crate::RECORDS_MAX_ROW {
        let get = |c: u32| recs.get_value((r, c - 1));
        let mid = s(get(2));
        if mid.trim().is_empty() {
            continue;
        }
        let row = r + 1;
        let seq = match raw(get(1)) {
            RawVal::Empty => serde_json::Value::Null,
            RawVal::Num(f) if f.fract() == 0.0 => serde_json::json!(f as i64),
            RawVal::Num(f) => serde_json::json!(f),
            RawVal::Text(t) => serde_json::Value::String(t),
            RawVal::Date(d) => serde_json::Value::String(d.format("%Y-%m-%d").to_string()),
            RawVal::Time(f) => serde_json::Value::String(crate::model::fmt_hm(f)),
        };
        classes.push(RawClass {
            row,
            seq,
            member_id: mid.trim().to_string(),
            date: raw(get(4)),
            time: raw(get(5)),
            course: s(get(12)),
            effect: raw(get(13)).is_one(),
            remark: s(get(14)),
        });
    }
    Ok((members, classes, refund_header_present))
}
