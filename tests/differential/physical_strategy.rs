#![cfg(feature = "test-hooks")]

use super::canonical::{CanonicalValue, canonicalize_rows};
use std::collections::BTreeMap;
use triviumdb::database::Database;
use triviumdb::test_hooks::{QueryExecutionStrategy, force_query_strategy};

fn database() -> (
    super::support::TestDir,
    Database<f32>,
    super::model::RefDatabase,
) {
    let root = super::support::TestDir::new("physical_strategy");
    let path = root.file("sample.tdb");
    let reference = super::model::RefDatabase::fixture(48);
    let mut database = Database::<f32>::open(&path, 3).unwrap();
    super::matrix::seed(&mut database, &reference);
    (root, database, reference)
}

#[test]
fn ForceSerial与ForceParallel执行结果一致() {
    let (_root, database, reference) = database();
    let query = "FIND {rank: {$gte: 0}} AS seed WITH seed WHERE seed.active == true RETURN seed, seed.rank AS rank ORDER BY seed.rank DESC LIMIT 12";
    let serial = {
        let _guard = force_query_strategy(QueryExecutionStrategy::ForceSerial);
        canonicalize_rows(database.tql_values(query).unwrap(), true)
    };
    let parallel = {
        let _guard = force_query_strategy(QueryExecutionStrategy::ForceParallel);
        canonicalize_rows(database.tql_values(query).unwrap(), true)
    };
    // 固定样本中偶数 rank 对应活跃节点，id 等于 rank + 1。
    let expected = (0..=46)
        .rev()
        .step_by(2)
        .take(12)
        .map(|rank| {
            BTreeMap::from([
                (
                    "seed".into(),
                    super::canonical::expected_node(&reference, rank + 1),
                ),
                ("rank".into(), CanonicalValue::Integer(rank as i64)),
            ])
        })
        .collect::<Vec<_>>();
    assert_eq!(serial, expected);
    assert_eq!(parallel, expected);
}

#[test]
fn ExpandRank融合与拆分物理路径结果一致() {
    let (_root, database, reference) = database();
    let query = "FIND {rank: 0} AS seed WITH seed EXPAND seed [:related*1..4] AS related RANK related BY VECTOR [1, 1, 0] TOP 3 RETURN related";
    let fused = canonicalize_rows(database.tql_values(query).unwrap(), true);
    let split = {
        let _guard = force_query_strategy(QueryExecutionStrategy::DisableFusion);
        canonicalize_rows(database.tql_values(query).unwrap(), true)
    };
    // 1 → 2 是起点唯一的 related 边；下一条为 next。
    let expected = vec![BTreeMap::from([(
        "related".into(),
        super::canonical::expected_node(&reference, 2),
    )])];
    assert_eq!(fused, expected);
    assert_eq!(split, expected);
}
