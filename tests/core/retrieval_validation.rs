//! 非有限向量和数量算术在所有 Result 检索入口保持一致。
use super::support::TestDir;
use serde_json::json;
use triviumdb::database::{BatchSearchConfig, Config, SearchConfig, StorageMode};
use triviumdb::{Database, TriviumError, VectorType};

#[test]
fn 检索配置在空批次和正常查询中保留准确字段() {
    let root = TestDir::new("retrieval_config_fields");
    let path = root.file("data.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    db.insert_with_id(7, &[1., 0.], json!({"saved": "完整事实"}))
        .unwrap();
    let wal_path = format!("{path}.wal");
    let wal = std::fs::read(&wal_path).unwrap();
    for (field, setter) in [
        (
            "min_score",
            (|config: &mut SearchConfig, value| config.min_score = value)
                as fn(&mut SearchConfig, f32),
        ),
        ("teleport_alpha", |config: &mut SearchConfig, value| {
            config.teleport_alpha = value
        }),
        ("fista_lambda", |config: &mut SearchConfig, value| {
            config.fista_lambda = value
        }),
        ("fista_threshold", |config: &mut SearchConfig, value| {
            config.fista_threshold = value
        }),
        ("dpp_quality_weight", |config: &mut SearchConfig, value| {
            config.dpp_quality_weight = value
        }),
        ("text_boost", |config: &mut SearchConfig, value| {
            config.text_boost = value
        }),
        ("min_edge_weight", |config: &mut SearchConfig, value| {
            config.min_edge_weight = value
        }),
    ] {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut config = SearchConfig::default();
            setter(&mut config, value);
            for result in [
                db.search_advanced(&[1., 0.], &config),
                db.search_batch(&[], &config, &BatchSearchConfig::default())
                    .map(|rows| rows.into_iter().flatten().collect()),
            ] {
                let error = result.unwrap_err();
                assert!(matches!(error.underlying(), TriviumError::InvalidInput(_)));
                assert_eq!(
                    serde_json::to_value(error.info().target).unwrap(),
                    json!({"field": field})
                );
            }
            assert_eq!(std::fs::read(&wal_path).unwrap(), wal);
        }
    }
    assert_eq!(
        db.search_exact(&[1., 0.], 1)
            .unwrap()
            .into_iter()
            .map(|hit| (hit.id, hit.score, hit.payload))
            .collect::<Vec<_>>(),
        vec![(7, 1., json!({"saved": "完整事实"}))]
    );
    db.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    let node = reopened.get(7).unwrap().unwrap();
    assert_eq!(node.vector, vec![1., 0.]);
    assert_eq!(node.payload, json!({"saved": "完整事实"}));
    reopened.close().unwrap();
}

fn invalid_vectors<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("invalid_query_vectors");
        let path = root.file("data.tdb");
        let config = Config {
            dim: 2,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<T>::open_with_config(&path, config).unwrap();
        for phase in 0..3 {
            if phase == 1 {
                db.insert_with_id(1, &[T::from_f32(1.), T::zero()], json!({}))
                    .unwrap();
            }
            if phase == 2 {
                db.close().unwrap();
                db = Database::open_with_config(&path, config).unwrap();
            }
            for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let query = [T::from_f32(invalid), T::zero()];
                assert!(matches!(
                    db.search_exact(&query, 1),
                    Err(TriviumError::InvalidVector { .. })
                ));
                assert!(matches!(
                    db.search(&query, 1, 0, -1.),
                    Err(TriviumError::InvalidVector { .. })
                ));
                assert!(matches!(
                    db.search_advanced(&query, &SearchConfig::default()),
                    Err(TriviumError::InvalidVector { .. })
                ));
                for anchors in [vec![], vec![1]] {
                    assert!(matches!(
                        db.search_graph_first(&query, &anchors, 1, 10),
                        Err(TriviumError::InvalidVector { .. })
                    ));
                }
                assert!(matches!(
                    db.search_batch(
                        &[query.to_vec()],
                        &SearchConfig::default(),
                        &BatchSearchConfig::default()
                    ),
                    Err(TriviumError::InvalidVector { .. })
                ));
            }
            assert!(matches!(
                db.search_exact(&[], 1),
                Err(TriviumError::DimensionMismatch {
                    expected: 2,
                    got: 0
                })
            ));
        }
        db.close().unwrap();
    }
}

#[test]
fn 浮点检索在空库持有库及重开后一致拒绝非有限向量() {
    invalid_vectors::<f32>();
    invalid_vectors::<half::f16>();
}

#[test]
fn 极大候选数量在精确和索引路径有界返回实际内容() {
    let root = TestDir::new("huge_topk");
    let mut db = Database::<f32>::open_with_config(
        &root.file("data.tdb"),
        Config {
            dim: 2,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"sign":1})).unwrap();
    db.insert_with_id(2, &[-1., 0.], json!({"sign":-1}))
        .unwrap();
    let tuples = |hits: Vec<triviumdb::node::SearchHit>| {
        hits.into_iter()
            .map(|hit| (hit.id, hit.score, hit.payload))
            .collect::<Vec<_>>()
    };
    let expected = vec![(1, 1., json!({"sign":1})), (2, -1., json!({"sign":-1}))];
    for indexed in [false, true] {
        if indexed {
            db.build_quiver_index(None).unwrap();
        }
        for count in [usize::MAX / 8 + 1, usize::MAX / 2 + 1, usize::MAX] {
            assert_eq!(
                tuples(db.search(&[1., 0.], count, 0, -1.).unwrap()),
                expected
            );
            assert_eq!(tuples(db.search_exact(&[1., 0.], count).unwrap()), expected);
            assert_eq!(
                tuples(db.search_graph_first(&[1., 0.], &[2, 1], count, 2).unwrap()),
                expected
            );
            for (recall_k, rerank_k) in [(0, 0), (count, count)] {
                let config = SearchConfig {
                    top_k: count,
                    recall_k,
                    rerank_k,
                    expand_depth: 0,
                    min_score: -1.,
                    ..Default::default()
                };
                assert_eq!(
                    tuples(db.search_advanced(&[1., 0.], &config).unwrap()),
                    expected
                );
                let batches = db
                    .search_batch(&[vec![1., 0.]], &config, &BatchSearchConfig::default())
                    .unwrap();
                assert_eq!(tuples(batches.into_iter().next().unwrap()), expected);
            }
        }
    }
    db.close().unwrap();
}

#[test]
fn 直接物理检索算子在空候选前拒绝非有限输入() {
    use triviumdb::query::pipeline::{
        ExactRerank, ExactVectorSearch, FistaResidualRecall, NodeSet, PipelineBudget,
        PipelineContext, PipelineOperator, QuiverVectorSearch,
    };
    let mt = triviumdb::storage::memtable::MemTable::<f32>::new(2);
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let query = vec![invalid, 0.];
        let operators: Vec<Box<dyn PipelineOperator<f32>>> = vec![
            Box::new(ExactVectorSearch {
                query: query.clone(),
                top_k: 1,
            }),
            Box::new(QuiverVectorSearch {
                query: query.clone(),
                top_k: 1,
                ef_search: 8,
            }),
            Box::new(ExactRerank {
                query: query.clone(),
                top_k: Some(1),
            }),
            Box::new(FistaResidualRecall {
                query,
                top_k: 1,
                lambda: 0.1,
                threshold: 0.1,
                iterations: 1,
            }),
        ];
        for operator in operators {
            let mut context = PipelineContext::new(&mt, PipelineBudget::default());
            assert!(
                matches!(
                    operator.apply(NodeSet::empty(), &mut context),
                    Err(TriviumError::InvalidVector { .. })
                ),
                "{}",
                operator.name()
            );
        }
    }
}

#[test]
fn 非法检索保留已发布的不可变代际() {
    let root = TestDir::new("invalid_search_manifest");
    let path = root.file("data.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    let count = triviumdb::storage::memtable::auto_quiver_node_threshold(2);
    let mut tx = triviumdb::database::TxBuilder::new();
    for id in 1..=count as u64 {
        tx.insert_with_id(id, &[1., 0.], json!(id));
    }
    db.commit_tx(tx).unwrap();
    db.publish_generation_manifest("before").unwrap();
    let idx = std::fs::read(format!("{path}.idx")).unwrap();
    for (field, config) in [
        (
            "min_edge_weight",
            SearchConfig {
                min_edge_weight: -1.,
                ..Default::default()
            },
        ),
        (
            "bm25_k1",
            SearchConfig {
                bm25_k1: 0.,
                ..Default::default()
            },
        ),
        (
            "bm25_b",
            SearchConfig {
                bm25_b: 1.1,
                ..Default::default()
            },
        ),
    ] {
        let error = db
            .search_hybrid(Some("term"), Some(&[1., 0.]), &config)
            .unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
        assert_eq!(
            serde_json::to_value(error.info().target).unwrap(),
            json!({"field":field})
        );
    }
    db.close().unwrap();
    assert_eq!(std::fs::read(format!("{path}.idx")).unwrap(), idx);
    let reader = Database::<f32>::open_immutable(&path, 2).unwrap();
    assert_eq!(reader.node_count().unwrap(), count);
    assert_eq!(
        reader.get_payload(count as u64).unwrap(),
        Some(json!(count))
    );
}

#[test]
fn 检索批次累计持有结果并在失败后归还配额() {
    use triviumdb::{ResourceLimits, ResourceRuntime};
    let mib = 1024 * 1024;
    for (name, shared, instance, code) in [
        (
            "instance",
            16 * mib,
            4 * mib,
            "TDB_CAPACITY_RESERVATION_REJECTED",
        ),
        ("runtime", 11 * mib, 8 * mib, "QueueFull"),
    ] {
        let root = TestDir::new(name);
        let runtime = ResourceRuntime::new(ResourceLimits {
            memory_limit: shared,
            ..Default::default()
        })
        .unwrap();
        let mut db = Database::<f32>::open_with_runtime(
            &root.file("data.tdb"),
            Config {
                dim: 2,
                memory_limit: instance,
                payload_cache_bytes: 0,
                ..Default::default()
            },
            runtime.clone(),
        )
        .unwrap();
        let payload = json!("x".repeat(64 * 1024));
        db.insert_with_id(1, &[1., 0.], payload.clone()).unwrap();
        let config = SearchConfig {
            top_k: 1,
            expand_depth: 0,
            ..Default::default()
        }
        .with_exact(true);
        let queries = vec![vec![1., 0.]; 128];
        for parallelism in [1, 2] {
            let batch = BatchSearchConfig { parallelism };
            let initial = runtime.stats().reserved_workspace_bytes;
            assert_eq!(
                db.search_batch(&queries, &config, &batch)
                    .unwrap_err()
                    .code(),
                code
            );
            assert_eq!(runtime.stats().reserved_workspace_bytes, initial);
            let rows = db.search_batch(&queries[..2], &config, &batch).unwrap();
            assert_eq!(rows.len(), 2);
            for row in rows {
                assert_eq!(row.len(), 1);
                assert_eq!(
                    (row[0].id, row[0].score, &row[0].payload),
                    (1, 1., &payload)
                );
            }
        }
    }
}
