#![allow(non_snake_case)]
//! GJB-5000B EMI 增强比特翻转测试
//!
//! 补充 hw_intrusion.rs 中已有的单点翻转覆盖，模拟军舰环境下
//! 强电磁脉冲 (EMP) 导致的突发性多比特错误 (burst error)。

use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("emi_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    crate::cleanup_files::database(path);
}

fn seed_db(path: &str, count: usize) {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    for i in 0..count {
        db.insert(
            &[i as f32, (i as f32).sin(), (i as f32).cos(), 1.0],
            serde_json::json!({"idx": i}),
        )
        .unwrap();
    }
    db.flush().unwrap();
}

fn assert_payloads_from_seed(db: &Database<f32>, max_count: usize) {
    for id in db.all_node_ids().unwrap() {
        let payload = db.get_payload(id).unwrap().expect("恢复节点必须有 payload");
        let idx = payload
            .get("idx")
            .and_then(|v| v.as_u64())
            .expect("恢复节点必须携带原始 idx");
        assert!(
            idx < max_count as u64,
            "恢复不能产生越界脏 payload: {payload}"
        );
    }
}

fn assert_reject_has_diagnostic<E: std::fmt::Display>(err: E) {
    assert!(!err.to_string().is_empty(), "安全拒绝必须返回可诊断错误");
}

// ════════════════════════════════════════════════════════════════
//  1. 连续 8 字节全翻转 — 突发错误 (Burst Error)
// ════════════════════════════════════════════════════════════════

/// 模拟强 EMI 脉冲导致的突发错误：连续 8 字节全部取反（不是单 bit）。
/// 在文件的 header、payload、vector 三个区域各翻转一段。
#[test]
fn EMI_01_连续8字节全翻转_burst_error() {
    let path = tmp_db("burst");
    cleanup(&path);
    seed_db(&path, 30);

    if let Ok(mut data) = std::fs::read(&path) {
        let len = data.len();
        // 在三个区域各翻转连续 8 字节
        let targets = [
            16.min(len.saturating_sub(8)), // header 区域（跳过 magic）
            len / 3,                       // payload 区域
            len * 2 / 3,                   // 文件后半部分
        ];
        for &off in &targets {
            if off + 8 <= len {
                for byte in data.iter_mut().skip(off).take(8) {
                    *byte = !*byte; // 全部取反
                }
            }
        }
        std::fs::write(&path, &data).unwrap();
        std::fs::remove_file(format!("{}.flush_ok", path)).ok();
    }

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "Burst error 不应导致 panic");

    match result.unwrap() {
        Ok(db) => {
            assert!(
                db.node_count().unwrap() <= 30,
                "Burst error 降级不能产生额外节点"
            );
            assert_payloads_from_seed(&db, 30);
            let hits = db.search(&[1.0, 0.0, 0.0, 0.0], 5, 0, 0.0).unwrap();
            assert!(
                hits.len() <= db.node_count().unwrap(),
                "搜索结果不能超过存活节点数"
            );
        }
        Err(e) => assert_reject_has_diagnostic(e),
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  2. 大面积随机比特翻转 — 搜索降级但不崩溃
// ════════════════════════════════════════════════════════════════

/// 向量数据区随机 10% 比特翻转，模拟大面积 EMI 损坏。
/// 验证：引擎优雅降级——搜索可以返回结果（可能不准确），但绝不 panic/segfault。
#[test]
fn EMI_02_向量数据区10_percent翻转_搜索降级不崩溃() {
    let path = tmp_db("mass_flip");
    cleanup(&path);
    seed_db(&path, 50);

    let vec_path = format!("{}.vec", path);
    if let Ok(mut data) = std::fs::read(&vec_path) {
        let len = data.len();
        let flip_count = len / 10; // 10% 的字节
        // 使用确定性伪随机序列
        for i in 0..flip_count {
            let offset = (i * 137 + 42) % len;
            let bit = (i * 31) % 8;
            data[offset] ^= 1u8 << bit;
        }
        std::fs::write(&vec_path, &data).unwrap();
        std::fs::remove_file(format!("{}.flush_ok", path)).ok();
    }

    // 重新打开
    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "大面积比特翻转不应导致 panic");

    if let Ok(Ok(db)) = result {
        assert!(
            db.node_count().unwrap() <= 50,
            "大面积翻转降级不能产生额外节点"
        );
        assert_payloads_from_seed(&db, 50);
        let search_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.search(&[1.0, 0.0, 0.0, 0.0], 10, 0, 0.0)
        }));
        assert!(search_result.is_ok(), "大面积损坏后搜索不应 panic");
        let hits = search_result.unwrap().unwrap();
        assert!(hits.len() <= 10, "搜索必须遵守 top_k 上限");
        assert!(
            hits.len() <= db.node_count().unwrap(),
            "搜索结果不能超过存活节点数"
        );
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  3. WAL 完整帧的单比特翻转
// ════════════════════════════════════════════════════════════════

#[test]
fn EMI_03_WAL完整帧单比特翻转逐项拒绝() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("crc.tdb");
    let path = file.to_str().unwrap();
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    for i in 0..20 {
        db.insert(&[i as f32, 0., 0., 0.], serde_json::json!({"idx":i}))
            .unwrap();
        if i == 9 {
            db.flush().unwrap();
        }
    }
    drop(db);
    let wal = format!("{path}.wal");
    let original = std::fs::read(&wal).unwrap();
    let frames = crate::wal_frames::frames(&original);
    assert_eq!(frames.len(), 11, "一个检查点封条与十条已确认写入");
    let mut corrupt = original.clone();
    // 长度和物理截断由格式规格覆盖。完整帧的正文和 CRC 逐位核对。
    for frame in frames {
        for offset in frame.start + 4..frame.end {
            for bit in 0..8 {
                corrupt[offset] ^= 1 << bit;
                std::fs::write(&wal, &corrupt).unwrap();
                crate::wal_frames::assert_corrupt(path, DIM, &corrupt, offset);
                corrupt[offset] = original[offset];
            }
        }
    }
    std::fs::write(&wal, original).unwrap();
    let expected: Vec<_> = (0..20)
        .map(|i| {
            serde_json::json!({
                "id":i+1, "vector":[i as f32,0.0,0.0,0.0], "payload":{"idx":i}, "edges":[]
            })
        })
        .collect();
    crate::wal_frames::assert_recovered(path, DIM, &serde_json::json!(expected));
}

// ════════════════════════════════════════════════════════════════
//  4. .tdb 文件全零覆写
// ════════════════════════════════════════════════════════════════

/// 将 .tdb 文件的所有内容设为 0x00（模拟 Flash 擦除未写入）
#[test]
fn EMI_04_TDB全零覆写_安全拒绝() {
    let path = tmp_db("all_zero");
    cleanup(&path);
    seed_db(&path, 20);

    if let Ok(data) = std::fs::read(&path) {
        let zeroed = vec![0u8; data.len()];
        std::fs::write(&path, &zeroed).unwrap();
        std::fs::remove_file(format!("{}.flush_ok", path)).ok();
    }

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "全零 .tdb 不应 panic");
    let opened = result.unwrap();
    assert!(opened.is_err(), "全零 .tdb 应被拒绝加载");
    let err = opened.err().unwrap();
    assert_reject_has_diagnostic(err);

    cleanup(&path);
    let clean = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        clean.node_count().unwrap(),
        0,
        "拒绝全零库后同路径重建不能携带脏状态"
    );

    cleanup(&path);
}
