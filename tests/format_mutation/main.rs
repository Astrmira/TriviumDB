#![allow(non_snake_case)]
//! 格式感知变异矩阵：小型 fixture、逐字段变异、只读零写与 fail-closed。

use std::io::{Read, Seek, SeekFrom, Write};
use triviumdb::database::Database;

#[derive(Debug, Clone)]
enum Mutation {
    TruncateAt(usize),
    FlipBit { offset: usize, bit: u8 },
    Overwrite { offset: usize, bytes: Vec<u8> },
    Append(Vec<u8>),
}

impl Mutation {
    fn apply(&self, bytes: &[u8]) -> Vec<u8> {
        let mut output = bytes.to_vec();
        match self {
            Self::TruncateAt(offset) => output.truncate((*offset).min(output.len())),
            Self::FlipBit { offset, bit } => {
                if let Some(byte) = output.get_mut(*offset) {
                    *byte ^= 1u8 << (bit % 8);
                }
            }
            Self::Overwrite { offset, bytes } => {
                if *offset < output.len() {
                    let end = offset.saturating_add(bytes.len()).min(output.len());
                    output[*offset..end].copy_from_slice(&bytes[..end - offset]);
                }
            }
            Self::Append(bytes) => output.extend_from_slice(bytes),
        }
        output
    }
}

fn path(name: &str) -> String {
    thread_local! { static ROOT: support::TestDir = support::TestDir::new("format-mutation"); }
    ROOT.with(|dir| dir.file(&format!("{name}.tdb")))
}

fn cleanup(path: &str) {
    crate::cleanup_files::database(path);
}

fn fixture(name: &str) -> String {
    let path = path(name);
    cleanup(&path);
    let mut database = Database::<f32>::open(&path, 4).unwrap();
    for index in 0..12u64 {
        database
            .insert_with_id(
                index + 1,
                &[index as f32, 1.0, 0.0, 0.0],
                serde_json::json!({"kind": "fixture", "index": index}),
            )
            .unwrap();
    }
    database.flush().unwrap();
    drop(database);
    path
}

fn copy_group(source: &str, destination: &str) {
    let path = std::path::Path::new(source);
    let name = path.file_name().unwrap().to_str().unwrap();
    for entry in std::fs::read_dir(path.parent().unwrap()).unwrap() {
        let entry = entry.unwrap();
        let file = entry.file_name();
        if let Some(suffix) = file.to_str().unwrap().strip_prefix(name)
            && (suffix.is_empty() || suffix.starts_with('.'))
        {
            std::fs::copy(entry.path(), format!("{destination}{suffix}")).unwrap();
        }
    }
}

fn verify_control(source: &str) {
    let control = format!("{source}_control");
    cleanup(&control);
    copy_group(source, &control);
    read_only::check(&control, 4, |result| {
        let mut db = result.unwrap();
        check_contents(&db);
        db.close().unwrap();
    });
    cleanup(&control);
}

fn check_contents(db: &Database<f32>) {
    let expected: Vec<_> = (1..=12).map(|id| serde_json::json!({
        "id":id, "vector":[(id-1) as f32,1.,0.,0.], "payload":{"kind":"fixture","index":id-1}, "edges":[]
    })).collect();
    assert_eq!(node_facts::nodes(db), serde_json::json!(expected));
}

fn mutations(length: usize) -> Vec<Mutation> {
    let mut output = vec![
        Mutation::TruncateAt(0),
        Mutation::TruncateAt(1),
        Mutation::TruncateAt(length / 2),
        Mutation::TruncateAt(length.saturating_sub(1)),
        Mutation::Append(vec![0xA5; 17]),
    ];
    for offset in [
        0,
        1,
        3,
        4,
        7,
        8,
        15,
        31,
        47,
        57,
        length / 2,
        length.saturating_sub(1),
    ] {
        for bit in [0, 3, 7] {
            output.push(Mutation::FlipBit { offset, bit });
        }
    }
    for offset in [0, 4, 8, 16, 24, 32, 48] {
        for bytes in [vec![0; 8], vec![0xFF; 8], u64::MAX.to_le_bytes().to_vec()] {
            output.push(Mutation::Overwrite { offset, bytes });
        }
    }
    output
}

#[test]
fn 主文件内容改变时拒绝打开并保持文件组原样() {
    let original_path = fixture("tdb");
    verify_control(&original_path);
    let original = std::fs::read(&original_path).unwrap();
    for (index, mutation) in mutations(original.len()).into_iter().enumerate() {
        let candidate = path(&format!("tdb_case_{index}"));
        cleanup(&candidate);
        copy_group(&original_path, &candidate);
        let changed = mutation.apply(&original);
        std::fs::write(&candidate, &changed).unwrap();
        read_only::check(&candidate, 4, |result| {
            if changed == original {
                check_contents(&result.unwrap());
            } else {
                let err = result.err().expect("已改变的主文件必须被检查点校验拒绝");
                assert!(
                    matches!(
                        err.code(),
                        "TDB_CORRUPTED_FILE"
                            | "TDB_MIGRATION_REQUIRED"
                            | "TDB_UNSUPPORTED_DATABASE_VERSION"
                            | "TDB_DIMENSION_MISMATCH"
                            | "TDB_DTYPE_MISMATCH"
                    ),
                    "{mutation:?}: {err}"
                );
            }
        });
        cleanup(&candidate);
    }
    cleanup(&original_path);
}

#[test]
fn flush_marker和vec等长变异由完整crc拒绝且只读零写() {
    let original_path = fixture("cross_file");
    verify_control(&original_path);
    for suffix in [".vec", ".flush_ok"] {
        let source = format!("{original_path}{suffix}");
        let original = std::fs::read(&source).unwrap();
        for offset in [0, original.len() / 2, original.len().saturating_sub(1)] {
            for bit in [0, 7] {
                let candidate = path(&format!("cross_{}_{}_{}", &suffix[1..], offset, bit));
                cleanup(&candidate);
                copy_group(&original_path, &candidate);
                let mutated = Mutation::FlipBit { offset, bit }.apply(&original);
                std::fs::write(format!("{candidate}{suffix}"), mutated).unwrap();
                read_only::check(&candidate, 4, |result| {
                    let error = result.err().expect("校验和不同的文件组必须拒绝");
                    assert_eq!(error.code(), "TDB_CORRUPTED_FILE");
                });
                cleanup(&candidate);
            }
        }
    }
    cleanup(&original_path);
}

#[test]
fn 文件读取工具自身覆盖偏移边界且不产生危险分配() {
    let path = path("io_helper");
    cleanup(&path);
    std::fs::write(&path, (0u8..64).collect::<Vec<_>>()).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.seek(SeekFrom::Start(16)).unwrap();
    file.write_all(&[1, 2, 3, 4]).unwrap();
    file.seek(SeekFrom::Start(16)).unwrap();
    let mut bytes = [0u8; 4];
    file.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes, [1, 2, 3, 4]);
    cleanup(&path);
}

#[path = "../support/cleanup.rs"]
mod cleanup_files;

#[path = "../support/files.rs"]
mod physical_files;
#[path = "../support/read_only.rs"]
mod read_only;
#[path = "../support/mod.rs"]
mod support;

#[path = "../support/facts.rs"]
mod node_facts;
