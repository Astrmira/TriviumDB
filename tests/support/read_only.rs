//! 从首次只读打开前到句柄释放后核对整个独占目录。
use triviumdb::Database;

pub fn check(path: &str, dim: usize, check: impl FnOnce(triviumdb::Result<Database<f32>>)) {
    let dir = std::path::Path::new(path).parent().unwrap();
    let before = crate::physical_files::files(dir);
    check(Database::<f32>::open_read_only(path, dim));
    let after = crate::physical_files::files(dir);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "{path}"
    );
    for (name, bytes) in before {
        assert!(
            bytes == after[&name],
            "只读操作改变了 {name:?} 的内容：{path}"
        );
    }
}
