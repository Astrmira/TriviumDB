use super::canonical::{CanonicalValue, canonicalize_rows};
use super::graph_reference::{TraverseDirection, difference, intersection, shortest_path, union};
use super::model::RefDatabase;
use std::collections::{BTreeMap, BTreeSet};
use triviumdb::database::Database;

#[test]
fn shortest_path生产结果与独立BFS_reference一致() {
    let root = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&root).unwrap();
    let path = root
        .join("graph_path_differential.tdb")
        .to_string_lossy()
        .to_string();
    super::matrix::cleanup(&path);
    let reference = RefDatabase::fixture(9);
    let mut database = Database::<f32>::open(&path, 3).unwrap();
    super::matrix::seed(&mut database, &reference);

    for (source, target, label) in [
        (1, 2, Some("related")),
        (2, 3, Some("next")),
        (1, 5, None),
        (9, 1, None),
    ] {
        let expected = shortest_path(
            &reference,
            source,
            target,
            8,
            label,
            TraverseDirection::Forward,
        );
        let label_clause = label.map_or_else(String::new, |value| format!(" LABEL {value}"));
        let query = format!(
            "FIND {{rank: {}}} AS seed WITH seed shortest_paths seed TO [{target}]{label_clause} AS route WITH route RETURN path(route) AS path",
            source - 1
        );
        let rows = canonicalize_rows(database.tql_values(&query).unwrap(), true);
        let expected = expected
            .into_iter()
            .map(|path| BTreeMap::from([("path".into(), CanonicalValue::Path(path))]))
            .collect::<Vec<_>>();
        assert_eq!(rows, expected, "最短路径差分：{query}");
    }
    super::matrix::cleanup(&path);
}

#[test]
fn 集合代数生产Pipeline与独立BTreeSet_reference一致() {
    let root = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&root).unwrap();
    let path = root
        .join("set_differential.tdb")
        .to_string_lossy()
        .to_string();
    super::matrix::cleanup(&path);
    let reference = RefDatabase::fixture(8);
    let mut database = Database::<f32>::open(&path, 3).unwrap();
    super::matrix::seed(&mut database, &reference);
    let left = BTreeSet::from([1, 2, 3, 4]);
    let right = BTreeSet::from([3, 4, 5]);
    for (operation, expected) in [
        ("union", union(&left, &right)),
        ("intersect", intersection(&left, &right)),
        ("except", difference(&left, &right)),
    ] {
        let query = format!(
            "FIND {{rank: {{$lt: 4}}}} AS seed WITH seed {operation} seed IDS [3, 4, 5] AS combined WITH combined RETURN combined"
        );
        let actual = canonicalize_rows(database.tql_values(&query).unwrap(), false);
        let mut expected = expected
            .into_iter()
            .map(|id| {
                BTreeMap::from([(
                    "combined".into(),
                    super::canonical::expected_node(&reference, id),
                )])
            })
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(actual, expected, "集合差分：{query}");
    }
    super::matrix::cleanup(&path);
}
