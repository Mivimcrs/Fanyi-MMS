use crate::model::{RawClass, RawMember, RawVal};
use chrono::NaiveDate;
use serde::Serialize;

pub const VALID_CARD_TYPES: [&str; 2] = ["次卡", "期限卡"];
pub const STATUS_EXPIRED: &str = "已到期";
pub const STATUS_SLEEP: &str = "沉睡客户";
pub const STATUS_WARN: &str = "预警客户";
pub const STATUS_NORMAL: &str = "正常";
pub const STATUS_PENDING: &str = "待开卡";

#[derive(Serialize)]
pub struct MemberOut {
    pub row: u32,
    pub id: String,
    pub name: String,
    pub phone: String,
    pub teacher: String,
    pub member_type: String,
    pub card_type: String,
    pub purchase_date: serde_json::Value,
    pub activation_date: serde_json::Value,
    pub expiry_date: serde_json::Value,
    pub amount: Option<f64>,
    pub receivable: Option<f64>,
    pub balance_due: f64,
    pub total_sessions: serde_json::Value,
    pub consumed: usize,
    pub remaining_sessions: Option<i64>,
    pub remaining_days: Option<i64>,
    pub unit_price: Option<f64>,
    pub remaining_value: f64,
    pub last_class: serde_json::Value,
    pub days_absent: i64,
    pub status: String,
    pub refund: bool,
    pub remark: String,
}

#[derive(Serialize)]
pub struct ClassOut {
    pub row: u32,
    pub seq: serde_json::Value,
    pub member_id: String,
    pub member_name: String,
    pub teacher: String,
    pub member_type: String,
    pub card_type: String,
    pub date: serde_json::Value,
    pub time: serde_json::Value,
    pub unit_price: Option<f64>,
    pub valid: bool,
    pub course: String,
    pub effect: bool,
    pub remark: String,
}

#[derive(Serialize)]
pub struct Dashboard {
    pub today: String,
    pub total: usize,
    pub valid_period: usize,
    pub period_card: usize,
    pub session_card: usize,
    pub refunded: usize,
    pub normal: usize,
    pub pending: usize,
    pub warning: usize,
    pub sleeping: usize,
    pub expired: usize,
    pub today_revenue: f64,
    pub month_revenue: f64,
    pub today_income: f64,
    pub month_income: f64,
    pub liability: f64,
    pub today_classes: usize,
    pub today_effect: usize,
}

fn disp(v: &RawVal) -> serde_json::Value {
    match v.to_display() {
        Some(s) => serde_json::Value::String(s),
        None => serde_json::Value::Null,
    }
}

/// Python round(x, 2) 的常用等价（正确十进制舍入）
fn round2(x: f64) -> f64 {
    let s = format!("{:.2}", x);
    s.parse().unwrap_or(x)
}

/// 公式镜像计算（与《技术文档》§8 口径一致，today 显式传入便于测试）
pub fn compute(
    today: NaiveDate,
    members: &[RawMember],
    classes: &[RawClass],
) -> (Vec<MemberOut>, Vec<ClassOut>, Dashboard) {
    let today_s = today.format("%Y-%m-%d").to_string();
    let month_p = &today_s[..7];

    let mut out_members: Vec<MemberOut> = Vec::with_capacity(members.len());
    let mut unit_by_id: std::collections::HashMap<&str, Option<f64>> = std::collections::HashMap::new();

    for m in members {
        let cs: Vec<&RawClass> = classes.iter().filter(|c| c.member_id == m.id).collect();
        let consumed = cs.len();
        let ct = m.card_type.as_str();

        let (remaining_sessions, remaining_days) = if ct == "次卡" {
            let total = m.total.unwrap_or(0.0) as i64;
            (Some((total - consumed as i64).max(0)), None)
        } else if ct == "期限卡" {
            let rd = m.expiry.date().map(|e| (e - today).num_days().max(0)).unwrap_or(0);
            (None, Some(rd))
        } else {
            (None, None)
        };

        // 课程单价：次卡 应收K/总次数M；期限卡 应收K/(有效期至I-开卡日期H)
        let mut unit: Option<f64> = None;
        if ct == "次卡" {
            if let (Some(recv), Some(true)) = (m.receivable, m.total.map(|t| t > 0.0)) {
                if recv != 0.0 {
                    unit = Some(recv / m.total.unwrap());
                }
            }
        } else if ct == "期限卡" {
            if let Some(recv) = m.receivable {
                if recv != 0.0 {
                    if let (Some(act), Some(exp)) = (m.activation.date(), m.expiry.date()) {
                        if exp > act {
                            let days = ((exp - act).num_days() as f64).max(1.0);
                            unit = Some(recv / days);
                        }
                    }
                }
            }
        }
        unit_by_id.insert(m.id.as_str(), unit);

        let remaining_value = match (unit, ct) {
            (Some(u), "次卡") => u * remaining_sessions.unwrap_or(0) as f64,
            (Some(u), "期限卡") => u * remaining_days.unwrap_or(0) as f64,
            _ => 0.0,
        };

        // 最后上课日期：有课取最大日期；否则回填开卡日期
        let mut last: Option<NaiveDate> = None;
        for c in &cs {
            if let Some(d) = c.date.date() {
                if last.is_none() || Some(d) > last {
                    last = Some(d);
                }
            }
        }
        if last.is_none() {
            last = m.activation.date();
        }
        let days_absent = match last {
            Some(l) => (today - l).num_days(),
            None => 0,
        };

        // 会员状态（与 Excel U 列公式一致）
        let exhausted = (ct == "次卡" && remaining_sessions == Some(0))
            || (ct == "期限卡" && remaining_days == Some(0));
        let mut status = if exhausted {
            if m.activation.is_empty() {
                STATUS_PENDING.to_string()
            } else {
                STATUS_EXPIRED.to_string()
            }
        } else if days_absent >= 10 {
            STATUS_SLEEP.to_string()
        } else if days_absent >= 7 {
            STATUS_WARN.to_string()
        } else {
            STATUS_NORMAL.to_string()
        };
        if m.refund {
            status = "退卡".to_string();
        }

        out_members.push(MemberOut {
            row: m.row,
            id: m.id.clone(),
            name: m.name.clone(),
            phone: m.phone.clone(),
            teacher: m.teacher.clone(),
            member_type: m.member_type.clone(),
            card_type: m.card_type.clone(),
            purchase_date: disp(&m.purchase),
            activation_date: disp(&m.activation),
            expiry_date: disp(&m.expiry),
            amount: m.amount,
            receivable: m.receivable,
            balance_due: round2(m.receivable.unwrap_or(0.0) - m.amount.unwrap_or(0.0)),
            total_sessions: serde_json::json!(m.total),
            consumed,
            remaining_sessions,
            remaining_days,
            unit_price: unit,
            remaining_value,
            last_class: match last {
                Some(d) => serde_json::Value::String(d.format("%Y-%m-%d").to_string()),
                None => serde_json::Value::Null,
            },
            days_absent,
            status,
            refund: m.refund,
            remark: m.remark.clone(),
        });
    }

    let member_by_id: std::collections::HashMap<&str, &RawMember> =
        members.iter().map(|m| (m.id.as_str(), m)).collect();

    let mut out_classes: Vec<ClassOut> = Vec::with_capacity(classes.len());
    for c in classes {
        let m = member_by_id.get(c.member_id.as_str()).copied();
        let ct = m.map(|x| x.card_type.as_str()).unwrap_or("");
        out_classes.push(ClassOut {
            row: c.row,
            seq: c.seq.clone(),
            member_id: c.member_id.clone(),
            member_name: m.map(|x| x.name.clone()).unwrap_or_default(),
            teacher: m.map(|x| x.teacher.clone()).unwrap_or_default(),
            member_type: m.map(|x| x.member_type.clone()).unwrap_or_default(),
            card_type: ct.to_string(),
            date: disp(&c.date),
            time: disp(&c.time),
            unit_price: m.and_then(|_| unit_by_id.get(c.member_id.as_str()).copied().flatten()),
            valid: VALID_CARD_TYPES.contains(&ct),
            course: c.course.clone(),
            effect: c.effect,
            remark: c.remark.clone(),
        });
    }

    // ---- dashboard ----
    let active: Vec<&MemberOut> = out_members.iter().filter(|m| !m.refund).collect();
    let valid_classes: Vec<&ClassOut> = out_classes.iter().filter(|c| c.valid).collect();
    let rev = |list: &[&ClassOut]| -> f64 {
        list.iter().filter_map(|c| c.unit_price).sum()
    };

    let dashboard = Dashboard {
        today: today_s.clone(),
        total: out_members.len(),
        valid_period: active
            .iter()
            .filter(|m| matches!(m.expiry_date.as_str(), Some(e) if e >= today_s.as_str()))
            .count(),
        period_card: active.iter().filter(|m| m.card_type == "期限卡").count(),
        session_card: active.iter().filter(|m| m.card_type == "次卡").count(),
        refunded: out_members.iter().filter(|m| m.refund).count(),
        normal: active.iter().filter(|m| m.status == STATUS_NORMAL).count(),
        pending: active.iter().filter(|m| m.status == STATUS_PENDING).count(),
        warning: active.iter().filter(|m| m.status == STATUS_WARN).count(),
        sleeping: active.iter().filter(|m| m.status == STATUS_SLEEP).count(),
        expired: active.iter().filter(|m| m.status == STATUS_EXPIRED).count(),
        today_revenue: rev(
            &valid_classes
                .iter()
                .filter(|c| c.date.as_str() == Some(today_s.as_str()))
                .cloned()
                .collect::<Vec<_>>(),
        ),
        month_revenue: rev(
            &valid_classes
                .iter()
                .filter(|c| {
                    matches!(c.date.as_str(), Some(d) if d.starts_with(month_p))
                })
                .cloned()
                .collect::<Vec<_>>(),
        ),
        today_income: out_members
            .iter()
            .filter(|m| m.purchase_date.as_str() == Some(today_s.as_str()))
            .filter_map(|m| m.amount)
            .sum(),
        month_income: out_members
            .iter()
            .filter(|m| matches!(m.purchase_date.as_str(), Some(d) if d.starts_with(month_p)))
            .filter_map(|m| m.amount)
            .sum(),
        liability: active.iter().map(|m| m.remaining_value).sum(),
        today_classes: valid_classes
            .iter()
            .filter(|c| c.date.as_str() == Some(today_s.as_str()))
            .count(),
        today_effect: valid_classes
            .iter()
            .filter(|c| c.date.as_str() == Some(today_s.as_str()) && c.effect)
            .count(),
    };

    (out_members, out_classes, dashboard)
}
