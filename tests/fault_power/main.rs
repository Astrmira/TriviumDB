#![allow(non_snake_case)]
//! GJB-5000B 快速断电循环测试
//!
//! 模拟军舰电源不稳定环境下的频繁断电重启，验证 TriviumDB 的
//! WAL + flush_ok 原子提交机制在反复中断下的数据完整性。

use std::process::Command;
use triviumdb::database::Database;

#[cfg(feature = "test-hooks")]
mod text_publication;

#[cfg(feature = "test-hooks")]
mod migration_publication;
#[path = "../support/mod.rs"]
mod support;

#[cfg(feature = "test-hooks")]
use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    thread_local! { static ROOT: support::TestDir = support::TestDir::new("power"); }
    ROOT.with(|dir| dir.file(name))
}

fn cleanup(path: &str) {
    crate::cleanup_files::database(path);
}

fn child_command() -> Command {
    let executable = std::env::current_exe().unwrap();
    if std::env::var_os("TRIVIUM_TEST_QEMU_AARCH64").is_some() {
        let mut command = Command::new("qemu-aarch64");
        command
            .arg("-L")
            .arg("/usr/aarch64-linux-gnu")
            .arg(executable);
        command
    } else {
        Command::new(executable)
    }
}

#[test]
fn 同一实例限额下恢复已确认的大字符串() {
    use triviumdb::database::StorageMode;
    let config = triviumdb::database::Config {
        dim: 2,
        memory_limit: 4 << 20,
        payload_cache_bytes: 0,
        sync_mode: triviumdb::storage::wal::SyncMode::Full,
        ..Default::default()
    };
    let payload = serde_json::json!("甲".repeat(87_000));
    if let Some(path) = std::env::var_os("TRIVIUM_STRING_RECOVERY_PATH") {
        let storage_mode = match std::env::var("TRIVIUM_STRING_RECOVERY_MODE")
            .unwrap()
            .as_str()
        {
            "Rom" => StorageMode::Rom,
            "Mmap" => StorageMode::Mmap,
            mode => panic!("未知存储模式：{mode}"),
        };
        let mut db = Database::<f32>::open_with_config(
            path.to_str().unwrap(),
            triviumdb::database::Config {
                storage_mode,
                ..config
            },
        )
        .unwrap();
        db.insert_with_id(4, &[1., 0.], payload).unwrap();
        std::process::exit(0);
    }
    let directory = tempfile::tempdir().unwrap();
    for storage_mode in [StorageMode::Rom, StorageMode::Mmap] {
        let mode = format!("{storage_mode:?}");
        let path = directory.path().join(format!("large-string-{mode}.tdb"));
        let child = child_command()
            .args(["--exact", "同一实例限额下恢复已确认的大字符串"])
            .env("TRIVIUM_STRING_RECOVERY_PATH", &path)
            .env("TRIVIUM_STRING_RECOVERY_MODE", &mode)
            .output()
            .unwrap();
        assert!(child.status.success(), "{mode}: {child:?}");
        for phase in ["WAL", "checkpoint"] {
            let mut db = Database::<f32>::open_with_config(
                path.to_str().unwrap(),
                triviumdb::database::Config {
                    storage_mode,
                    ..config
                },
            )
            .unwrap_or_else(|error| panic!("{mode} {phase}: {error}"));
            let node = db.get(4).unwrap().unwrap();
            assert_eq!(node.vector, [1., 0.]);
            assert_eq!(node.payload, payload);
            db.close().unwrap();
        }
    }
}

#[test]
fn Pelorus曲率更新可随已确认向量写入恢复() {
    use triviumdb::database::{Config, StorageMode};
    use triviumdb::index::pelorus::{PelorusConfig, PelorusSearchOptions};
    use triviumdb::storage::wal::SyncMode;

    let config = |mode: &str| Config {
        dim: 2,
        storage_mode: if mode == "mmap" {
            StorageMode::Mmap
        } else {
            StorageMode::Rom
        },
        sync_mode: SyncMode::Full,
        auto_build_quiver: false,
        ..Default::default()
    };
    if let Some(path) = std::env::var_os("TRIVIUM_PELORUS_RECOVERY_PATH") {
        let mode = std::env::var("TRIVIUM_PELORUS_RECOVERY_MODE").unwrap();
        let mut db =
            Database::<f32>::open_with_config(path.to_str().unwrap(), config(&mode)).unwrap();
        db.insert_with_id(1, &[0.25, 0.0], serde_json::json!({"value":1}))
            .unwrap();
        db.build_pelorus_index(PelorusConfig {
            curvature: 4.0,
            ..Default::default()
        })
        .unwrap();
        db.build_pelorus_index(PelorusConfig::default()).unwrap();
        db.insert_with_id(2, &[0.75, 0.0], serde_json::json!({"value":2}))
            .unwrap();
        // 已确认写入直接退出，跳过数据库关闭和析构。
        std::process::exit(0);
    }
    let directory = tempfile::tempdir().unwrap();
    for mode in ["rom", "mmap"] {
        let path = directory.path().join(format!("{mode}.tdb"));
        let path = path.to_str().unwrap();
        let child = child_command()
            .args(["--exact", "Pelorus曲率更新可随已确认向量写入恢复"])
            .env("TRIVIUM_PELORUS_RECOVERY_PATH", path)
            .env("TRIVIUM_PELORUS_RECOVERY_MODE", mode)
            .output()
            .unwrap();
        assert!(child.status.success(), "{mode}: {child:?}");
        let mut db = Database::<f32>::open_with_config(path, config(mode)).unwrap();
        assert_eq!(
            db.get_payload(1).unwrap(),
            Some(serde_json::json!({"value":1}))
        );
        assert_eq!(
            db.get_payload(2).unwrap(),
            Some(serde_json::json!({"value":2}))
        );
        let hits = db
            .search_pelorus(&[0.75, 0.0], &PelorusSearchOptions::default())
            .unwrap();
        assert_eq!((hits[0].id, hits[0].distance), (2, 0.0));
        assert_eq!(hits[0].payload, serde_json::json!({"value":2}));
        db.close().unwrap();
        let reopened = Database::<f32>::open_with_config(path, config(mode)).unwrap();
        assert_eq!(
            reopened.get_payload(2).unwrap(),
            Some(serde_json::json!({"value":2}))
        );
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn Pelorus懒重建保留已发布曲率并恢复随后写入() {
    use triviumdb::database::{Config, MissingIndexPolicy, StorageMode};
    use triviumdb::index::pelorus::{PelorusConfig, PelorusSearchOptions};
    use triviumdb::sdk::RequestContext;
    use triviumdb::storage::wal::SyncMode;
    use triviumdb::test_hooks::pause_at_request;

    let config = |mode: &str| Config {
        dim: 2,
        storage_mode: if mode == "rom" {
            StorageMode::Rom
        } else {
            StorageMode::Mmap
        },
        sync_mode: SyncMode::Full,
        missing_index_policy: MissingIndexPolicy::BuildInMemory,
        auto_build_quiver: false,
        ..Default::default()
    };
    let tight = PelorusConfig {
        curvature: 4.0,
        ..Default::default()
    };
    if let Some(path) = std::env::var_os("TRIVIUM_LAZY_PELORUS_RECOVERY_PATH") {
        let mode = std::env::var("TRIVIUM_LAZY_PELORUS_RECOVERY_MODE").unwrap();
        let mut db =
            Database::<f32>::open_with_config(path.to_str().unwrap(), config(&mode)).unwrap();
        db.insert_with_id(1, &[0.125, 0.0], serde_json::json!(1))
            .unwrap();
        db.build_pelorus_index(PelorusConfig::default()).unwrap();
        // 超过变更节点上限，使原曲率配置保留且索引进入懒重建路径。
        for id in 2..=68 {
            db.insert_with_id(id, &[id as f32 / 1024.0, 0.0], serde_json::json!(id))
                .unwrap();
        }
        let before = db.estimated_memory();
        assert_eq!(
            db.search_pelorus(&[0.0], &Default::default())
                .unwrap_err()
                .code(),
            "TDB_DIMENSION_MISMATCH"
        );
        assert_eq!(
            db.search_pelorus_batch(&[vec![0.0, 0.0], vec![1.0, 1.0]], &Default::default())
                .unwrap_err()
                .code(),
            "TDB_INVALID_VECTOR"
        );
        assert_eq!(db.estimated_memory(), before);
        let context =
            RequestContext::with_timeout("pelorus-rebuild", std::time::Duration::ZERO, false)
                .unwrap();
        let gate = pause_at_request(
            ConcurrencyPoint::PelorusBeforeLazyRebuild,
            "pelorus-rebuild",
        );
        std::thread::scope(|threads| {
            let lazy = threads.spawn(|| {
                context.run(|| {
                    db.search_pelorus(
                        &[0.125, 0.0],
                        &PelorusSearchOptions {
                            top_k: 1,
                            ..Default::default()
                        },
                    )
                })
            });
            while !gate.is_arrived() {
                assert!(!lazy.is_finished(), "查询没有进入懒重建");
                std::thread::yield_now();
            }
            let published = db.build_pelorus_index(tight);
            gate.release();
            published.unwrap();
            let hits = lazy.join().unwrap().unwrap();
            assert_eq!((hits[0].id, hits[0].distance), (1, 0.0));
        });
        assert_eq!(
            db.update_vector(1, &[0.75, 0.0]).unwrap_err().code(),
            "TDB_INVALID_VECTOR"
        );
        db.update_vector(1, &[0.375, 0.0]).unwrap();
        // 仅依赖检查点和 WAL，跳过关闭与析构。
        std::process::exit(0);
    }
    let directory = tempfile::tempdir().unwrap();
    for mode in ["rom", "mmap"] {
        let path = directory.path().join(format!("{mode}.tdb"));
        let path = path.to_str().unwrap();
        let output = child_command()
            .args(["--exact", "Pelorus懒重建保留已发布曲率并恢复随后写入"])
            .env("TRIVIUM_LAZY_PELORUS_RECOVERY_PATH", path)
            .env("TRIVIUM_LAZY_PELORUS_RECOVERY_MODE", mode)
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        let mut db = Database::<f32>::open_with_config(path, config(mode)).unwrap();
        assert_eq!(
            db.update_vector(1, &[0.75, 0.0]).unwrap_err().code(),
            "TDB_INVALID_VECTOR"
        );
        let hits = db
            .search_pelorus(
                &[0.375, 0.0],
                &PelorusSearchOptions {
                    top_k: 1,
                    exact: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!((hits[0].id, hits[0].distance), (1, 0.0));
        assert_eq!(hits[0].payload, serde_json::json!(1));
        db.close().unwrap();
        let reopened = Database::<f32>::open_with_config(path, config(mode)).unwrap();
        assert_eq!(reopened.get(1).unwrap().unwrap().vector, [0.375, 0.0]);
    }
}

#[cfg(feature = "test-hooks")]
fn publication_point(name: &str) -> ConcurrencyPoint {
    match name {
        "after_backup" => ConcurrencyPoint::AfterPublicationPrepared,
        "after_vec" => ConcurrencyPoint::AfterVecPersisted,
        "after_tdb" => ConcurrencyPoint::AfterTdbPersisted,
        "before_marker" => ConcurrencyPoint::BeforeFlushMarkerRename,
        "after_marker" => ConcurrencyPoint::AfterFlushMarkerRename,
        "before_wal_replace" => ConcurrencyPoint::BeforeWalReplace,
        "during_restore" => ConcurrencyPoint::AfterPublicationRestore,
        other => panic!("未知发布阶段: {other}"),
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn __publication_power_loss_child_entry() {
    let Ok(path) = std::env::var("TRIVIUM_PUBLICATION_CHILD_PATH") else {
        return;
    };
    let phase = std::env::var("TRIVIUM_PUBLICATION_CHILD_PHASE").unwrap();
    let ready = std::env::var("TRIVIUM_PUBLICATION_CHILD_READY").unwrap();
    let case = std::env::var("TRIVIUM_PUBLICATION_CHILD_CASE").unwrap();
    let (dim, base) = publication_shape(&case);
    let waiter = pause_at(publication_point(&phase));
    let worker = std::thread::spawn(move || {
        let mut database = Database::<f32>::open_with_config(
            &path,
            triviumdb::database::Config {
                dim,
                sync_mode: triviumdb::storage::wal::SyncMode::Full,
                ..Default::default()
            },
        )
        .unwrap();
        if phase == "during_restore" {
            panic!("恢复断点未阻塞可写打开");
        }
        for id in base + 1..=base + 3 {
            database
                .insert_with_id(
                    id,
                    &publication_vector(id, dim),
                    serde_json::json!({"generation": "new", "id": id}),
                )
                .unwrap();
        }
        if case == "rewrite" {
            database.update_vector(1, &vec![-1.0; dim]).unwrap();
            database
                .update_payload(1, serde_json::json!({"generation": "new", "id": 1}))
                .unwrap();
            database.delete(2).unwrap();
            database.link(1, 3, "更新后的边", 2.0).unwrap();
            database.compact().unwrap();
        } else {
            database.flush().unwrap();
        }
    });
    waiter.wait_until_arrived();
    std::fs::write(&ready, "已到达断点".as_bytes()).unwrap();
    std::mem::forget(waiter);
    std::mem::forget(worker);
    loop {
        std::thread::park();
    }
}

#[cfg(feature = "test-hooks")]
fn publication_shape(case: &str) -> (usize, u64) {
    match case {
        "first" => (DIM, 0),
        "append" => (DIM, 4),
        "rewrite" => (65, 2048),
        _ => panic!("未知发布场景: {case}"),
    }
}

#[cfg(feature = "test-hooks")]
fn publication_vector(id: u64, dim: usize) -> Vec<f32> {
    (0..dim)
        .map(|axis| id as f32 + axis as f32 * 0.125)
        .collect()
}

#[cfg(feature = "test-hooks")]
fn assert_exact_recovered_data(database: &Database<f32>, case: &str) {
    let (dim, base) = publication_shape(case);
    let expected: Vec<u64> = (1..=base + 3)
        .filter(|id| case != "rewrite" || *id != 2)
        .collect();
    let mut actual = database.all_node_ids().unwrap();
    actual.sort_unstable();
    assert_eq!(
        actual, expected,
        "必须恢复所有已同步事务，不能用空库或旧代代替"
    );
    for id in expected {
        let node = database
            .get(id)
            .unwrap()
            .unwrap_or_else(|| panic!("恢复状态缺少节点 {id}"));
        assert_eq!(
            node.vector,
            if case == "rewrite" && id == 1 {
                vec![-1.0; dim]
            } else {
                publication_vector(id, dim)
            },
            "节点 {id} 的向量来自混合代际"
        );
        let payload = database.get_payload(id).unwrap().unwrap();
        assert_eq!(payload["id"], id);
        assert_eq!(
            payload["generation"],
            if id > base || (case == "rewrite" && id == 1) {
                "new"
            } else {
                "old"
            }
        );
    }
    if case == "rewrite" {
        let edges = database.get_edges(1).unwrap();
        assert!(
            edges.iter().any(|edge| edge.target_id == 3
                && edge.label == "更新后的边"
                && edge.weight == 2.0)
        );
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn PWR_00A_发布阶段强杀必须恢复全部已同步事务() {
    for case in ["first", "append", "rewrite"] {
        for phase in [
            "after_backup",
            "after_vec",
            "after_tdb",
            "before_marker",
            "after_marker",
            "before_wal_replace",
            "during_restore",
        ] {
            let path = tmp_db(&format!(
                "publication_{case}_{phase}_{}",
                std::process::id()
            ));
            let (dim, base) = publication_shape(case);
            cleanup(&path);
            // 先完成新库初始化，强杀屏障只观察已确认事务之后的检查点发布。
            {
                let mut database = Database::<f32>::open(&path, dim).unwrap();
                for id in 1..=base {
                    database
                        .insert_with_id(
                            id,
                            &publication_vector(id, dim),
                            serde_json::json!({"generation": "old", "id": id}),
                        )
                        .unwrap();
                }
                database.flush().unwrap();
            }

            kill_publication_child(
                &path,
                if phase == "during_restore" {
                    "after_tdb"
                } else {
                    phase
                },
                case,
            );
            if phase == "during_restore" {
                kill_publication_child(&path, phase, case);
            }
            let dir = std::path::Path::new(&path).parent().unwrap();
            let before = physical_files::files(dir);
            assert!(
                Database::<f32>::open_read_only(&path, dim).is_err(),
                "待恢复的 WAL/发布日志必须阻止只读打开"
            );
            assert!(Database::<f32>::open_immutable(&path, dim).is_err());
            assert_eq!(
                physical_files::files(dir),
                before,
                "只读与不可变打开失败不得改变任何文件"
            );

            let mut database = Database::<f32>::open(&path, dim).unwrap();
            assert_exact_recovered_data(&database, case);
            database.flush().unwrap();
            drop(database);
            let reader = Database::<f32>::open_read_only(&path, dim).unwrap();
            assert_exact_recovered_data(&reader, case);
            drop(reader);
            cleanup(&path);
        }
    }
}

#[cfg(feature = "test-hooks")]
fn kill_publication_child(path: &str, phase: &str, case: &str) {
    let ready = format!("{path}.{phase}.ready");
    let mut command = child_command();
    command
        .env("TRIVIUM_PUBLICATION_CHILD_PATH", path)
        .env("TRIVIUM_PUBLICATION_CHILD_PHASE", phase)
        .env("TRIVIUM_PUBLICATION_CHILD_READY", &ready)
        .env("TRIVIUM_PUBLICATION_CHILD_CASE", case)
        .arg("__publication_power_loss_child_entry");
    kill_at_barrier(command, &ready);
}

fn kill_at_barrier(mut command: Command, ready: &str) {
    let ready = std::path::Path::new(ready);
    assert!(!ready.exists(), "屏障文件必须为本次子进程独占：{ready:?}");
    let mut child = command.args(["--exact", "--nocapture"]).spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ready.exists() && std::time::Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("子进程在屏障前退出：{status}，{ready:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let arrived = ready.exists();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(arrived, "子进程未到达屏障：{ready:?}");
    std::fs::remove_file(ready).unwrap();
}

#[cfg(feature = "test-hooks")]
#[path = "../support/files.rs"]
mod physical_files;

#[test]
fn __group_commit_power_loss_child_entry() {
    let Ok(path) = std::env::var("TRIVIUM_GROUP_COMMIT_CHILD") else {
        return;
    };
    let ready = format!("{path}.ready");
    let mut database = Database::<f32>::open_with_config(
        &path,
        triviumdb::database::Config {
            dim: DIM,
            storage_mode: triviumdb::database::StorageMode::Mmap,
            sync_mode: triviumdb::storage::wal::SyncMode::Full,
            ..Default::default()
        },
    )
    .unwrap();
    let (results, synced) = database
        .group_commit(|database| {
            (1..=12u64)
                .map(|id| {
                    database.insert_with_id(
                        id,
                        &[id as f32, 1.0, 0.0, 0.0],
                        serde_json::json!({"group": true, "id": id}),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert!(synced && results.iter().all(Result::is_ok));
    std::fs::write(&ready, b"ready").unwrap();
    std::mem::forget(database);
    loop {
        std::thread::park();
    }
}

#[test]
fn PWR_00B_group_commit确认后强杀可恢复全部独立事务() {
    let path = tmp_db("group_commit_kill");
    cleanup(&path);
    let mut command = child_command();
    command
        .env("TRIVIUM_GROUP_COMMIT_CHILD", &path)
        .arg("__group_commit_power_loss_child_entry");
    kill_at_barrier(command, &format!("{path}.ready"));
    let expected = (1..=12)
        .map(|id| {
            power_node(
                id,
                [id as f32, 1., 0., 0.],
                serde_json::json!({"group":true,"id":id}),
            )
        })
        .collect::<Vec<_>>();
    assert_reopened(&path, &expected);
    cleanup(&path);
}

#[test]
fn __power_loss_child_entry() {
    let Ok(path) = std::env::var("TRIVIUM_POWER_LOSS_CHILD") else {
        return;
    };
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for index in 0..8u32 {
        db.insert(
            &[index as f32, 1.0, 0.0, 0.0],
            serde_json::json!({"phase": "wal", "index": index}),
        )
        .unwrap();
    }
    // 阻止析构；父进程会立即强制终止这个小型专用子进程。
    std::mem::forget(db);
    std::fs::write(format!("{path}.ready"), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn PWR_00_真实强杀子进程后WAL保持可恢复() {
    let path = tmp_db("real_process_kill");
    cleanup(&path);
    let mut command = child_command();
    command
        .env("TRIVIUM_POWER_LOSS_CHILD", &path)
        .arg("__power_loss_child_entry");
    kill_at_barrier(command, &format!("{path}.ready"));

    let expected = (0..8u32)
        .map(|i| {
            power_node(
                u64::from(i) + 1,
                [i as f32, 1., 0., 0.],
                serde_json::json!({"phase":"wal","index":i}),
            )
        })
        .collect::<Vec<_>>();
    assert_reopened(&path, &expected);
    cleanup(&path);
}

#[path = "../support/facts.rs"]
mod node_facts;

fn power_node(id: u64, vector: [f32; DIM], payload: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"id":id, "vector":vector, "payload":payload, "edges":[]})
}

fn assert_reopened(path: &str, expected: &[serde_json::Value]) {
    let expected = serde_json::Value::Array(expected.to_vec());
    for _ in 0..2 {
        let mut db = Database::<f32>::open(path, DIM).unwrap();
        assert_eq!(node_facts::nodes(&db), expected);
        db.close().unwrap();
    }
}

#[test]
fn PWR_01_正常析构与间隔检查点在连续重开后保持每笔写入() {
    let path = tmp_db("rapid_cycle");
    cleanup(&path);
    let mut expected = Vec::new();
    for round in 0..100u32 {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        assert_eq!(
            node_facts::nodes(&db),
            serde_json::json!(expected),
            "重开轮次 {round}"
        );
        for j in 0..if round % 10 == 0 { 5 } else { 3 } {
            let id = expected.len() as u64 + 1;
            let vector = [round as f32, j as f32, 0., 0.];
            let payload = serde_json::json!({"r":round,"j":j});
            assert_eq!(db.insert(&vector, payload.clone()).unwrap(), id);
            expected.push(power_node(id, vector, payload));
        }
        if round % 10 == 0 {
            db.flush().unwrap();
        }
    }
    assert_reopened(&path, &expected);
    cleanup(&path);
}

#[test]
fn PWR_02_未发布的暂存文件不改变已提交记录() {
    let path = tmp_db("flush_interrupt");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let mut expected = Vec::new();
    for i in 0..50u32 {
        let id = u64::from(i) + 1;
        let vector = [i as f32, 0., 0., 0.];
        let payload = serde_json::json!({"idx":i});
        db.insert_with_id(id, &vector, payload.clone()).unwrap();
        expected.push(power_node(id, vector, payload));
    }
    db.close().unwrap();
    // 暂存文件具有真实格式头，但在正文前被截断。
    let checkpoint = std::fs::read(&path).unwrap();
    std::fs::write(format!("{path}.tmp"), &checkpoint[..6]).unwrap();
    std::fs::write(format!("{path}.vec.tmp"), b"CORRUPT_VEC").unwrap();
    assert_reopened(&path, &expected);
    cleanup(&path);
}

#[test]
fn PWR_03_删除检查点后只保留指定节点及其完整内容() {
    let path = tmp_db("delete_cycle");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let mut expected = Vec::new();
    for id in 1..=100 {
        let vector = [id as f32, 0., 0., 0.];
        let payload = serde_json::json!({"id":id});
        db.insert_with_id(id, &vector, payload.clone()).unwrap();
        if id > 50 {
            expected.push(power_node(id, vector, payload));
        }
    }
    db.close().unwrap();
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=50 {
        db.delete(id).unwrap();
    }
    db.flush().unwrap();
    drop(db);
    assert_reopened(&path, &expected);
    cleanup(&path);
}

#[test]
fn PWR_04_已提交事务在析构与再次持久化后保持全部记录() {
    let path = tmp_db("tx_crash");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let mut tx = db.begin_tx();
    let mut expected = Vec::new();
    for i in 0..20u32 {
        let vector = [i as f32, 0., 0., 0.];
        let payload = serde_json::json!({"tx_item":i});
        tx.insert(&vector, payload.clone());
        expected.push(power_node(u64::from(i) + 1, vector, payload));
    }
    tx.commit().unwrap();
    drop(db);
    assert_reopened(&path, &expected);
    cleanup(&path);
}

#[cfg(feature = "test-hooks")]
mod snapshot;

#[path = "../support/cleanup.rs"]
mod cleanup_files;
