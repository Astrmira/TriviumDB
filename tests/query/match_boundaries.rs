//! 按物理边计行、显式路径栈、预算与运行中取消的独立样例。
use super::support::TestDir;
use serde_json::json;
use triviumdb::database::{Config, StorageMode};
use triviumdb::query::tql_executor::{QueryControl, TqlLimits, TqlValue, execute_tql_with_limits};
use triviumdb::query::tql_parser::parse_tql;
use triviumdb::storage::memtable::MemTable;
use triviumdb::{Database, TriviumError};

fn number(value: &TqlValue<f32>) -> u64 {
    match value {
        TqlValue::Int(value) => (*value).try_into().unwrap(),
        TqlValue::UInt(value) => *value,
        _ => panic!("应返回精确整数列: {value:?}"),
    }
}

#[test]
fn 属性索引与反转计划保持平行边的完整行重数() {
    let contract: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/graph_query_boundaries.json")).unwrap();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("parallel_match");
        let path = root.file("data.tdb");
        let config = Config {
            dim: 1,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        for node in contract["nodes"].as_array().unwrap() {
            db.insert_with_id(
                node["id"].as_u64().unwrap(),
                &[1.],
                json!({"kind": node["kind"]}),
            )
            .unwrap();
        }
        for edge in contract["edges"].as_array().unwrap() {
            db.link(
                edge[0].as_u64().unwrap(),
                edge[1].as_u64().unwrap(),
                edge[2].as_str().unwrap(),
                1.,
            )
            .unwrap();
        }
        for phase in 0..3 {
            if phase == 1 {
                db.create_index("kind").unwrap();
            }
            if phase == 2 {
                db.close().unwrap();
                db = Database::open_read_only(&path, 1).unwrap();
            }
            for case in contract["counts"].as_array().unwrap() {
                let rows = db.tql(case["query"].as_str().unwrap()).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(
                    number(&rows[0]["total"]),
                    case["expected"].as_u64().unwrap(),
                    "{mode:?}/{phase}/{case}"
                );
            }
            let case = &contract["distinct"];
            let rows = db.tql(case["query"].as_str().unwrap()).unwrap();
            let values: Vec<_> = rows
                .iter()
                .map(|row| json!({"x":number(&row["x"]), "y":number(&row["y"])}))
                .collect();
            assert_eq!(json!(values), case["rows"]);
        }
        db.close().unwrap();
    }
}

#[test]
fn 匹配的聚合与同分分页使用实体身份() {
    let root = TestDir::new("match_identity");
    let path = root.file("fixture.tdb");
    let mut db = Database::<f32>::open(&path, 1).unwrap();
    for id in 1..=8 {
        db.insert_with_id(
            id,
            &[1.],
            json!({"n":if id == 1 {json!(1)} else {json!(1.0)}, "selected":matches!(id, 3|4)}),
        )
        .unwrap();
    }
    for (source, target) in [(1, 4), (2, 3)] {
        db.link(source, target, "r", 1.).unwrap();
    }
    for phase in 0..3 {
        if phase == 1 {
            db.create_index("selected").unwrap();
        }
        if phase == 2 {
            db.close().unwrap();
            db = Database::open_read_only(&path, 1).unwrap();
        }
        let rows = db.tql("MATCH (a)-[:r]->(b {selected:true}) RETURN COLLECT(a.id) AS ids, MIN(a.n) AS lo, MAX(a.n) AS hi").unwrap();
        assert_eq!(rows.len(), 1);
        assert!(matches!(&rows[0]["ids"], TqlValue::List(ids) if ids == &[json!(1),json!(2)]));
        for key in ["lo", "hi"] {
            assert!(
                matches!(rows[0][key], TqlValue::Int(1)),
                "{key}: {:?}",
                rows[0]
            );
        }
        for pattern in ["(a)-[e:r]->(z)", "(z)-[e:r]->(a)"] {
            for (offset, expected) in [(0, (1, 4)), (1, (2, 3))] {
                let rows = db.tql(&format!("MATCH {pattern} RETURN e.source AS s, e.target AS t ORDER BY e.weight LIMIT 1 OFFSET {offset}")).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!((number(&rows[0]["s"]), number(&rows[0]["t"])), expected);
            }
        }
    }
}

#[test]
fn 字符串转义按码点持久化且非法代理项原子拒绝() {
    let root = TestDir::new("string_escapes");
    let path = root.file("escape.tdb");
    let mut db = Database::<f32>::open(&path, 1).unwrap();
    db.tql_mut(r#"CREATE ({v:"\u732b\r\b\f\/\n\t\\\"\uD83D\uDC08"})"#)
        .unwrap();
    for invalid in [r#""\uD800""#, r#""\uDC08""#, r#""\u12xy""#, r#""\q""#] {
        let error = db
            .tql_mut(&format!("CREATE ({{v:{invalid}}})"))
            .unwrap_err();
        assert_eq!(error.code(), "TDB_QUERY_PARSE");
    }
    db.close().unwrap();
    let db = Database::<f32>::open_read_only(&path, 1).unwrap();
    assert_eq!(db.all_node_ids().unwrap(), [1]);
    assert_eq!(
        db.get_payload(1).unwrap().unwrap(),
        json!({"v":"猫\r\x08\x0c/\n\t\\\"🐈"})
    );
}

#[test]
fn 预算仅覆盖返回页与实际节点投影() {
    let root = TestDir::new("graph_projection");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 1).unwrap();
    for id in 1..=4 {
        db.insert_with_id(id, &[1.], json!({"g":"x"})).unwrap();
    }
    db.link(1, 2, "r", 1.).unwrap();
    db.upsert_edge(1, 4, "other", 1., json!({"s":"x".repeat(65536)}))
        .unwrap();
    db.upsert_edge(3, 4, "r", 1., json!({"s":"x".repeat(65536)}))
        .unwrap();
    db.set_memory_limit(db.estimated_memory() + 16384).unwrap();
    let rows = db
        .tql("MATCH ()-[e:r]->() RETURN e.source AS i LIMIT 1")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(number(&rows[0]["i"]), 1);
    let endpoints = db
        .tql("MATCH (a)-[e:r]->(b) RETURN a.id AS s, b.id AS t LIMIT 1")
        .unwrap();
    assert_eq!(endpoints.len(), 1);
    assert_eq!(
        (number(&endpoints[0]["s"]), number(&endpoints[0]["t"])),
        (1, 2)
    );
    // FIND 按 Payload 过滤，选取带大出边的节点。
    let ids = db
        .tql(r#"FIND {g:"x"} RETURN _.id AS i LIMIT 1 OFFSET 2"#)
        .unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(number(&ids[0]["i"]), 3);
    let direct = db
        .shortest_path_detailed(1, 2, 3, Some("r".to_owned()))
        .unwrap();
    assert_eq!(direct.results[0].path, [1, 2]);
    assert_eq!(direct.traversed_edges, 1);
    assert!(!direct.truncated);
    for error in [
        db.edge_page(3, Default::default(), None, 0, 1).unwrap_err(),
        db.reachable_detailed(3, &Default::default()).unwrap_err(),
        db.search_graph_first(&[1.], &vec![1; 16384], 1, 1)
            .unwrap_err(),
    ] {
        assert_eq!(error.code(), "TDB_CAPACITY_RESERVATION_REJECTED");
    }
    assert_eq!(
        db.edge_page(3, Default::default(), None, 1, 1)
            .unwrap()
            .0
            .len(),
        0
    );
    db.set_memory_limit(0).unwrap();
    assert_eq!(
        db.edge_page(3, Default::default(), None, 0, 1).unwrap().0[0].metadata["s"]
            .as_str()
            .unwrap()
            .len(),
        65536
    );
}

#[test]
fn 双向单跳自环只计一次且平行边分别计数() {
    let root = TestDir::new("self_loop_match");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 1).unwrap();
    db.insert_with_id(1, &[1.], json!({})).unwrap();
    for label in ["red", "blue"] {
        db.link(1, 1, label, 1.).unwrap();
    }
    for edge in ["-[]->", "<-[]-", "-[]-", "-[e]-", "-[:red|blue]-"] {
        let rows = db
            .tql(&format!(
                "MATCH (a {{id:1}}){edge}(b) RETURN COUNT(*) AS total"
            ))
            .unwrap();
        assert_eq!(number(&rows[0]["total"]), 2, "{edge}");
        let query = format!(
            "MATCH (a {{id:1}}){edge}(b) WITH a,b WITH a AS source WHERE source.id == 1 RETURN COUNT(*) AS total"
        );
        let rows = db.tql(&query).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(number(&rows[0]["total"]), 2, "{query}");
    }
    let rows = db
        .tql("MATCH (a)-[e]->(b) WITH a,b RETURN DISTINCT a.id AS id,b.id AS target")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((number(&rows[0]["id"]), number(&rows[0]["target"])), (1, 1));
    let rows = db.tql("MATCH (a)-[e]->(b) WITH a INTERSECT a IDS [1] AS kept WITH kept RETURN COUNT(*) AS total").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(number(&rows[0]["total"]), 1);
    db.close().unwrap();
}

#[test]
fn 可变长路径恢复绑定并按每段简单路径处理环和零跳() {
    let mut mt = chain(3, false);
    mt.upsert_edge(3, 1, "next".into(), 1., json!({})).unwrap();
    let query = parse_tql("MATCH (a {id:1})-[:next*1..4]->(b) RETURN b").unwrap();
    let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
    assert_eq!(
        rows.iter().map(|row| row["b"].id).collect::<Vec<_>>(),
        [2, 3, 4]
    );
    let query = parse_tql("MATCH (a {id:1})-[:next]->(b)-[:next*1..3]->(a) RETURN a, b").unwrap();
    let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0]["a"].id, rows[0]["b"].id), (1, 2));
    let query = parse_tql("MATCH (a {id:1})-[:next*0..0]->(b) RETURN b").unwrap();
    let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["b"].id, 1);
}

#[test]
fn 路径栈工作区在展开前受实例预算限制() {
    let mt = chain(54, false);
    let query = parse_tql("MATCH (a {id:1})-[:next*54..54]->(b) RETURN b").unwrap();
    let limits = TqlLimits {
        memory_limit: 4096,
        ..Default::default()
    };
    assert!(matches!(
        execute_tql_with_limits(&query, &mt, limits)
            .unwrap_err()
            .underlying(),
        TriviumError::ResourceBudgetExceeded {
            resource: "query_memory_bytes",
            ..
        }
    ));
    let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["b"].id, 55);
}

fn chain(length: u64, branches: bool) -> MemTable<f32> {
    let mut mt = MemTable::new(1);
    mt.set_auto_build_quiver(false);
    for id in 1..=length + 1 {
        mt.insert_with_id(id, &[1.], json!({})).unwrap();
    }
    for id in 1..=length {
        mt.upsert_edge(id, id + 1, "next".into(), 1., json!({}))
            .unwrap();
        if branches && id < length {
            mt.upsert_edge(id, id + 2, "next".into(), 1., json!({}))
                .unwrap();
        }
    }
    mt
}

#[test]
fn 轻量计数在重开后保持行重数且不读取正文() {
    let root = TestDir::new("cold_count");
    let path = root.file("data.tdb");
    let config = Config {
        dim: 1,
        storage_mode: StorageMode::Mmap,
        payload_cache_bytes: 0,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    for id in 1..=8 {
        db.insert_with_id(id, &[1.], json!("x".repeat(32768)))
            .unwrap();
    }
    db.link(1, 2, "red", 1.).unwrap();
    db.link(1, 2, "blue", 1.).unwrap();
    db.close().unwrap();
    let db = Database::<f32>::open_with_config(
        &path,
        Config {
            memory_limit: 2400 * 1024,
            ..config
        },
    )
    .unwrap();
    let before = db.payload_memory_stats();
    for (query, expected) in [
        ("MATCH (n) RETURN COUNT(*) AS total", 8),
        (
            "MATCH (a)-[e]->(b) RETURN COUNT(*) AS total ORDER BY total",
            2,
        ),
        ("OPTIONAL MATCH (a)-[e]->(b) RETURN COUNT(*) AS total", 9),
    ] {
        let rows = db.tql(query).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(number(&rows[0]["total"]), expected);
    }
    let after = db.payload_memory_stats();
    assert_eq!(
        (after.payload_lookups, after.payload_parsed_bytes),
        (before.payload_lookups, before.payload_parsed_bytes)
    );
}

#[test]
fn 兼容节点接口区分同函数的多个聚合列() {
    let root = TestDir::new("aggregate_columns");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 1).unwrap();
    db.insert_with_id(1, &[1.], json!({})).unwrap();
    let rows = db
        .tql_nodes("MATCH (n) RETURN COUNT(*), COUNT(n.missing)")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 2);
    for (key, value) in [("Count(*)", 1), ("Count(n.missing)", 0)] {
        assert_eq!(rows[0][key].payload[key], value);
    }
}

#[test]
fn 数值减法与行注释支持不同空白形式() {
    let root = TestDir::new("expression_spacing");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 1).unwrap();
    db.insert_with_id(1, &[1.], json!({"rank":3})).unwrap();
    for newline in ["\r", "\r\n", "\n"] {
        for expr in ["3-1", "3 -1", "3- 1", "n.rank-1", "3+-1"] {
            let query = format!("MATCH (n) -- 注释{newline}RETURN {expr} AS result");
            assert_eq!(number(&db.tql(&query).unwrap()[0]["result"]), 2);
        }
    }
    let rows = db
        .tql("MATCH (n) RETURN -9223372036854775808 AS result")
        .unwrap();
    assert!(matches!(rows[0]["result"], TqlValue::Int(i64::MIN)));
}

#[test]
fn 小栈子进程完成深路径和超时回归() {
    let root = TestDir::new("match_worker");
    let receipt = root.file("completed");
    let mut child = super::query_child_command()
        .args([
            "--ignored",
            "--exact",
            "match_boundaries::small_stack_worker",
            "--nocapture",
        ])
        .env("TDB_MATCH_RECEIPT", &receipt)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("路径子进程超过等待上限");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(status.success(), "路径子进程必须正常完成: {status}");
    assert_eq!(
        std::fs::read(receipt).unwrap(),
        b"match-boundaries-completed"
    );
}

#[test]
#[ignore = "由父用例在独立进程执行"]
fn small_stack_worker() {
    let receipt = std::env::var("TDB_MATCH_RECEIPT").unwrap();
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            // 12000 跳远高于原递归实现可在 2 MiB 栈上安全处理的深度。
            let mt = chain(12_000, false);
            let query = parse_tql("MATCH (a {id:1})-[:next*12000..12000]->(b) RETURN b").unwrap();
            let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["b"].id, 12_001);
            // 普通多段模式也用显式栈，覆盖可变长路径之外的递归边界。
            // AST 公共入口也保持有界调用栈；字符串入口另有运算复杂度上限。
            use triviumdb::query::tql_ast::{
                EdgeDirection, QueryEntry, ReturnClause, TqlEdgePattern, TqlNodePattern,
            };
            let mut query = parse_tql("MATCH (n0 {id:1}) RETURN n0").unwrap();
            let QueryEntry::Match { pattern } = &mut query.entry else {
                panic!("应为 MATCH")
            };
            for id in 1..=12_000 {
                pattern.nodes.push(TqlNodePattern {
                    var: Some(format!("n{id}")),
                    filter: None,
                });
                pattern.edges.push(TqlEdgePattern {
                    var: None,
                    labels: vec!["next".into()],
                    hop_range: None,
                    direction: EdgeDirection::Forward,
                });
            }
            query.returns = ReturnClause::Variables(vec!["n12000".into()]);
            let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["n12000"].id, 12_001);

            // 55 节点的跳一/跳二 DAG 有指数级路径；最小跳数之前也必须消耗预算。
            let mt = chain(54, true);
            let query = parse_tql("MATCH (a {id:1})-[:next*54..54]->(b) RETURN b").unwrap();
            let started = std::time::Instant::now();
            let limits = TqlLimits {
                query_control: Some(QueryControl::with_deadline(
                    started + std::time::Duration::from_millis(100),
                )),
                ..Default::default()
            };
            assert!(matches!(
                execute_tql_with_limits(&query, &mt, limits)
                    .unwrap_err()
                    .underlying(),
                TriviumError::QueryDeadlineExceeded | TriviumError::QueryExecution(_)
            ));
            assert!(matches!(
                execute_tql_with_limits(&query, &mt, TqlLimits::default())
                    .unwrap_err()
                    .underlying(),
                TriviumError::QueryExecution(_)
            ));
        })
        .unwrap()
        .join()
        .unwrap();
    std::fs::write(receipt, b"match-boundaries-completed").unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 路径已经开始展开后取消和期限耗尽仍会终止执行() {
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    for deadline in [false, true] {
        let mt = chain(54, true);
        let query = parse_tql("MATCH (a {id:1})-[:next*54..54]->(b) RETURN b").unwrap();
        let expiry = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let control = if deadline {
            QueryControl::with_deadline(expiry)
        } else {
            QueryControl::default()
        };
        let worker_control = control.clone();
        let gate = pause_at(ConcurrencyPoint::TqlPathStep);
        let worker = std::thread::spawn(move || {
            execute_tql_with_limits(
                &query,
                &mt,
                TqlLimits {
                    query_control: Some(worker_control),
                    ..Default::default()
                },
            )
        });
        gate.wait_until_arrived();
        if deadline {
            // 屏障证明执行已进入路径展开；时钟等待只用于到达真实期限。
            while std::time::Instant::now() < expiry {
                std::thread::sleep(expiry.saturating_duration_since(std::time::Instant::now()));
            }
        } else {
            control.cancel();
        }
        gate.release();
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(
            error.code(),
            if deadline {
                "TDB_QUERY_DEADLINE_EXCEEDED"
            } else {
                "TDB_QUERY_CANCELLED"
            }
        );
    }
}

#[test]
fn 同一数据的向量过滤与范围分页在建索引和重开后保持结果() {
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        let root = TestDir::new("filtered_pages");
        let path = root.file("data.tdb");
        let config = Config {
            dim: 2,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        for (id, v, cat, x) in [
            (1, [1., 0.], "a", 40),
            (2, [0.8, 0.6], "a", 30),
            (3, [0., 1.], "b", 20),
            (4, [-1., 0.], "b", 10),
        ] {
            db.insert_with_id(id, &v, json!({"cat":cat,"x":x})).unwrap();
        }
        for phase in 0..3 {
            if phase == 1 {
                db.create_index("cat").unwrap();
                db.create_ordered_index("x").unwrap();
            }
            if phase == 2 {
                db.close().unwrap();
                db = Database::open_read_only(&path, 2).unwrap();
            }
            for (query, expected) in [
                (
                    r#"SEARCH VECTOR [1,0] TOP 1 WHERE {cat:"b"} RETURN _.id AS id"#,
                    vec![3],
                ),
                ("FIND {x:{$gt:0}} RETURN _.id AS id LIMIT 2", vec![1, 2]),
                (
                    "FIND {x:{$gt:0}} RETURN _.id AS id LIMIT 2 OFFSET 2",
                    vec![3, 4],
                ),
                (
                    r#"FIND {cat:"a"} RETURN _.id AS id ORDER BY _.x ASC LIMIT 1"#,
                    vec![2],
                ),
            ] {
                let ids: Vec<_> = db
                    .tql(query)
                    .unwrap()
                    .iter()
                    .map(|row| number(&row["id"]))
                    .collect();
                assert_eq!(ids, expected, "{mode:?}/{phase}/{query}");
            }
            let explanation = db
                .tql_nodes(r#"EXPLAIN SEARCH VECTOR [1,0] TOP 1 WHERE {cat:"b"} RETURN *"#)
                .unwrap();
            let plan = &explanation[0]["plan"].payload;
            assert_eq!(plan["filter_before_top_k"], true);
            assert_eq!(plan["vector_access_path"], "exact_fallback");
            assert_eq!(
                plan["access_path"]["kind"],
                if phase == 0 {
                    "full_node_scan"
                } else {
                    "property_index"
                }
            );
        }
        db.close().unwrap();
    }
}

#[test]
fn 匹配写按目标计数并在创建任何实体前拒绝无效输入() {
    let root = TestDir::new("matched-write-targets");
    let path = root.file("data.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    for id in 1..=4 {
        db.insert_with_id(id, &[1., 0.], json!({"n":id})).unwrap();
    }
    for id in 2..=4 {
        db.link(1, id, "r", 1.).unwrap();
    }
    let changed = db
        .tql_mut("MATCH (a)-[:r]->(b) SET VECTOR(a) == [0,1]")
        .unwrap();
    assert_eq!(changed.affected, 1);
    for query in [
        r#"MATCH (a {id:1}) CREATE (a {newkey:"x"})-[:q]->(z {x:9})"#,
        r#"MATCH (a {id:99}) CREATE (a {newkey:"x"})-[:q]->(z {x:9})"#,
        "CREATE (a {first:1 second:[2,3]})",
        "CREATE (a {first:1,second:[2 3]})",
    ] {
        assert!(db.tql_mut(query).is_err(), "{query}");
    }
    db.close().unwrap();
    let db = Database::<f32>::open_read_only(&path, 2).unwrap();
    assert_eq!(db.all_node_ids().unwrap(), [1, 2, 3, 4]);
    let node = db.get(1).unwrap().unwrap();
    assert_eq!(node.vector, [0., 1.]);
    assert_eq!(node.payload, json!({"n":1}));
    assert_eq!(
        node.edges
            .iter()
            .map(|e| (e.target_id, e.label.as_str()))
            .collect::<Vec<_>>(),
        [(2, "r"), (3, "r"), (4, "r")]
    );
}

#[test]
fn 深链路径只产出指定深度的完整节点序列() {
    let mt = chain(2048, false);
    let paths =
        triviumdb::graph::pathfinding::variable_length_paths(&mt, 1, 2048, 2048, Some("next"), 1);
    assert_eq!(paths, [(2049, (1..=2049).collect::<Vec<_>>())]);
}
