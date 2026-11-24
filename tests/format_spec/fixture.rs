use super::spec::FileRole;
use triviumdb::database::Database;

pub const DIM: usize = 4;
pub fn path(name: &str) -> String {
    thread_local! { static ROOT: crate::support::TestDir = crate::support::TestDir::new("format-spec"); }
    ROOT.with(|dir| dir.file(name))
}

fn payload_generation_path(path: &str) -> Option<String> {
    let marker = crate::artifact::read(format!("{path}.flush_ok")).ok()?;
    let generation = u64::from_le_bytes(marker.get(5..13)?.try_into().ok()?);
    Some(format!("{path}.pld.{generation}"))
}

pub fn cleanup(path: &str) {
    crate::cleanup_files::database(path);
}

pub fn seed(name: &str) -> String {
    let path = path(name);
    cleanup(&path);
    let mut database = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=8 {
        database
            .insert_with_id(
                id,
                &[id as f32, 1.0, 0.0, -1.0],
                serde_json::json!({"kind": if id % 2 == 0 { "even" } else { "odd" }, "rank": id}),
            )
            .unwrap();
    }
    database
        .upsert_edge(1, 2, "next", 1.0, serde_json::json!({"index": 1}))
        .unwrap();
    database
        .upsert_edge(2, 3, "next", 0.5, serde_json::json!({"index": 2}))
        .unwrap();
    database.create_index("kind").unwrap();
    database.create_ordered_index("rank").unwrap();
    database.flush().unwrap();
    drop(database);
    path
}

pub fn copy_roles(source: &str, target: &str, roles: &[FileRole]) {
    cleanup(target);
    for role in roles {
        if *role == FileRole::Payload {
            if let Some(source_file) = payload_generation_path(source) {
                let marker = crate::artifact::read(format!("{source}.flush_ok")).unwrap();
                let generation = u64::from_le_bytes(marker[5..13].try_into().unwrap());
                crate::artifact::copy(source_file, format!("{target}.pld.{generation}")).unwrap();
            }
            continue;
        }
        let source_file = format!("{source}{}", role.suffix());
        if std::path::Path::new(&source_file).exists() {
            crate::artifact::copy(source_file, format!("{target}{}", role.suffix())).unwrap();
        }
    }
    {
        std::fs::copy(format!("{source}.lock"), format!("{target}.lock")).unwrap();
        std::fs::copy(format!("{source}.idx"), format!("{target}.idx")).unwrap();
        if let Some(source_file) = payload_generation_path(source) {
            let marker = std::fs::read(format!("{source}.flush_ok")).unwrap();
            let generation = u64::from_le_bytes(marker[5..13].try_into().unwrap());
            std::fs::copy(source_file, format!("{target}.pld.{generation}")).unwrap();
        }
    }
}

pub fn with_read_only(path: &str, verify: impl FnOnce(triviumdb::Result<Database<f32>>)) {
    crate::read_only::check(path, DIM, verify);
}

pub fn assert_corrupt(result: triviumdb::Result<Database<f32>>) {
    let err = result.err().expect("损坏的制品必须被拒绝");
    assert_eq!(err.code(), "TDB_CORRUPTED_FILE", "{err}");
}

pub fn assert_seed(db: &Database<f32>) {
    let expected: Vec<_> = (1..=8).map(|id| {
        let edges = match id {
            1 => serde_json::json!([{"target":2, "label":"next", "weight":1.0,"metadata":{"index":1}}]),
            2 => serde_json::json!([{"target":3, "label":"next", "weight":0.5,"metadata":{"index":2}}]),
            _ => serde_json::json!([]),
        };
        serde_json::json!({"id":id, "vector":[id as f32,1.,0.,-1.], "payload":{"kind":if id%2==0 {"even"} else {"odd"},"rank":id}, "edges":edges})
    }).collect();
    assert_eq!(crate::node_facts::nodes(db), serde_json::json!(expected));
}

#[test]
fn 未登记后缀的意外文件进入物理快照() {
    let root = crate::support::TestDir::new("unknown-artifact");
    let before = crate::physical_files::files(&root.0);
    std::fs::write(root.file("data.tdb.unexpected"), b"extra").unwrap();
    let after = crate::physical_files::files(&root.0);
    assert!(before.is_empty());
    assert_eq!(after.len(), 1);
    assert_eq!(after[std::ffi::OsStr::new("data.tdb.unexpected")], b"extra");
}
