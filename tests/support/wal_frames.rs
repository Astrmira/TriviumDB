#[path = "facts.rs"]
mod facts;

pub fn frames(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
    assert_eq!(&bytes[..4], b"TVWL");
    let mut offset = 6;
    let mut result = Vec::new();
    while offset < bytes.len() {
        let size = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        assert!(size > 0 && size + 8 <= bytes.len() - offset);
        result.push(offset..offset + size + 8);
        offset += size + 8;
    }
    result
}

/// 完整坏帧须在解析和数据库入口均被结构化拒绝，留下原始日志供诊断。
pub fn assert_corrupt(path: &str, dim: usize, bytes: &[u8], offset: usize) {
    let parsed = triviumdb::storage::wal::Wal::read_entries::<f32>(path).unwrap_err();
    assert_eq!(
        parsed.code(),
        "TDB_CORRUPTED_FILE",
        "偏移 {offset}: {parsed}"
    );
    let opened = triviumdb::Database::<f32>::open(path, dim)
        .err()
        .expect("完整坏帧被接受");
    assert_eq!(
        opened.code(),
        "TDB_CORRUPTED_FILE",
        "偏移 {offset}: {opened}"
    );
    assert_eq!(
        std::fs::read(format!("{path}.wal")).unwrap(),
        bytes,
        "偏移 {offset}"
    );
}

/// 首次回放与正常关闭后的重开都须给出同一组已提交事实。
pub fn assert_recovered(path: &str, dim: usize, expected: &serde_json::Value) {
    for _ in 0..2 {
        let mut db = triviumdb::Database::<f32>::open(path, dim).unwrap();
        assert_eq!(&facts::nodes(&db), expected);
        db.close().unwrap();
    }
}
