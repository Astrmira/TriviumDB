//! 通过公开查询入口验证认知算子的唯一性、数值尺度与有限结果。

use super::support::TestDir;
use serde_json::json;
use triviumdb::database::{Config, StorageMode};
use triviumdb::query::tql_executor::TqlValue;
use triviumdb::{Database, HookContext, SearchConfig, SearchHit, SearchHook};

#[test]
fn 残差按输入尺度拟合并按执行阶段计费() {
    use triviumdb::query::pipeline::{
        FistaResidualRecall, NodeSet, PipelineBudget, PipelineContext, PipelineOperator,
    };
    use triviumdb::storage::memtable::MemTable;
    for (scale, lambda, expected_weight) in [(1e20_f32, 0.1, 1.0), (1e-6, 2.5e-13, 0.75)] {
        let (weights, _, norm) =
            triviumdb::cognitive::fista_solve(&[scale], &[vec![scale]], lambda, 30);
        assert!((weights[0] - expected_weight).abs() < 1e-5);
        assert!((norm / scale - (1.0 - expected_weight)).abs() < 1e-5);
    }
    let mut mt = MemTable::<f32>::new(2);
    mt.insert_with_id(1, &[1., 0.], json!({})).unwrap();
    mt.insert_with_id(2, &[0., 1.], json!({})).unwrap();
    let mut op = FistaResidualRecall {
        query: vec![1., 0.],
        top_k: 1,
        lambda: 0.1,
        threshold: 0.3,
        iterations: 30,
    };
    let budget = || PipelineBudget {
        max_vector_read_bytes: 8,
        ..Default::default()
    };
    assert!(
        op.apply(
            NodeSet::from_ids([1]),
            &mut PipelineContext::new(&mt, budget())
        )
        .unwrap()
        .is_empty()
    );
    op.threshold = 0.;
    assert!(
        op.apply(
            NodeSet::from_ids([1]),
            &mut PipelineContext::new(&mt, budget())
        )
        .is_err()
    );
    assert!(
        op.apply(NodeSet::empty(), &mut PipelineContext::new(&mt, budget()))
            .unwrap()
            .is_empty()
    );
    op.iterations = 0;
    assert_eq!(
        op.apply(NodeSet::empty(), &mut PipelineContext::new(&mt, budget()))
            .unwrap_err()
            .code(),
        "TDB_INVALID_INPUT"
    );
}

#[test]
fn 残差查询区分拟合完成与上游空集() {
    let root = TestDir::new("residual-query");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 2).unwrap();
    db.insert_with_id(1, &[1e20, 0.], json!({})).unwrap();
    db.insert_with_id(2, &[0., 1e20], json!({})).unwrap();
    for input in ["SEARCH VECTOR [1e20,0] TOP 1 AS s", "MATCH (s {id:999})"] {
        let query = format!(
            "{input} WITH s RESIDUAL s BY VECTOR [1e20,0] TOP 1 LAMBDA 0.1 THRESHOLD 1 ITERATIONS 30 AS r WITH r RETURN id(r) AS id"
        );
        assert!(db.tql(&query).unwrap().is_empty());
    }
    let rows = db.tql("MATCH (s {id:1}) WITH s RESIDUAL s BY VECTOR [0,1e20] TOP 1 LAMBDA 0.1 THRESHOLD 1 ITERATIONS 30 AS r WITH r RETURN id(r) AS id").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(scalar(&rows[0]["id"]), json!(2));
}

#[test]
fn 多样性选择保留尺度与前缀且大投影的主题平分() {
    use triviumdb::cognitive::{dpp_greedy, nmf_analyze_query};
    for s in [1.0, 1e-14, 1e30] {
        assert_eq!(
            dpp_greedy(&[vec![s, 0.], vec![s, 0.], vec![0., s]], &[1.; 3], 2, 1.),
            [0, 2]
        );
    }
    for (k, expected) in [(1, vec![1]), (2, vec![1, 0]), (4, vec![1, 0])] {
        assert_eq!(
            dpp_greedy(&[vec![1., 0.], vec![0., 1.]], &[0.1, 1.], k, 1.),
            expected
        );
    }
    let (depth, coverage, novelty, topics) =
        nmf_analyze_query(&[1e30, 1e30], &[1e15, 0., 0., 1e15], 2, 2);
    assert_eq!(
        (depth, coverage, novelty, topics),
        (0., 2, 1., vec![0.5, 0.5])
    );
}

struct HighQualityRecall;

fn scalar(value: &TqlValue<f32>) -> serde_json::Value {
    match value {
        TqlValue::Int(value) => json!(value),
        TqlValue::Float(value) => json!(value),
        _ => panic!("预期数值标量，实际为 {value:?}"),
    }
}

impl SearchHook for HighQualityRecall {
    fn on_custom_recall(
        &self,
        _: &[f32],
        _: &SearchConfig,
        _: &mut HookContext,
    ) -> Option<Vec<SearchHit>> {
        Some(
            (1..=3)
                .map(|id| SearchHit {
                    id,
                    score: 100.0,
                    payload: json!({}),
                })
                .collect(),
        )
    }
}

#[test]
fn dpp_高质量因子在高级检索和TQL中返回完整的不同节点() {
    let root = TestDir::new("dpp-finite-quality");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let mut db = Database::<f32>::open_with_config(
            &root.file(&format!("{mode:?}.tdb")),
            Config {
                dim: 2,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        for (id, vector) in [(1, [1.0, 0.0]), (2, [0.8, 0.6]), (3, [0.6, 0.8])] {
            db.insert_with_id(id, &vector, json!({})).unwrap();
            db.index_keyword(id, "alpha").unwrap();
        }
        db.set_hook(HighQualityRecall).unwrap();
        let hits = db
            .search_advanced(
                &[1.0, 0.0],
                &SearchConfig {
                    top_k: 2,
                    recall_k: 3,
                    rerank_k: 3,
                    min_score: -1.0,
                    expand_depth: 0,
                    enable_advanced_pipeline: true,
                    enable_dpp: true,
                    dpp_quality_weight: 10.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            hits.iter()
                .map(|hit| (hit.id, hit.score))
                .collect::<Vec<_>>(),
            [(1, 100.0), (3, 100.0)]
        );
        let text = "alpha ".repeat(100);
        let rows = db.tql(&format!("TEXT AC \"{text}\" TOP 3 AS hits WITH hits DIVERSIFY hits TOP 2 QUALITY_WEIGHT 10 AS selected WITH selected RETURN selected.id AS id, text_score(selected) AS quality, diversity_score(selected) AS rank ORDER BY rank DESC")).unwrap();
        assert_eq!(rows.len(), 2);
        for (row, id, rank) in [(&rows[0], 1, 1.0), (&rows[1], 3, 0.5)] {
            assert_eq!(scalar(&row["id"]), json!(id));
            assert_eq!(scalar(&row["quality"]), json!(100.0));
            assert_eq!(scalar(&row["rank"]), json!(rank));
        }
        db.close().unwrap();
    }
}

#[test]
fn nmf_大向量在两个存储模式保留全部有限主题权重() {
    let root = TestDir::new("nmf-finite-topics");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for scale in [1.0f32, 1e30, f32::MAX] {
            let mut db = Database::<f32>::open_with_config(
                &root.file(&format!("{mode:?}-{scale}.tdb")),
                Config {
                    dim: 2,
                    storage_mode: mode,
                    auto_build_quiver: false,
                    ..Default::default()
                },
            )
            .unwrap();
            for (id, vector) in [(1, [scale, 0.0]), (2, [0.0, scale]), (3, [scale, scale])] {
                db.insert_with_id(id, &vector, json!({})).unwrap();
            }
            let query = "SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed TOPICS seed K 2 ITERATIONS 8 AS clustered WITH clustered RETURN clustered.id AS id, topic(clustered) AS topic, topic_score(clustered) AS score ORDER BY id";
            let rows = db.tql(query).unwrap();
            assert_eq!(rows.len(), 3);
            for (index, row) in rows.iter().enumerate() {
                assert_eq!(scalar(&row["id"]), json!(index + 1));
                assert!(matches!(row["topic"], TqlValue::Int(0..=1)));
                assert!(
                    matches!(row["score"], TqlValue::Float(value) if value.is_finite() && value > 0.0),
                    "{mode:?}, {scale}, {row:?}"
                );
            }
            let again = db.tql(query).unwrap();
            assert_eq!(again.len(), 3);
            for (first, second) in rows.iter().zip(&again) {
                for field in ["id", "topic", "score"] {
                    assert_eq!(scalar(&first[field]), scalar(&second[field]));
                }
            }
            db.close().unwrap();
        }
    }
}

#[test]
fn 调和中心性在TQL投影排序和聚合中保留f64尺度() {
    let root = TestDir::new("harmonic-finite-score");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let mut db = Database::<f32>::open_with_config(
            &root.file(&format!("{mode:?}.tdb")),
            Config {
                dim: 2,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        db.insert_with_id(1, &[1.0, 0.0], json!({})).unwrap();
        db.insert_with_id(2, &[0.0, 1.0], json!({})).unwrap();
        let weight = 1e-40f32;
        db.link(1, 2, "rel", weight).unwrap();
        let prefix = "SEARCH VECTOR [0, 0] TOP 2 AS seed WITH seed HARMONIC_CENTRALITY seed LABEL rel AS ranked WITH ranked";
        let rows = db.tql(&format!("{prefix} RETURN ranked.id AS id, harmonic_centrality(ranked) AS score, graph_score(ranked) AS graph ORDER BY score DESC")).unwrap();
        assert_eq!(rows.len(), 2);
        let expected = 1.0 / f64::from(weight);
        for (row, id, score) in [(&rows[0], 1, expected), (&rows[1], 2, 0.0)] {
            assert_eq!(scalar(&row["id"]), json!(id));
            assert_eq!(scalar(&row["score"]), json!(score));
            assert_eq!(scalar(&row["graph"]), json!(score));
        }
        let aggregate = db
            .tql(&format!(
                "{prefix} RETURN AVG(harmonic_centrality(ranked)) AS average"
            ))
            .unwrap();
        assert_eq!(aggregate.len(), 1);
        assert_eq!(scalar(&aggregate[0]["average"]), json!(expected / 2.0));
        db.close().unwrap();
    }
}

#[test]
fn 浮点认知算子在汉明指纹库统一拒绝() {
    let dir = TestDir::new("binary-cognitive");
    let mut db = Database::<u64>::open(&dir.file("data.tdb"), 2).unwrap();
    db.insert_with_id(1, &[1 << 60, 0], json!({})).unwrap();
    for stage in [
        "RESIDUAL s BY VECTOR [1,0] TOP 1 LAMBDA 0.1 THRESHOLD 0 ITERATIONS 2",
        "DIVERSIFY s TOP 1 QUALITY_WEIGHT 1",
        "TOPICS s K 1 ITERATIONS 2",
    ] {
        let error = db
            .tql(&format!(
                "SEARCH VECTOR [1,0] TOP 1 AS s WITH s {stage} AS r WITH r RETURN r.id AS id"
            ))
            .unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT", "{stage}: {error}");
    }
}

#[test]
fn 残差准入包含宽锚点与查询工作区() {
    use triviumdb::query::pipeline::{
        FistaResidualRecall, NodeIdsSource, PipelineBudget, PipelineContext, execute_pipeline,
    };
    use triviumdb::storage::memtable::MemTable;
    let mut mt = MemTable::<f32>::new(4096);
    let mut anchor = vec![0.; 4096];
    anchor[0] = 1.;
    mt.insert_with_id(1, &anchor, json!({})).unwrap();
    let mut query = vec![0.; 4096];
    query[1] = 1.;
    let op = FistaResidualRecall {
        query,
        top_k: 1,
        lambda: 0.1,
        threshold: 0.,
        iterations: 2,
    };
    let mut context = PipelineContext::new(
        &mt,
        PipelineBudget {
            max_node_set_bytes: 1024,
            ..Default::default()
        },
    );
    let ops: Vec<Box<dyn triviumdb::query::pipeline::PipelineOperator<f32>>> =
        vec![Box::new(NodeIdsSource { ids: vec![1] }), Box::new(op)];
    assert_eq!(
        execute_pipeline(&mut context, &ops).unwrap_err().code(),
        "TDB_QUERY_EXECUTION"
    );
    context.budget.max_node_set_bytes = 1024 * 1024;
    let set = execute_pipeline(&mut context, &ops).unwrap();
    assert_eq!(set.rows().len(), 1);
    assert_eq!(set.rows()[0].id, 1);
    assert_eq!(set.rows()[0].residual_score.unwrap().value, 0.);
}

#[test]
fn 零向量仅贡献DPP正则项() {
    use triviumdb::cognitive::dpp_greedy;
    assert_eq!(
        dpp_greedy(&[vec![0.], vec![0.], vec![1.]], &[1.; 3], 2, 1.),
        [2, 0]
    );
}

#[test]
fn NMF拒绝缺项多项和溢出形状() {
    use triviumdb::cognitive::nmf_multiplicative_update;
    for (m, d, data) in [
        (1, 2, &[1.][..]),
        (1, 1, &[1., 0.][..]),
        (usize::MAX, 2, &[][..]),
    ] {
        assert_eq!(
            nmf_multiplicative_update(data, m, d, 1, 2, 0.01)
                .unwrap_err()
                .code(),
            "TDB_INVALID_INPUT"
        );
    }
}
