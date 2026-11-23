//! 维护准入和构建拒绝保持完整事实，文本与属性候选分别按完成点持久化。

#[path = "../support/index_maintenance.rs"]
pub(super) mod maintenance;
use serde_json::json;
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::{Database, VectorType};

#[test]
fn 查询限额和制品统计在重开前后一致() {
    use triviumdb::query::tql_executor::TqlValue;
    let root = super::support::TestDir::new("limit-and-stats");
    let path = root.file("records.tdb");
    let cfg = Config {
        dim: 1,
        auto_build_quiver: false,
        payload_cache_bytes: 0,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
    db.insert_with_id(1, &[1.], json!({"g":1})).unwrap();
    db.insert_with_id(2, &[1.], json!({"g":2,"p":"x".repeat(600_000)}))
        .unwrap();
    db.link(1, 2, "next", 1.).unwrap();
    db.close().unwrap();
    let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
    db.set_memory_limit(db.estimated_memory() + 1).unwrap();
    assert_eq!(
        db.neighbors(1, 1).unwrap_err().code(),
        "TDB_CAPACITY_RESERVATION_REJECTED"
    );
    assert_eq!(
        db.tql("MATCH (n {id:1}) RETURN n.id AS n")
            .unwrap_err()
            .code(),
        "TDB_CAPACITY_RESERVATION_REJECTED"
    );
    db.set_memory_limit(0).unwrap();
    assert_eq!(db.neighbors(1, 1).unwrap(), [2]);
    for indexed in [false, true] {
        if indexed {
            db.set_memory_limit(0).unwrap();
            db.create_index("g").unwrap();
        }
        db.set_memory_limit(1 << 20).unwrap();
        let rows = db.tql("FIND {g:1} RETURN _.id AS n LIMIT 1").unwrap();
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0]["n"], TqlValue::Int(1)));
        assert!(
            db.tql("FIND {g:2} RETURN _.id AS n ORDER BY n LIMIT 0")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            db.tql("MATCH (n {id:2}) WHERE n.g == 99 RETURN n.id")
                .unwrap_err()
                .code(),
            "TDB_CAPACITY_RESERVATION_REJECTED"
        );
        assert!(
            db.tql("FIND {g:2} RETURN _.id AS n LIMIT 0")
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(db.index_memory_stats().posting_entries, 2);
    db.set_memory_limit(0).unwrap();
    db.close().unwrap();
    let mut db = Database::<f32>::open_read_only(&path, 1).unwrap();
    assert!(db.storage_info().sidecars[".pld"]);
    assert_eq!(db.index_memory_stats().posting_entries, 2);
    let fields = db.index_info().unwrap();
    assert_eq!(
        fields
            .iter()
            .map(|f| (&*f.field, f.entry_count))
            .collect::<Vec<_>>(),
        [("g", 2)]
    );
    db.close().unwrap();
}

fn matrix<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(dtype: &str) {
    let root = super::support::TestDir::new(dtype);
    let contract = maintenance::contract();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let config = Config {
            dim: 2,
            storage_mode: mode,
            load_text_index: true,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<T>::open_with_config(&path, config).unwrap();
        maintenance::seed(&mut db);
        let before_wal = std::fs::read(format!("{path}.wal")).unwrap();
        for case in contract["validation"].as_array().unwrap() {
            let error = maintenance::execute(&mut db, case).unwrap_err();
            assert_eq!(error.code(), case["code"].as_str().unwrap(), "{case}");
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["target"],
                case["target"]
            );
            if case["reason"] == true {
                assert!(
                    error.details()["reason"]
                        .as_str()
                        .is_some_and(|reason| !reason.is_empty())
                );
                assert_eq!(error.details().as_object().unwrap().len(), 1);
            } else {
                assert_eq!(error.details(), case["details"]);
            }
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["write"],
                contract["validation_write"]
            );
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before_wal);
            assert_eq!(
                maintenance::facts(&db),
                maintenance::expected(false, json!([]))
            );
        }
        // 1 KiB 余量足以保留当前事实，长正文和关键词的构建峰值需要额外准入。
        db.set_memory_limit(db.estimated_memory() + 1024).unwrap();
        for api in ["index_text", "index_keyword"] {
            let case = json!({"api":api,"id":1,"text":"x".repeat(4096),"keyword":"x".repeat(4096)});
            let error = maintenance::execute(&mut db, &case).unwrap_err();
            assert_eq!(error.code(), "TDB_CAPACITY_RESERVATION_REJECTED");
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["write"],
                contract["validation_write"]
            );
        }
        db.set_memory_limit(0).unwrap();
        assert_eq!(
            maintenance::facts(&db),
            maintenance::expected(false, json!([]))
        );
        db.close().unwrap();
        for case in contract["operations"].as_array().unwrap() {
            let error = maintenance::execute(&mut db, case).unwrap_err();
            assert_eq!(error.code(), "TDB_DATABASE_CLOSED", "{case}");
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["write"],
                contract["admission"]
            );
        }
        let file_group = || {
            std::fs::read_dir(&root.0)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    (entry.file_name(), std::fs::read(entry.path()).unwrap())
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let before_files = file_group();
        for access in [AccessMode::ReadOnly, AccessMode::Immutable] {
            let mut reader = Database::<T>::open_with_config(
                &path,
                Config {
                    access_mode: access,
                    ..config
                },
            )
            .unwrap();
            for case in contract["operations"].as_array().unwrap() {
                let error = maintenance::execute(&mut reader, case).unwrap_err();
                assert_eq!(error.code(), "TDB_READ_ONLY", "{case}/{access:?}");
                assert_eq!(error.details(), json!({"operation":case["api"]}));
                assert_eq!(
                    serde_json::to_value(error.info()).unwrap()["write"],
                    contract["admission"]
                );
            }
            assert_eq!(
                maintenance::facts(&reader),
                maintenance::expected(false, json!([]))
            );
            reader.close().unwrap();
            assert_eq!(file_group(), before_files);
        }
        let mut db = Database::<T>::open_with_config(&path, config).unwrap();
        maintenance::execute(&mut db, &json!({"api":"rebuild_text_index"})).unwrap();
        db.create_unique_index("tag").unwrap();
        let indexes = json!([{"kind":"Hash","fields":["tag"],"unique":true}]);
        assert_eq!(
            maintenance::facts(&db),
            maintenance::expected(true, indexes.clone())
        );
        db.close().unwrap();
        let mut reopened = Database::<T>::open_with_config(&path, config).unwrap();
        assert_eq!(
            maintenance::facts(&reopened),
            maintenance::expected(true, indexes)
        );
        reopened.close().unwrap();
    }
}

#[test]
fn maintenance_admission_f32() {
    matrix::<f32>("maintenance-f32");
}
#[test]
fn maintenance_admission_f16() {
    matrix::<half::f16>("maintenance-f16");
}
#[test]
fn maintenance_admission_u64() {
    matrix::<u64>("maintenance-u64");
}

#[test]
fn generation_id拒绝先于源检查点且保留完整事实() {
    let root = super::support::TestDir::new("generation-validation-checkpoint");
    let path = root.file("db.tdb");
    let config = Config {
        dim: 2,
        load_text_index: true,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    maintenance::seed(&mut db);
    db.update_payload(1, maintenance::contract()["nodes"][0]["payload"].clone())
        .unwrap();
    let source_files = || {
        ["", ".wal", ".flush_ok"].map(|suffix| std::fs::read(format!("{path}{suffix}")).unwrap())
    };
    let before = source_files();
    let destination = root.file("snapshot");
    for generation in ["".to_owned(), " ".to_owned(), "学".repeat(86)] {
        for error in [
            db.create_snapshot(&destination, &generation).unwrap_err(),
            db.publish_generation_manifest(&generation).unwrap_err(),
        ] {
            assert_eq!(error.code(), "TDB_INVALID_INPUT");
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["write"],
                serde_json::json!({"phase":"validation","outcome":"not_committed"})
            );
        }
        assert_eq!(source_files(), before);
        assert!(!std::path::Path::new(&destination).exists());
        assert_eq!(
            maintenance::facts(&db),
            maintenance::expected(false, json!([]))
        );
    }
    db.close().unwrap();
    let mut reopened = Database::<f32>::open_with_config(&path, config).unwrap();
    assert_eq!(
        maintenance::facts(&reopened),
        maintenance::expected(false, json!([]))
    );
    reopened.close().unwrap();
}

#[test]
fn text_workspace_rejection_preserves_facts_and_retries_after_quota_release() {
    use triviumdb::resources::{ResourceLimits, ResourceRuntime};
    let root = super::support::TestDir::new("text-workspace-quota");
    let path = root.file("db.tdb");
    let config = Config {
        dim: 2,
        load_text_index: true,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut seed = Database::<f32>::open_with_config(&path, config).unwrap();
    maintenance::seed(&mut seed);
    seed.close().unwrap();
    // 预先发布空库，使占位实例关闭时只释放配额，避免额外的检查点工作区干扰重试条件。
    let blocker_path = root.file("blocker.tdb");
    Database::<f32>::open_with_config(&blocker_path, config)
        .unwrap()
        .close()
        .unwrap();
    // 总额 2 MiB 保护 256 KiB 维护空间，两个实例声明 1 MiB 与 512 KiB，前台余量为 256 KiB。
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 2 * 1024 * 1024,
        maintenance_memory_limit: 256 * 1024,
        ..Default::default()
    })
    .unwrap();
    let mut db = Database::<f32>::open_with_runtime(
        &path,
        Config {
            memory_limit: 1024 * 1024,
            ..config
        },
        runtime.clone(),
    )
    .unwrap();
    let mut blocker = Database::<f32>::open_with_runtime(
        &blocker_path,
        Config {
            memory_limit: 512 * 1024,
            ..config
        },
        runtime.clone(),
    )
    .unwrap();
    assert_eq!(runtime.stats().available_foreground_bytes, Some(256 * 1024));
    let text = "x".repeat(4096);
    let error = db.index_text(1, &text).unwrap_err();
    assert_eq!(error.code(), "QueueFull");
    assert_eq!(
        error.details(),
        json!({"resource":"memory_bytes","requested":262656,"used":1572864,"limit":1835008})
    );
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        maintenance::contract()["validation_write"]
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(
        maintenance::facts(&db),
        maintenance::expected(false, json!([]))
    );
    blocker.close().unwrap();
    db.index_text(1, &text).unwrap();
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    let rows = db
        .tql_values(&format!(
            "TEXT BM25 \"{text}\" TOP 10 AS hits WITH hits RETURN hits"
        ))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["hits"].as_node().unwrap().id, 1);
    db.index_text(1, "oldalpha").unwrap();
    db.set_memory_limit(db.estimated_memory() + 1024).unwrap();
    db.close().unwrap();
    let mut reopened = Database::<f32>::open_with_config(&path, config).unwrap();
    assert_eq!(
        maintenance::facts(&reopened),
        maintenance::expected(false, json!([]))
    );
    reopened.close().unwrap();
}

#[test]
fn 零节点检查点保留全部属性索引定义() {
    let dir = super::support::TestDir::new("empty-index-catalog");
    let expected: std::collections::BTreeSet<_> = (0..1025).map(|i| format!("key{i}")).collect();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = dir.file(&format!("{mode:?}.tdb"));
        let config = Config {
            dim: 1,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        db.insert_with_id(1, &[1.], json!({"key0":1})).unwrap();
        for field in &expected {
            db.create_index(field).unwrap();
        }
        db.delete(1).unwrap();
        db.close().unwrap();
        let db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert_eq!(db.node_count().unwrap(), 0);
        let fields = db.index_info().unwrap();
        assert!(fields.iter().all(|f| f.entry_count == 0));
        assert_eq!(
            fields
                .into_iter()
                .map(|f| f.field)
                .collect::<std::collections::BTreeSet<_>>(),
            expected
        );
    }
}

#[test]
fn 重建属性目录保留唯一约束且错误不安装部分目录() {
    use triviumdb::{index::property::PropertyIndexKind, storage::memtable::MemTable};
    let mut mt = MemTable::<f32>::new(1);
    mt.insert_with_id(1, &[1.], json!({"key":"abc","n":1}))
        .unwrap();
    mt.register_unique_property_index("key").unwrap();
    let defs = vec![(PropertyIndexKind::Ordered, vec!["n".to_owned()])];
    mt.rebuild_property_indexes_from_definitions(&defs).unwrap();
    let before = mt.property_indexes().index_definitions();
    assert_eq!(mt.unique_index_definitions(), [vec!["key".to_owned()]]);
    assert!(
        mt.rebuild_property_indexes_from_definitions(&[(
            PropertyIndexKind::Ngram,
            vec!["n".into()]
        )])
        .is_err()
    );
    assert_eq!(mt.property_indexes().index_definitions(), before);
    assert_eq!(
        mt.insert_with_id(2, &[2.], json!({"key":"abc"}))
            .unwrap_err()
            .code(),
        "TDB_UNIQUE_CONSTRAINT"
    );
}

#[test]
fn 拥有型点读和精确双曲检索分别按实例余量准入() {
    use triviumdb::index::pelorus::{PelorusConfig, PelorusSearchOptions};
    let dir = super::support::TestDir::new("owned-read-budget");
    let mut db = Database::<f32>::open(&dir.file("point.tdb"), 256).unwrap();
    let vector = [0.0; 256];
    let big = json!({"pad":"x".repeat(32*1024)});
    db.insert_with_id(1, &vector, big.clone()).unwrap();
    db.insert_with_id(2, &vector, json!(null)).unwrap();
    db.upsert_edge(1, 2, "rel", 1., big.clone()).unwrap();
    db.build_pelorus_index(PelorusConfig::default()).unwrap();
    let search = PelorusSearchOptions {
        top_k: 1,
        exact: true,
        ..Default::default()
    };
    db.set_memory_limit(db.estimated_memory() + 2048).unwrap();
    for err in [
        db.get(1).unwrap_err(),
        db.get_payload(1).unwrap_err(),
        db.get_edges(1).unwrap_err(),
        db.get_edge(1, 2, "rel").unwrap_err(),
        db.get_incoming_edges(2, None).unwrap_err(),
    ] {
        assert_eq!(err.code(), "TDB_CAPACITY_RESERVATION_REJECTED");
    }
    db.set_memory_limit(0).unwrap();
    assert_eq!(db.get(1).unwrap().unwrap().payload, big);
    assert_eq!(db.get_edges(1).unwrap()[0].metadata, big);
    db.update_payload(1, json!(null)).unwrap();
    db.set_memory_limit(db.estimated_memory() + 2048).unwrap();
    assert_eq!(
        db.search_pelorus(&vector, &search).unwrap_err().code(),
        "TDB_CAPACITY_RESERVATION_REJECTED"
    );
    db.set_memory_limit(0).unwrap();
    assert_eq!(db.search_pelorus(&vector, &search).unwrap()[0].id, 1);
}

#[test]
fn 关键词预热拒绝后仍可编译并返回原始命中() {
    let dir = super::support::TestDir::new("keyword-build-budget");
    let mut db = Database::<f32>::open(&dir.file("text.tdb"), 1).unwrap();
    db.insert_with_id(9, &[1.], json!(null)).unwrap();
    for i in 0..120 {
        db.index_keyword(9, &format!("term{i:03}")).unwrap();
    }
    let before = db.estimated_memory();
    db.set_memory_limit(before + 16).unwrap();
    assert_eq!(
        db.build_text_index().unwrap_err().code(),
        "TDB_CAPACITY_RESERVATION_REJECTED"
    );
    assert_eq!(db.estimated_memory(), before);
    db.set_memory_limit(0).unwrap();
    db.build_text_index().unwrap();
    let rows = db
        .tql_values("TEXT AC 'term119' TOP 1 AS n WITH n RETURN n")
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["n"].as_node().unwrap().id)
            .collect::<Vec<_>>(),
        [9]
    );
}

#[test]
fn 宽复合索引仅为过滤所用字段对读取一次事实() {
    let dir = super::support::TestDir::new("pair-scan");
    let mut db = Database::<f32>::open(&dir.file("pairs.tdb"), 1).unwrap();
    for id in 1..=20 {
        db.insert_with_id(id, &[1.], json!({"a":id,"b":id,"c":id,"d":id,"e":id}))
            .unwrap();
    }
    db.create_composite_index(&["a", "b", "c", "d", "e"].map(String::from))
        .unwrap();
    let sql = "EXPLAIN SEARCH VECTOR [1] TOP 1 AS n WITH n WHERE n.a == 1 AND n.b == 1 RETURN n";
    for reads in [20, 0] {
        let before = db.payload_memory_stats().payload_lookups;
        assert_eq!(db.tql(sql).unwrap().len(), 1);
        assert_eq!(db.payload_memory_stats().payload_lookups - before, reads);
    }
}
