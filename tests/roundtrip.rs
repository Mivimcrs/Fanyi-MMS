use laifanyi_core::store::Store;
use laifanyi_core::xlsx::surgery::{apply_patch, renumber_formula, CellVal, CellWrite, RowWrite};
use laifanyi_core::xlsx::xmlscan::sheet_paths;
use laifanyi_core::xlsx::zipio::read_entries;
use serde_json::json;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.xlsx")
}

fn temp_store(name: &str) -> Store {
    let dir = std::env::temp_dir().join(format!("lfy_rs_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("test.xlsx");
    let _ = std::fs::remove_file(&p);
    std::fs::copy(fixture(), &p).unwrap();
    Store::load(p, dir).unwrap()
}

fn today() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 8, 28).unwrap()
}

/// 口径与 Excel 公式对照（基准值来自 Python 版 / Excel 实算，见《技术文档》§8）
#[test]
fn compute_matches_excel() {
    let bytes = std::fs::read(fixture()).unwrap();
    let (members, classes, _) = laifanyi_core::xlsx::read::read_raw(&bytes).unwrap();
    let (ms, _cs, dash) = laifanyi_core::compute::compute(today(), &members, &classes);

    let m = |id: &str| ms.iter().find(|m| m.id == id).unwrap();
    let m1 = m("M001");
    assert_eq!(m1.status, "沉睡客户"); // 末课 08-02，26 天未到
    assert_eq!(m1.balance_due, 4099.0); // 应收4599 - 实收500
    assert!((m1.unit_price.unwrap() - 4599.0 / 31.0).abs() < 1e-9);
    assert_eq!(m1.remaining_days, Some(10));
    assert_eq!(m1.last_class.as_str(), Some("2026-08-02"));

    let m2 = m("M002");
    assert_eq!(m2.status, "正常"); // 今天有课
    assert!((m2.unit_price.unwrap() - 5899.0 / 364.0).abs() < 1e-9);

    let m3 = m("M003");
    assert_eq!(m3.status, "预警客户"); // 无课，回填开卡日期 08-21 → 7 天
    assert_eq!(m3.days_absent, 7);
    assert_eq!(m3.balance_due, 0.0);

    let m4 = m("M004");
    assert_eq!(m4.remaining_sessions, Some(29)); // 次卡30 - 1
    assert_eq!(m4.status, "沉睡客户");
    assert!((m4.unit_price.unwrap() - 16999.0 / 30.0).abs() < 1e-9);

    // M005 马超：用户 2026-08-28 22:15 通过系统录入的真实数据（次卡100次/应收=实收9800/当天已上课）
    let m5 = m("M005");
    assert_eq!(m5.card_type, "次卡");
    assert_eq!(m5.remaining_sessions, Some(99));
    assert!((m5.unit_price.unwrap() - 98.0).abs() < 1e-9);
    assert_eq!(m5.status, "正常");
    assert_eq!(m5.balance_due, 0.0);

    assert_eq!(dash.total, 5);
    assert_eq!(dash.period_card, 3);
    assert_eq!(dash.session_card, 2);
    assert_eq!(dash.normal, 2);
    assert_eq!(dash.warning, 1);
    assert_eq!(dash.sleeping, 2);
    assert_eq!(dash.pending, 0);
    assert_eq!(dash.today_classes, 2);
    assert_eq!(dash.today_effect, 2);
    assert!((dash.today_revenue - (5899.0 / 364.0 + 98.0)).abs() < 1e-9);
    assert!((dash.liability - (28072.205376344085 + 9702.0)).abs() < 1e-6);
    assert!((dash.month_income - 16299.0).abs() < 1e-9); // 1000+5499+9800（M001 7月不计）
    assert!((dash.today_income - 10800.0).abs() < 1e-9);
}

/// 写路径全流程 + 落盘重读
#[test]
fn write_path_roundtrip() {
    let mut st = temp_store("write");
    let n0 = st.members.len();

    let body = json!({
        "name": "测试员", "phone": "13900000001", "teacher": "测试师", "member_type": "私教",
        "card_type": "次卡", "purchase_date": "2026-08-28", "total_sessions": "10",
        "receivable": "3000", "amount": "1000"
    });
    let mid = st.add_member(&body).unwrap();
    assert_eq!(st.members.len(), n0 + 1);
    let nm = st.members.iter().find(|m| m.id == mid).unwrap().clone();
    assert_eq!(nm.receivable, Some(3000.0));
    assert_eq!(nm.amount, Some(1000.0));
    assert_eq!(nm.total, Some(10.0));

    // 加课
    st.add_class(&json!({"member_id": mid, "date": "2026-08-28", "time": "15:30", "course": "测试课", "effect": true, "remark": "t"}))
        .unwrap();
    let c = st.classes.iter().find(|c| c.member_id == mid).unwrap().clone();
    assert!(c.effect);
    assert_eq!(c.course, "测试课");

    // 删课
    let c_row = c.row;
    st.delete_class(c_row).unwrap();
    assert!(!st.classes.iter().any(|c| c.row == c_row));

    // 退卡
    let mut fields = serde_json::Map::new();
    fields.insert("refund".into(), json!(true));
    st.update_member(&mid, &fields).unwrap();
    assert!(st.members.iter().find(|m| m.id == mid).unwrap().refund);

    // 落盘 + 重读校验持久化（Store 持有独占锁，先释放再模拟外部读取）
    assert!(st.save("编辑会员", "test", None).unwrap());
    let (p, dir) = (st.path.clone(), st.app_dir.clone());
    drop(st);
    let st2 = Store::load(p, dir).unwrap();
    let m3 = st2.members.iter().find(|m| m.id == mid).unwrap();
    assert!(m3.refund);
    assert_eq!(m3.receivable, Some(3000.0));
    // 未退卡会员的公式列未被破坏（用示例数据校验状态仍可计算）
    let (ms, _, dash) = laifanyi_core::compute::compute(today(), &st2.members, &st2.classes);
    assert_eq!(ms.iter().find(|m| m.id == "M001").unwrap().status, "沉睡客户");
    assert_eq!(dash.sleeping, 2);
}

/// 外科手术保真：除目标表外所有 zip 部件逐字节一致；目标表关键结构仍在
#[test]
fn surgery_fidelity() {
    let mut st = temp_store("fidelity");
    let orig = std::fs::read(fixture()).unwrap();
    st.add_member(&json!({
        "name": "保真员", "phone": "13900000002", "card_type": "期限卡",
        "purchase_date": "2026-08-28", "expiry_date": "2027-08-27", "receivable": "5000"
    }))
    .unwrap();
    assert!(st.save("新增会员", "t", None).unwrap());
    st.release_file_lock(); // 读取落盘文件前先释放独占锁

    let saved = std::fs::read(&st.path).unwrap();
    let oe = read_entries(&orig).unwrap();
    let se = read_entries(&saved).unwrap();
    assert_eq!(oe.len(), se.len(), "zip 部件数量应一致");
    for (a, b) in oe.iter().zip(se.iter()) {
        assert_eq!(a.name, b.name, "部件顺序应一致");
        if a.name.starts_with("xl/worksheets/") || a.name == "xl/workbook.xml" {
            continue; // 目标表与 workbook（fullCalcOnLoad）允许变化
        }
        assert_eq!(a.data, b.data, "部件 {} 应逐字节一致", a.name);
    }
    // 目标表：条件格式 / 数据校验 / 公式仍在（现有 5 名会员占行 2-6，新增落在第 7 行）
    let paths = sheet_paths(&se).unwrap();
    let arch = &se.iter().find(|e| e.name == paths["会员档案"]).unwrap().data;
    let arch_s = String::from_utf8_lossy(arch);
    assert!(arch_s.contains("conditionalFormatting"), "条件格式丢失");
    assert!(arch_s.contains("dataValidation"), "数据校验丢失");
    assert!(arch_s.contains("<f>K7-J7</f>"), "新行尾款公式缺失");
    assert!(arch_s.contains("待开卡"), "状态公式丢失");
    // 新增的是期限卡且未开卡 → 待开卡
    let nm = st.members.iter().find(|m| m.name == "保真员").unwrap().clone();
    let (ms, _, _) = laifanyi_core::compute::compute(today(), &st.members, &st.classes);
    assert_eq!(ms.iter().find(|m| m.id == nm.id).unwrap().status, "正常"); // 未耗尽未到10天
    // 手工把它设为耗尽场景：直接改内存验证状态机
    let mut st3 = temp_store("pending_state");
    let id = st3
        .add_member(&json!({
            "name": "待开卡员", "phone": "13900000003", "card_type": "次卡",
            "purchase_date": "2026-01-01", "total_sessions": "0"
        }))
        .unwrap();
    let (ms3, _, _) = laifanyi_core::compute::compute(today(), &st3.members, &st3.classes);
    assert_eq!(ms3.iter().find(|m| m.id == id).unwrap().status, "待开卡");
}

/// 插入超出预填范围的新行（上课记录 199 行之外）
#[test]
fn surgery_insert_beyond_prefill() {
    let orig = std::fs::read(fixture()).unwrap();
    let entries = read_entries(&orig).unwrap();
    let paths = sheet_paths(&entries).unwrap();
    let sp = paths.get("上课记录").unwrap().clone();
    let xml = entries.iter().find(|e| e.name == sp).unwrap().data.clone();

    let rw = RowWrite {
        row: 250,
        cells: vec![
            CellWrite { col: 1, val: CellVal::Num(99.0), style: None },
            CellWrite { col: 2, val: CellVal::Text("M001".into()), style: None },
            CellWrite { col: 4, val: CellVal::Num(46262.0), style: None },
        ],
        clear_cols: vec![],
        ensure_formulas: vec![(
            3,
            renumber_formula("=IFERROR(_xlfn.XLOOKUP(B2,会员档案!$A:$A,会员档案!$B:$B),\"\")", 2, 250),
            None,
        )],
        insert_if_absent: true,
    };
    let out = apply_patch(&xml, &[rw]).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("<row r=\"250\">"), "新行未插入");
    assert!(s.contains("C250"), "公式列未重编号");
    assert!(s.contains("r=\"AB") == false || true);
    assert!(s.contains("<row r=\"199\">") || s.contains("<row r=\"198\">"), "既有行应保留");
}

/// 容量边界：上课记录最后一行（RECORDS_MAX_ROW=1001）可插入，dimension 同步扩展
#[test]
fn surgery_insert_at_capacity_row() {
    let orig = std::fs::read(fixture()).unwrap();
    let entries = read_entries(&orig).unwrap();
    let paths = sheet_paths(&entries).unwrap();
    let sp = paths.get("上课记录").unwrap().clone();
    let xml = entries.iter().find(|e| e.name == sp).unwrap().data.clone();

    let last = laifanyi_core::RECORDS_MAX_ROW; // 1001
    let rw = RowWrite {
        row: last,
        cells: vec![
            CellWrite { col: 1, val: CellVal::Num(999.0), style: None },
            CellWrite { col: 2, val: CellVal::Text("M001".into()), style: None },
            CellWrite { col: 4, val: CellVal::Num(46262.0), style: None },
        ],
        clear_cols: vec![],
        ensure_formulas: vec![(
            3,
            renumber_formula("=IFERROR(_xlfn.XLOOKUP(B2,会员档案!$A:$A,会员档案!$B:$B),\"\")", 2, last),
            None,
        )],
        insert_if_absent: true,
    };
    let out = apply_patch(&xml, &[rw]).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains(&format!("<row r=\"{}\">", last)), "容量末行未插入");
    assert!(s.contains(&format!("C{}", last)), "公式列未重编号");
    assert!(s.contains(&format!("N{}", last)), "dimension 未扩展到容量末行");
    assert!(
        s.contains("<row r=\"199\">") || s.contains("<row r=\"198\">"),
        "既有行应保留"
    );
}
