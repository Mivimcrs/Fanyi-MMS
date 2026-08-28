use chrono::{NaiveDate, NaiveDateTime, Timelike};
use serde_json::Value;

/// 单元格原始值（与 openpyxl 读出的 Python 值一一对应）
#[derive(Clone, Debug, PartialEq)]
pub enum RawVal {
    Empty,
    Text(String),
    Num(f64),
    Date(NaiveDate),
    /// 一天中的时间（Excel 时间分数 0..1）
    Time(f64),
}

impl RawVal {
    pub fn is_empty(&self) -> bool {
        match self {
            RawVal::Empty => true,
            RawVal::Text(s) => s.trim().is_empty(),
            _ => false,
        }
    }
    /// 对应 Python d2s()：date -> "YYYY-MM-DD"，time -> "HH:MM"，其余 str(v)
    pub fn to_display(&self) -> Option<String> {
        match self {
            RawVal::Empty => None,
            RawVal::Text(s) => Some(s.clone()),
            RawVal::Num(f) => Some(fmt_num(*f)),
            RawVal::Date(d) => Some(d.format("%Y-%m-%d").to_string()),
            RawVal::Time(f) => Some(fmt_hm(*f)),
        }
    }
    pub fn date(&self) -> Option<NaiveDate> {
        match self {
            RawVal::Date(d) => Some(*d),
            _ => None,
        }
    }
    pub fn num(&self) -> Option<f64> {
        match self {
            RawVal::Num(f) => Some(*f),
            _ => None,
        }
    }
    /// 判断"数值为 1"（效果图标记，等价 Python str(v)=="1" 对整数 1 的判定）
    pub fn is_one(&self) -> bool {
        match self {
            RawVal::Num(f) => *f == 1.0,
            RawVal::Text(s) => s.trim() == "1",
            _ => false,
        }
    }
}

pub fn fmt_num(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{}", f)
    }
}

pub fn fmt_hm(frac: f64) -> String {
    let secs = (frac * 86400.0).round() as i64;
    let secs = secs.clamp(0, 86399);
    format!("{:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

/// 等价 Python norm_phone：只保留数字
pub fn norm_phone(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// 等价 Python parse_date：接受 - / . 三种分隔
pub fn parse_date_str(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    for fmt in ["%Y-%m-%d", "%Y/%m/%d", "%Y.%m.%d"] {
        if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            return Some(d);
        }
    }
    None
}

/// 等价 Python parse_time：^(\d{1,2}):(\d{2})
pub fn parse_time_frac(s: &str) -> Option<f64> {
    let s = s.trim();
    let chars: Vec<char> = s.chars().collect();
    let mut h = String::new();
    let mut i = 0;
    while i < chars.len() && chars[i].is_ascii_digit() && h.len() < 2 {
        h.push(chars[i]);
        i += 1;
    }
    if h.is_empty() || i >= chars.len() || chars[i] != ':' {
        return None;
    }
    let m: String = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
    if m.len() < 2 {
        return None;
    }
    let hh: i64 = h.parse().ok()?;
    let mm: i64 = m.parse().ok()?;
    if mm >= 60 {
        return None;
    }
    Some(((hh % 24) * 3600 + mm * 60) as f64 / 86400.0)
}

pub fn now_time_frac() -> f64 {
    let t = local_now();
    (t.hour() * 3600 + t.minute() * 60 + t.second()) as f64 / 86400.0
}

/// 本地当前时间。chrono 的 clock 特性会在 macOS 链接 CoreFoundation 框架，
/// 导致交叉编译需要 macOS SDK；因此改用平台 API 自实现（unix: localtime_r，windows: GetLocalTime）
pub fn local_now() -> NaiveDateTime {
    #[cfg(unix)]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&secs, &mut tm).is_null() {
                return NaiveDate::from_ymd_opt(1970, 1, 1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap();
            }
            NaiveDate::from_ymd_opt(tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32)
                .unwrap()
                .and_hms_opt(tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32)
                .unwrap()
        }
    }
    #[cfg(windows)]
    {
        unsafe {
            use windows_sys::Win32::Foundation::SYSTEMTIME;
            use windows_sys::Win32::System::SystemInformation::GetLocalTime;
            let mut st: SYSTEMTIME = std::mem::zeroed();
            GetLocalTime(&mut st);
            NaiveDate::from_ymd_opt(st.wYear as i32, st.wMonth as u32, st.wDay as u32)
                .unwrap()
                .and_hms_opt(st.wHour as u32, st.wMinute as u32, st.wSecond as u32)
                .unwrap()
        }
    }
}

/// Excel 1900 日期系统串行值（与 openpyxl 一致，基准 1899-12-30）
pub fn excel_serial(d: NaiveDate) -> f64 {
    (d - NaiveDate::from_ymd_opt(1899, 12, 30).unwrap()).num_days() as f64
}

pub fn col_to_num(letters: &str) -> Option<u32> {
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let mut n: u32 = 0;
    for c in letters.chars() {
        let u = c.to_ascii_uppercase();
        if !('A'..='Z').contains(&u) {
            return None;
        }
        n = n * 26 + (u as u32 - 'A' as u32 + 1);
    }
    Some(n)
}

pub fn num_to_col(mut n: u32) -> String {
    let mut out = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        out.push(b'A' + rem);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// JSON 数值/字符串 -> f64（等价 Python to_num）
pub fn to_num(v: &Value) -> Option<f64> {
    match v {
        Value::Null => None,
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                t.parse::<f64>().ok()
            }
        }
        _ => None,
    }
}

/// 会员/上课记录的原始行
#[derive(Clone, Debug)]
pub struct RawMember {
    pub row: u32,
    pub id: String,
    pub name: String,
    pub phone: String,
    pub teacher: String,
    pub member_type: String,
    pub card_type: String,
    pub purchase: RawVal,
    pub activation: RawVal,
    pub expiry: RawVal,
    pub amount: Option<f64>,
    pub receivable: Option<f64>,
    pub total: Option<f64>,
    pub refund: bool,
    pub remark: String,
}

#[derive(Clone, Debug)]
pub struct RawClass {
    pub row: u32,
    pub seq: Value,
    pub member_id: String,
    pub date: RawVal,
    pub time: RawVal,
    pub course: String,
    pub effect: bool,
    pub remark: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cols() {
        assert_eq!(col_to_num("A"), Some(1));
        assert_eq!(col_to_num("W"), Some(23));
        assert_eq!(col_to_num("L"), Some(12));
        assert_eq!(num_to_col(23), "W");
        assert_eq!(num_to_col(1), "A");
        assert_eq!(num_to_col(28), "AB");
    }

    #[test]
    fn test_parse() {
        assert!(parse_date_str("2026-8-8").is_some());
        assert!(parse_date_str("2026/08/08").is_some());
        assert!(parse_date_str("2026.08.08").is_some());
        assert!(parse_date_str("2026-8-8 ").is_some());
        assert!(parse_date_str("abc").is_none());
        assert_eq!(parse_time_frac("11:00"), Some(11.0 / 24.0));
        assert_eq!(parse_time_frac("9:30"), Some(9.5 / 24.0));
        assert_eq!(parse_time_frac("x"), None);
    }

    #[test]
    fn test_serial() {
        let d = NaiveDate::from_ymd_opt(2026, 8, 28).unwrap();
        assert_eq!(excel_serial(d), 46262.0);
        assert_eq!(excel_serial(NaiveDate::from_ymd_opt(1900, 1, 1).unwrap()), 2.0);
    }
}
