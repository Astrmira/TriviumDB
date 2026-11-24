use super::canonical::{CanonicalValue, canonicalize_rows};
use std::collections::BTreeMap;
use std::collections::HashMap;
use triviumdb::database::Database;
use triviumdb::query::tql_prepared::TqlParamValue;

use crate::support;

#[test]
fn Prepared与Direct在多组参数下逐行一致() {
    let root = support::TestDir::new("prepared_differential");
    let path = root.file("database.tdb");
    let mut database = Database::<f32>::open(&path, 2).unwrap();
    for id in 1..=24 {
        database
            .insert_with_id(
                id,
                &[id as f32, 1.0],
                serde_json::json!({"rank": id as i64}),
            )
            .unwrap();
    }
    let prepared = database
        .prepare_tql("MATCH (node) WHERE node.rank >= $min AND node.rank <= $max RETURN node, node.rank AS rank")
        .unwrap();
    assert_eq!(prepared.parameter_names(), vec!["max", "min"]);
    for (min, max) in [(1, 1), (3, 8), (10, 24), (30, 40)] {
        let direct = database
            .tql_values(&format!(
                "MATCH (node) WHERE node.rank >= {min} AND node.rank <= {max} RETURN node, node.rank AS rank"
            ))
            .unwrap();
        let parameters = HashMap::from([
            ("min".into(), TqlParamValue::Int(min)),
            ("max".into(), TqlParamValue::Int(max)),
        ]);
        let actual = database
            .execute_prepared_tql(&prepared, &parameters)
            .unwrap();
        let mut expected = (1..=24i64)
            .filter(|rank| (min..=max).contains(rank))
            .map(|rank| {
                let node = CanonicalValue::Node {
                    id: rank as u64,
                    vector: [rank as f32, 1.].map(f32::to_bits).to_vec(),
                    payload: Box::new(CanonicalValue::Object(BTreeMap::from([(
                        "rank".into(),
                        CanonicalValue::Integer(rank),
                    )]))),
                    edges: Vec::new(),
                };
                BTreeMap::from([
                    ("node".into(), node),
                    ("rank".into(), CanonicalValue::Integer(rank)),
                ])
            })
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(
            canonicalize_rows(actual, false),
            expected,
            "Prepared: {min}..={max}"
        );
        assert_eq!(
            canonicalize_rows(direct, false),
            expected,
            "Direct: {min}..={max}"
        );
    }
    let missing = database
        .execute_prepared_tql(&prepared, &HashMap::new())
        .unwrap_err();
    assert_eq!(missing.code(), "TDB_MISSING_QUERY_PARAMETER");
}
