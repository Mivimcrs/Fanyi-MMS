use std::io::{Read, Write};

/// xlsx（zip）内的一个条目
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

/// 读取 zip 全部条目（保持原顺序）
pub fn read_entries(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut ar =
        zip::ZipArchive::new(cursor).map_err(|e| format!("BadZipFile: {}", e))?;
    let mut out = Vec::with_capacity(ar.len());
    for i in 0..ar.len() {
        let mut f = ar.by_index(i).map_err(|e| format!("BadZipFile: {}", e))?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        let mut data = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut data)
            .map_err(|e| format!("read entry failed: {}", e))?;
        out.push(Entry { name, data });
    }
    Ok(out)
}

/// 重新打包（其余条目内容原样保留，仅重新压缩）
pub fn write_entries(entries: &[Entry]) -> Result<Vec<u8>, String> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zw = zip::ZipWriter::new(buf);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for e in entries {
        zw.start_file(e.name.as_str(), opts)
            .map_err(|e| format!("zip start_file failed: {}", e))?;
        zw.write_all(&e.data)
            .map_err(|e| format!("zip write failed: {}", e))?;
    }
    zw.finish()
        .map_err(|e| format!("zip finish failed: {}", e))
        .map(|c| c.into_inner())
}

/// 删除部件（calcChain），并同步清理 [Content_Types].xml 与 workbook rels
fn remove_part(entries: &mut Vec<Entry>, part: &str) {
    if let Some(pos) = entries.iter().position(|e| e.name == part) {
        entries.remove(pos);
        // 1) [Content_Types].xml 中的 Override
        if let Some(ct) = entries.iter_mut().find(|e| e.name == "[Content_Types].xml") {
            let text = String::from_utf8_lossy(&ct.data).into_owned();
            let needle = format!(r#"<Override PartName="/{}""#, part);
            if let Some(s) = text.find(&needle) {
                if let Some(rel) = text[s..].find("/>") {
                    let mut t = String::from(&text[..s]);
                    t.push_str(&text[s + rel + 2..]);
                    ct.data = t.into_bytes();
                }
            }
        }
        // 2) workbook rels 中的 Relationship（Target 指向该部件）
        let base = part.rsplit('/').next().unwrap_or(part);
        if let Some(rels) = entries.iter_mut().find(|e| e.name == "xl/_rels/workbook.xml.rels") {
            let text = String::from_utf8_lossy(&rels.data).into_owned();
            let mut out = String::new();
            let mut rest = text.as_str();
            while let Some(p) = rest.find("<Relationship ") {
                let after = &rest[p..];
                let end = after.find("</Relationship>").map(|x| x + 15).unwrap_or_else(|| {
                    after.find("/>").map(|x| x + 2).unwrap_or(after.len())
                });
                let seg = &after[..end];
                let keep = !seg.contains(&format!("Target=\"{}", base));
                if keep {
                    out.push_str(&rest[..p + end]);
                }
                rest = &after[end..];
            }
            out.push_str(rest);
            rels.data = out.into_bytes();
        }
    }
}

/// 在 workbook.xml 的 calcPr 上强制打开时全量重算（保证 TODAY() 相关列刷新）
fn force_full_calc(entries: &mut Vec<Entry>) {
    if let Some(wb) = entries.iter_mut().find(|e| e.name == "xl/workbook.xml") {
        let text = String::from_utf8_lossy(&wb.data).into_owned();
        if let Some(p) = text.find("<calcPr") {
            if !text.contains("fullCalcOnLoad") {
                // 在 calcPr 标签的 '>' 前插入属性
                if let Some(gt) = text[p..].find('>') {
                    let mut t = String::from(&text[..p + gt]);
                    t.push_str(" fullCalcOnLoad=\"1\"");
                    t.push_str(&text[p + gt..]);
                    wb.data = t.into_bytes();
                }
            }
        } else if let Some(p) = text.find("</workbook>") {
            let mut t = String::from(&text[..p]);
            t.push_str(r#"<calcPr calcId="140000" fullCalcOnLoad="1"/>"#);
            t.push_str(&text[p..]);
            wb.data = t.into_bytes();
        }
    }
}

/// 保存前的最终组装：删 calcChain（Excel 自动重建）+ 强制重算 + 打包
pub fn finalize_and_write(entries: &[Entry]) -> Result<Vec<u8>, String> {
    let mut list: Vec<Entry> = entries
        .iter()
        .filter(|e| e.name != "xl/calcChain.xml")
        .map(|e| Entry { name: e.name.clone(), data: e.data.clone() })
        .collect();
    remove_part(&mut list, "xl/calcChain.xml");
    force_full_calc(&mut list);
    write_entries(&list)
}
