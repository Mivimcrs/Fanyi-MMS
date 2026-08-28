use crate::model::{
    excel_serial, fmt_num, norm_phone, now_time_frac, parse_date_str, parse_time_frac, RawClass,
    RawMember,
};
use crate::snapshot;
use crate::xlsx::read::read_raw;
use crate::xlsx::surgery::{apply_patch, renumber_formula, CellVal, CellWrite, RowWrite};
use crate::xlsx::xmlscan::{row_templates, sheet_paths, CellTpl};
use crate::xlsx::zipio::{finalize_and_write, read_entries, Entry};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn now_ts() -> String {
    crate::model::local_now().format("%Y-%m-%d %H:%M:%S").to_string()
}

const OCCUPIED_MSG: &str = "表格文件正被 Excel/WPS 打开，暂存于服务中；请关闭文件后点击“重试保存”。";

/// 挂起工件（保存失败时把内存工作簿落盘，进程退出后仍可恢复）
pub fn pending_bytes_path(app_dir: &Path) -> PathBuf {
    app_dir.join(".laifanyi.pending.xlsx")
}

pub fn pending_meta_path(app_dir: &Path) -> PathBuf {
    app_dir.join(".laifanyi.pending.json")
}

/// 删除挂起工件（保存成功 / 恢复 / 丢弃 / 换表时调用；单槽位语义）
pub fn remove_pending_files(app_dir: &Path) {
    let _ = std::fs::remove_file(pending_bytes_path(app_dir));
    let _ = std::fs::remove_file(pending_meta_path(app_dir));
}

fn same_path(a: &Path, b: &Path) -> bool {
    if let (Ok(x), Ok(y)) = (a.canonicalize(), b.canonicalize()) {
        return x == y;
    }
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

/// 启动检测：存在挂起工件且其目标与当前表格一致时返回元数据
fn detect_recoverable(path: &Path, app_dir: &Path) -> Option<Value> {
    if !pending_bytes_path(app_dir).is_file() {
        return None;
    }
    let meta: Value =
        serde_json::from_slice(&std::fs::read(pending_meta_path(app_dir)).ok()?).ok()?;
    let target = PathBuf::from(meta.get("target")?.as_str()?);
    let full = if target.is_absolute() {
        target
    } else {
        app_dir.join(target)
    };
    if !same_path(&full, path) {
        return None;
    }
    Some(meta)
}

#[derive(Debug)]
pub enum StoreError {
    /// 业务校验失败 -> HTTP 400
    Biz(String),
    /// 系统异常 -> HTTP 500
    Sys(String),
}

fn tpl_style_of(tpl: &BTreeMap<u32, CellTpl>, col: u32) -> Option<String> {
    tpl.get(&col).and_then(|t| t.style.clone())
}

fn tpl_formula_of(
    tpl: &BTreeMap<u32, CellTpl>,
    col: u32,
    from: u32,
    to: u32,
) -> Option<(String, Option<String>)> {
    tpl.get(&col).and_then(|t| {
        t.formula
            .as_ref()
            .map(|f| (renumber_formula(f, from, to), t.style.clone()))
    })
}

pub struct Store {
    pub path: PathBuf,
    pub app_dir: PathBuf,
    entries: Vec<Entry>,
    doc_bytes: Vec<u8>,
    pub members: Vec<RawMember>,
    pub classes: Vec<RawClass>,
    tpl_archive: BTreeMap<u32, CellTpl>,
    tpl_class: BTreeMap<u32, CellTpl>,
    sheet_archive: String,
    sheet_class: String,
    refund_header_present: bool,
    pub pending: bool,
    pub pending_reason: Option<String>,
    pub last_saved: Option<String>,
    /// 启动时检测到的挂起变更元数据（保存失败落盘的工作簿可恢复）
    pub recoverable: Option<Value>,
}

fn js_str(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(fmt_num(n.as_f64().unwrap_or(0.0))),
        Value::Bool(b) => Some(if *b { "True" } else { "False" }.to_string()),
        _ => Some(v.to_string()),
    }
}

impl Store {
    pub fn load(path: PathBuf, app_dir: PathBuf) -> Result<Store, String> {
        let bytes = std::fs::read(&path).map_err(|e| format!("OSError: {}", e))?;
        let mut st = Self::from_bytes(path, app_dir, bytes)?;
        st.last_saved = None;
        Ok(st)
    }

    pub fn from_bytes(path: PathBuf, app_dir: PathBuf, bytes: Vec<u8>) -> Result<Store, String> {
        let entries = read_entries(&bytes)?;
        let paths = sheet_paths(&entries)?;
        let sheet_archive = paths
            .get(crate::SHEET_ARCHIVE)
            .cloned()
            .ok_or_else(|| format!("该表格缺少工作表：{}", crate::SHEET_ARCHIVE))?;
        let sheet_class = paths
            .get(crate::SHEET_RECORDS)
            .cloned()
            .ok_or_else(|| format!("该表格缺少工作表：{}", crate::SHEET_RECORDS))?;
        let (members, classes, refund_header_present) = read_raw(&bytes)?;
        let arch_xml = find_entry_bytes(&entries, &sheet_archive)?;
        let cls_xml = find_entry_bytes(&entries, &sheet_class)?;
        let tpl_archive = row_templates(arch_xml, 2);
        let tpl_class = row_templates(cls_xml, 2);
        let doc_bytes = finalize_and_write(&entries)?;
        let recoverable = detect_recoverable(&path, &app_dir);
        Ok(Store {
            path,
            app_dir,
            entries,
            doc_bytes,
            members,
            classes,
            tpl_archive,
            tpl_class,
            sheet_archive,
            sheet_class,
            refund_header_present,
            pending: false,
            pending_reason: None,
            last_saved: None,
            recoverable,
        })
    }

    fn rebuild(&mut self) -> Result<(), StoreError> {
        // 用补丁后的条目重打 zip，并重读原始行
        self.doc_bytes = finalize_and_write(&self.entries).map_err(StoreError::Sys)?;
        let (members, classes, header) = read_raw(&self.doc_bytes).map_err(StoreError::Sys)?;
        self.members = members;
        self.classes = classes;
        self.refund_header_present = header;
        Ok(())
    }

    fn patch_sheet(&mut self, sheet: &str, writes: Vec<RowWrite>) -> Result<(), StoreError> {
        let path = sheet.to_string();
        let idx = self
            .entries
            .iter()
            .position(|e| e.name == path)
            .ok_or_else(|| StoreError::Sys(format!("missing sheet part {}", path)))?;
        let new_xml = apply_patch(&self.entries[idx].data, &writes).map_err(StoreError::Sys)?;
        self.entries[idx].data = new_xml;
        self.rebuild()
    }

    // ------------------------------------------------------------- 查询
    pub fn data_json(&self) -> Value {
        let today = crate::model::local_now().date();
        let (m, c, d) = crate::compute::compute(today, &self.members, &self.classes);
        json!({"members": m, "classes": c, "dashboard": d, "meta": self.meta()})
    }

    pub fn meta(&self) -> Value {
        json!({
            "excel_path": self.path.display().to_string(),
            "saved": !self.pending,
            "pending_reason": self.pending_reason,
            "last_saved": self.last_saved,
            "member_capacity": crate::ARCHIVE_MAX_ROW - 1,
            "class_capacity": crate::RECORDS_MAX_ROW - 1,
            "pending_recovery": self.recoverable,
        })
    }

    fn first_empty_row(rows: &[u32], max_row: u32) -> Option<u32> {
        let used: std::collections::HashSet<u32> = rows.iter().copied().collect();
        (2..=max_row).find(|r| !used.contains(r))
    }

    fn tpl_style(tpl: &BTreeMap<u32, CellTpl>, col: u32) -> Option<String> {
        tpl.get(&col).and_then(|t| t.style.clone())
    }

    // ------------------------------------------------------------- 编号
    pub fn gen_member_id(&self, phone: &str, base_id: Option<&str>) -> String {
        let base = match base_id {
            Some(b) => b.split('-').next().unwrap_or("").to_string(),
            None => {
                let np = norm_phone(phone);
                let same = self.members.iter().find(|m| {
                    let mp = norm_phone(&m.phone);
                    !mp.is_empty() && mp == np
                });
                match same {
                    Some(m) => m.id.split('-').next().unwrap_or("").to_string(),
                    None => {
                        let mut max = 0i64;
                        for m in &self.members {
                            let id = &m.id;
                            if let Some(rest) = id.strip_prefix('M') {
                                if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                                    if let Ok(v) = rest.parse::<i64>() {
                                        max = max.max(v);
                                    }
                                }
                            }
                        }
                        return format!("M{:03}", max + 1);
                    }
                }
            }
        };
        let mut max = 0i64;
        let prefix = format!("{}-", base);
        for m in &self.members {
            if let Some(rest) = m.id.strip_prefix(&prefix) {
                if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(v) = rest.parse::<i64>() {
                        max = max.max(v);
                    }
                }
            }
        }
        format!("{}-{}", base, max + 1)
    }

    // ------------------------------------------------------------- 变更
    pub fn add_member(&mut self, body: &Value) -> Result<String, StoreError> {
        let gstr = |k: &str| body.get(k).and_then(js_str).unwrap_or_default();
        let row = Self::first_empty_row(
            &self.members.iter().map(|m| m.row).collect::<Vec<_>>(),
            crate::ARCHIVE_MAX_ROW,
        )
        .ok_or_else(|| StoreError::Biz(format!("会员档案已满（{} 行）", crate::ARCHIVE_MAX_ROW - 1)))?;

        let mid = self.gen_member_id(
            &gstr("phone"),
            body.get("base_member_id").and_then(js_str).as_deref(),
        );
        let mut rw = RowWrite::new(row);
        rw.insert_if_absent = true;
        let ta = &self.tpl_archive;
        let t = |c: u32| tpl_style_of(ta, c);
        let mut push = |col: u32, val: CellVal, style: Option<String>| {
            rw.cells.push(CellWrite { col, val, style });
        };
        push(1, CellVal::Text(mid.clone()), t(1));
        push(2, CellVal::Text(gstr("name")), t(2));
        push(3, CellVal::Text(gstr("phone")), t(3));
        push(4, CellVal::Text(gstr("teacher")), t(4));
        push(5, CellVal::Text(gstr("member_type")), t(5));
        push(6, CellVal::Text(gstr("card_type")), t(6));

        let g = parse_date_str(&gstr("purchase_date"))
            .unwrap_or_else(|| crate::model::local_now().date());
        push(7, CellVal::Num(excel_serial(g)), t(7));
        if let Some(a) = parse_date_str(&gstr("activation_date")) {
            push(8, CellVal::Num(excel_serial(a)), t(8));
        }
        if let Some(h) = parse_date_str(&gstr("expiry_date")) {
            push(9, CellVal::Num(excel_serial(h)), t(9));
        }
        if let Some(amt) = crate::model::to_num(body.get("amount").unwrap_or(&Value::Null)) {
            push(10, CellVal::Num(amt), t(10));
        }
        if let Some(recv) = crate::model::to_num(body.get("receivable").unwrap_or(&Value::Null)) {
            push(11, CellVal::Num(recv), t(11));
        }
        if gstr("card_type") == "次卡" {
            if let Some(tot) = crate::model::to_num(body.get("total_sessions").unwrap_or(&Value::Null)) {
                push(13, CellVal::Num(tot.trunc()), t(13));
            }
        }
        if !gstr("remark").is_empty() {
            push(23, CellVal::Text(gstr("remark")), t(23));
        }
        // 公式列：L(12) 与 N..U(14..=21)，模板行 2
        let ensure: Vec<u32> = std::iter::once(12).chain(14..=21).collect();
        for col in ensure {
            if let Some((f, st)) = tpl_formula_of(&self.tpl_archive, col, 2, row) {
                rw.ensure_formulas.push((col, f, st));
            }
        }
        self.patch_sheet(&self.sheet_archive.clone(), vec![rw])?;
        Ok(mid)
    }

    pub fn update_member(&mut self, mid: &str, fields: &Map<String, Value>) -> Result<(), StoreError> {
        let m = self
            .members
            .iter()
            .find(|m| m.id == mid)
            .cloned()
            .ok_or_else(|| StoreError::Biz(format!("会员不存在: {}", mid)))?;
        let row = m.row;
        let mut rw = RowWrite::new(row);
        let tpl = &self.tpl_archive;

        // 文本字段
        for (key, col) in [
            ("name", 2u32),
            ("phone", 3),
            ("teacher", 4),
            ("member_type", 5),
            ("card_type", 6),
        ] {
            if let Some(v) = fields.get(key) {
                if let Some(s) = js_str(v) {
                    rw.cells.push(CellWrite { col, val: CellVal::Text(s), style: tpl_style_of(tpl, col) });
                }
            }
        }
        // 日期字段（值无效时跳过，等价 Python）
        for (key, col) in [("purchase_date", 7u32), ("activation_date", 8), ("expiry_date", 9)] {
            if let Some(v) = fields.get(key) {
                if let Some(s) = js_str(v) {
                    if !s.is_empty() {
                        if let Some(d) = parse_date_str(&s) {
                            rw.cells.push(CellWrite { col, val: CellVal::Num(excel_serial(d)), style: tpl_style_of(tpl, col) });
                        }
                    }
                }
            }
        }
        if let Some(v) = fields.get("amount") {
            match crate::model::to_num(v) {
                Some(a) => rw.cells.push(CellWrite { col: 10, val: CellVal::Num(a), style: tpl_style_of(tpl, 10) }),
                None => rw.clear_cols.push(10),
            }
        }
        if let Some(v) = fields.get("receivable") {
            match crate::model::to_num(v) {
                Some(a) => rw.cells.push(CellWrite { col: 11, val: CellVal::Num(a), style: tpl_style_of(tpl, 11) }),
                None => rw.clear_cols.push(11),
            }
        }
        if let Some(v) = fields.get("total_sessions") {
            match crate::model::to_num(v) {
                Some(a) => rw.cells.push(CellWrite { col: 13, val: CellVal::Num(a.trunc()), style: tpl_style_of(tpl, 13) }),
                None => rw.clear_cols.push(13),
            }
        }
        if let Some(v) = fields.get("remark") {
            match js_str(v) {
                Some(s) if !s.is_empty() => rw.cells.push(CellWrite { col: 23, val: CellVal::Text(s), style: tpl_style_of(tpl, 23) }),
                _ => rw.clear_cols.push(23),
            }
        }
        let mut header_write: Option<RowWrite> = None;
        if let Some(v) = fields.get("refund") {
            let refund = v.as_bool().unwrap_or(false);
            let st22 = tpl_style_of(tpl, 22);
            if refund {
                rw.cells.push(CellWrite { col: 22, val: CellVal::Text("是".to_string()), style: st22.clone() });
                if !self.refund_header_present {
                    let mut h = RowWrite::new(1);
                    h.cells.push(CellWrite { col: 22, val: CellVal::Text("退卡".to_string()), style: st22 });
                    header_write = Some(h);
                    self.refund_header_present = true;
                }
            } else {
                rw.clear_cols.push(22);
            }
        }
        let mut writes = vec![rw];
        if let Some(h) = header_write {
            writes.push(h);
        }
        self.patch_sheet(&self.sheet_archive.clone(), writes)?;
        Ok(())
    }

    pub fn add_class(&mut self, body: &Value) -> Result<(), StoreError> {
        let gstr = |k: &str| body.get(k).and_then(js_str).unwrap_or_default();
        let mid = gstr("member_id");
        if !self.members.iter().any(|m| m.id == mid) {
            return Err(StoreError::Biz(format!("会员编号不存在: {}", mid)));
        }
        let dt = parse_date_str(&gstr("date")).ok_or_else(|| StoreError::Biz("上课日期无效".into()))?;
        let tm = parse_time_frac(&gstr("time")).unwrap_or_else(now_time_frac);

        let row = Self::first_empty_row(
            &self.classes.iter().map(|c| c.row).collect::<Vec<_>>(),
            crate::RECORDS_MAX_ROW,
        )
        .ok_or_else(|| StoreError::Biz(format!("上课记录已满（{} 行）", crate::RECORDS_MAX_ROW - 1)))?;

        let mut seq_max = 0i64;
        for c in &self.classes {
            if let Some(f) = c.seq.as_f64() {
                seq_max = seq_max.max(f as i64);
            } else if let Some(s) = c.seq.as_str() {
                if let Ok(v) = s.trim().parse::<i64>() {
                    seq_max = seq_max.max(v);
                }
            }
        }

        let mut rw = RowWrite::new(row);
        rw.insert_if_absent = true;
        let t = |c: u32| Self::tpl_style(&self.tpl_class, c);
        rw.cells.push(CellWrite { col: 1, val: CellVal::Num((seq_max + 1) as f64), style: t(1) });
        rw.cells.push(CellWrite { col: 2, val: CellVal::Text(mid.clone()), style: t(2) });
        rw.cells.push(CellWrite { col: 4, val: CellVal::Num(excel_serial(dt)), style: t(4) });
        rw.cells.push(CellWrite { col: 5, val: CellVal::Num(tm), style: t(5) });
        let course = gstr("course");
        if !course.is_empty() {
            rw.cells.push(CellWrite { col: 12, val: CellVal::Text(course), style: t(12) });
        }
        if body.get("effect").and_then(Value::as_bool).unwrap_or(false) {
            rw.cells.push(CellWrite { col: 13, val: CellVal::Text("1".to_string()), style: t(13) });
        }
        let remark = gstr("remark");
        if !remark.is_empty() {
            rw.cells.push(CellWrite { col: 14, val: CellVal::Text(remark), style: t(14) });
        }
        for col in 3..=11u32 {
            if let Some((f, st)) = tpl_formula_of(&self.tpl_class, col, 2, row) {
                rw.ensure_formulas.push((col, f, st));
            }
        }
        self.patch_sheet(&self.sheet_class.clone(), vec![rw])?;
        Ok(())
    }

    pub fn delete_class(&mut self, row: u32) -> Result<(), StoreError> {
        if !(2..=crate::RECORDS_MAX_ROW).contains(&row) {
            return Err(StoreError::Biz("行号无效".into()));
        }
        if !self.classes.iter().any(|c| c.row == row) {
            return Err(StoreError::Biz("该行没有上课记录".into()));
        }
        let mut rw = RowWrite::new(row);
        rw.clear_cols = vec![1, 2, 4, 5, 12, 13, 14];
        self.patch_sheet(&self.sheet_class.clone(), vec![rw])?;
        Ok(())
    }

    // ------------------------------------------------------------- 持久化
    pub fn save(&mut self, action: &str, summary: &str, mid: Option<&str>) -> Result<bool, StoreError> {
        if !self.pending {
            snapshot::snapshot(&self.path, &self.app_dir, action, summary, mid);
        }
        let tmp = self.path.with_extension("xlsx.tmp");
        let result = std::fs::write(&tmp, &self.doc_bytes).and_then(|_| std::fs::rename(&tmp, &self.path));
        match result {
            Ok(_) => {
                self.pending = false;
                self.pending_reason = None;
                self.last_saved = Some(now_ts());
                self.recoverable = None;
                remove_pending_files(&self.app_dir);
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                let _ = std::fs::remove_file(&tmp);
                // 变更落盘到挂起工件：进程退出后重启仍可恢复
                self.write_pending_artifacts(action, summary);
                self.pending = true;
                self.pending_reason = Some(OCCUPIED_MSG.to_string());
                Ok(false)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(StoreError::Sys(format!("OSError: {}", e)))
            }
        }
    }

    /// 把内存中未保存的工作簿写到挂起工件（保存失败时调用；测试用于模拟该场景）
    pub fn write_pending_artifacts(&self, action: &str, summary: &str) {
        let ts = now_ts();
        let target = self
            .path
            .strip_prefix(&self.app_dir)
            .map(|r| r.display().to_string())
            .unwrap_or_else(|_| self.path.display().to_string());
        let _ = std::fs::write(pending_bytes_path(&self.app_dir), &self.doc_bytes);
        let meta = json!({"target": target, "ts": ts, "action": action, "summary": summary});
        let _ = std::fs::write(
            pending_meta_path(&self.app_dir),
            serde_json::to_vec_pretty(&meta).unwrap_or_default(),
        );
    }

    /// 用挂起工件覆盖表格文件并重载（工件内容是改动后的完整工作簿）。
    /// 失败前磁盘状态在首次 save 时已自动快照进 versions/，恢复本身不会造成丢失。
    pub fn recover_pending(&mut self) -> Result<(), StoreError> {
        let src = pending_bytes_path(&self.app_dir);
        if !src.is_file() || self.recoverable.is_none() {
            return Err(StoreError::Biz("没有可恢复的挂起变更".into()));
        }
        let bytes = std::fs::read(&src).map_err(|e| StoreError::Sys(format!("OSError: {}", e)))?;
        let tmp = self.path.with_extension("xlsx.tmp");
        let result = std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &self.path));
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(match e.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    StoreError::Biz("表格文件仍被 Excel/WPS 占用，请先关闭后重试".into())
                }
                _ => StoreError::Sys(format!("OSError: {}", e)),
            });
        }
        remove_pending_files(&self.app_dir);
        self.recoverable = None;
        self.reload_file().map_err(StoreError::Sys)?;
        self.last_saved = Some(now_ts());
        Ok(())
    }

    /// 放弃挂起变更（表格保持磁盘当前内容）
    pub fn discard_pending(&mut self) {
        remove_pending_files(&self.app_dir);
        self.recoverable = None;
    }

    /// 放弃内存变更，从磁盘重读；有未保存变更时拒绝
    pub fn reload_from_disk(&mut self) -> Result<(), StoreError> {
        if self.pending {
            return Err(StoreError::Biz(
                "存在未保存的变更，不能重载（请先关闭表格并重试保存）".into(),
            ));
        }
        self.reload_file().map_err(StoreError::Sys)
    }

    /// 用磁盘当前内容重建内存（恢复版本后调用）
    pub fn reload_file(&mut self) -> Result<(), String> {
        let bytes = std::fs::read(&self.path).map_err(|e| format!("OSError: {}", e))?;
        let entries = read_entries(&bytes)?;
        let paths = sheet_paths(&entries)?;
        let sheet_archive = paths
            .get(crate::SHEET_ARCHIVE)
            .cloned()
            .ok_or_else(|| format!("该表格缺少工作表：{}", crate::SHEET_ARCHIVE))?;
        let sheet_class = paths
            .get(crate::SHEET_RECORDS)
            .cloned()
            .ok_or_else(|| format!("该表格缺少工作表：{}", crate::SHEET_RECORDS))?;
        let (members, classes, refund_header_present) = read_raw(&bytes)?;
        let arch_xml = find_entry_bytes(&entries, &sheet_archive)?;
        let cls_xml = find_entry_bytes(&entries, &sheet_class)?;
        self.tpl_archive = row_templates(arch_xml, 2);
        self.tpl_class = row_templates(cls_xml, 2);
        self.sheet_archive = sheet_archive;
        self.sheet_class = sheet_class;
        self.doc_bytes = finalize_and_write(&entries)?;
        self.entries = entries;
        self.members = members;
        self.classes = classes;
        self.refund_header_present = refund_header_present;
        self.pending = false;
        self.pending_reason = None;
        self.last_saved = None;
        Ok(())
    }

    pub fn restore_version(&mut self, file: &str) -> Result<(), StoreError> {
        let safe = file
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        if !safe.ends_with(".xlsx") {
            return Err(StoreError::Biz(format!("版本文件不存在：{}", safe)));
        }
        let src = snapshot::versions_dir(&self.app_dir).join(&safe);
        if !src.is_file() {
            return Err(StoreError::Biz(format!("版本文件不存在：{}", safe)));
        }
        snapshot::snapshot(
            &self.path,
            &self.app_dir,
            "恢复前自动备份",
            &format!("恢复 {} 前的当前状态", safe),
            None,
        );
        std::fs::copy(&src, &self.path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    StoreError::Biz("表格文件被占用，请先关闭 Excel/WPS 再恢复".into())
                } else {
                    StoreError::Sys(format!("OSError: {}", e))
                }
            })?;
        self.reload_file().map_err(StoreError::Sys)
    }

    pub fn versions_json(&self) -> Vec<Value> {
        snapshot::list(&self.app_dir)
    }
}

fn find_entry_bytes<'a>(entries: &'a [Entry], name: &str) -> Result<&'a [u8], String> {
    entries
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.data.as_slice())
        .ok_or_else(|| format!("missing part {}", name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("lfy_store_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("test.xlsx");
        std::fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.xlsx"),
            &p,
        )
        .unwrap();
        Store::load(p, dir).unwrap()
    }

    #[test]
    fn first_empty_row_skips_used() {
        assert_eq!(Store::first_empty_row(&[2, 3, 5], 10), Some(4));
        assert_eq!(Store::first_empty_row(&[], 10), Some(2));
        assert_eq!(Store::first_empty_row(&[2, 3, 4], 4), None);
    }

    /// 挂起变更全链路：内存变更 → 写挂起工件 → 重新加载出现恢复标记 → 恢复落盘
    #[test]
    fn pending_recover_roundtrip() {
        let mut st = temp_store("recover");
        let n0 = st.members.len();
        let mid = st
            .add_member(&json!({
                "name": "挂起员", "phone": "13800000099", "card_type": "次卡",
                "total_sessions": "5", "receivable": "500"
            }))
            .unwrap();
        // 模拟保存失败：变更只存在于内存 doc_bytes，磁盘未动
        st.write_pending_artifacts("新增会员", "测试挂起恢复");
        let (path, dir) = (st.path.clone(), st.app_dir.clone());
        drop(st);

        // 重新加载：磁盘仍是旧状态，但应检测到挂起变更
        let mut st2 = Store::load(path.clone(), dir.clone()).unwrap();
        assert!(st2.recoverable.is_some(), "应检测到挂起变更");
        assert_eq!(st2.meta()["pending_recovery"]["summary"], "测试挂起恢复");
        assert_eq!(st2.members.len(), n0);

        // 恢复：挂起内容覆盖表格并重载
        st2.recover_pending().unwrap();
        assert!(st2.members.iter().any(|m| m.id == mid));
        assert!(st2.recoverable.is_none());

        // 磁盘文件已包含变更，工件已清理
        let st3 = Store::load(path, dir).unwrap();
        assert!(st3.members.iter().any(|m| m.id == mid));
        assert!(!pending_bytes_path(&st3.app_dir).is_file());
        assert!(!pending_meta_path(&st3.app_dir).is_file());
    }

    #[test]
    fn pending_discard() {
        let mut st = temp_store("discard");
        st.write_pending_artifacts("手动", "x");
        let (path, dir) = (st.path.clone(), st.app_dir.clone());
        drop(st);
        let mut st2 = Store::load(path, dir).unwrap();
        assert!(st2.recoverable.is_some());
        st2.discard_pending();
        assert!(st2.recoverable.is_none());
        assert!(!pending_bytes_path(&st2.app_dir).is_file());
        assert!(!pending_meta_path(&st2.app_dir).is_file());
    }

    /// 挂起工件目标与当前表格不一致时不提示恢复（换表/重命名场景）
    #[test]
    fn pending_target_mismatch_not_recoverable() {
        let mut st = temp_store("mismatch");
        st.write_pending_artifacts("编辑会员", "x");
        // 把工件目标改成另一个文件名
        let meta_path = pending_meta_path(&st.app_dir);
        let mut meta: Value =
            serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        meta["target"] = json!("别的表格.xlsx");
        std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let (path, dir) = (st.path.clone(), st.app_dir.clone());
        drop(st);
        let st2 = Store::load(path, dir).unwrap();
        assert!(st2.recoverable.is_none(), "目标不一致不应提示恢复");
    }
}
