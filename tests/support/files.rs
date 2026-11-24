//! 在由测试独占的平面制品目录中，按真实文件名和完整字节采集状态。
use std::{collections::BTreeMap, ffi::OsString, fs, path::Path};

pub fn files(path: &Path) -> BTreeMap<OsString, Vec<u8>> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            assert!(
                entry.file_type().unwrap().is_file(),
                "意外的非普通制品：{:?}",
                entry.path()
            );
            (entry.file_name(), fs::read(entry.path()).unwrap())
        })
        .collect()
}
