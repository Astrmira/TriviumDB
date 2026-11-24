//! CI 稳定性能报告入口。
//!
//! 使用固定随机种子和小型内置数据生成可重复的延迟、吞吐与召回指标，并写入
//! `target/bench-reports`。它用于趋势和 Gate，不代替外部百万向量质量评测。

#[path = "support/recall.rs"]
mod recall;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::time::Instant;
use triviumdb::Database;
use triviumdb::database::{Config, SearchConfig, StorageMode};

const DIM: usize = 384;
const NODE_COUNT: usize = 1500;
const QUERY_COUNT: usize = 20;

type VectorCorpus = Vec<(u64, Vec<f32>)>;
type QueryVectors = Vec<Vec<f32>>;

#[derive(Clone)]
struct BenchMetric {
    name: String,
    category: String,
    iterations: usize,
    elapsed_ms: f64,
    ops_per_sec: f64,
    memory_bytes: usize,
    rss_bytes: u64,
    rss_peak_bytes: u64,
    disk_bytes: u64,
    correctness: String,
}

fn disk_bytes(path: &str) -> u64 {
    ["", ".wal", ".vec", ".flush_ok"]
        .iter()
        .map(|ext| {
            fs::metadata(format!("{}{}", path, ext))
                .map(|meta| meta.len())
                .unwrap_or(0)
        })
        .sum()
}

fn rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let Ok(status) = fs::read_to_string("/proc/self/status") else {
            return 0;
        };
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                return rest
                    .split_whitespace()
                    .next()
                    .and_then(|kb| kb.parse::<u64>().ok())
                    .map(|kb| kb * 1024)
                    .unwrap_or(0);
            }
        }
        0
    }

    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

fn update_peak_rss(peak: &mut u64) -> u64 {
    let current = rss_bytes();
    *peak = (*peak).max(current);
    current
}

fn gen_vec(rng: &mut StdRng, dim: usize, center: Option<&[f32]>) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    for (i, x) in v.iter_mut().enumerate() {
        let base = center.map(|c| c[i] * 0.35).unwrap_or(0.0);
        *x = base + rng.gen_range(-1.0..1.0);
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.into_iter().map(|x| x / norm).collect()
}

fn brute_force_top_k(vectors: &[(u64, Vec<f32>)], query: &[f32], top_k: usize) -> Vec<u64> {
    let norm = |vector: &[f32]| {
        vector
            .iter()
            .map(|&x| f64::from(x).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    let query_norm = norm(query);
    let mut scored: Vec<_> = vectors
        .iter()
        .map(|(id, vector)| {
            let dot: f64 = vector
                .iter()
                .zip(query)
                .map(|(&a, &b)| f64::from(a) * f64::from(b))
                .sum();
            (*id, dot / (query_norm * norm(vector)))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().take(top_k).map(|(id, _)| id).collect()
}

struct BenchState {
    metrics: Vec<BenchMetric>,
    peak_rss: u64,
}

fn measure(
    state: &mut BenchState,
    category: &str,
    name: &str,
    iterations: usize,
    path: &str,
    memory_bytes: usize,
    op: impl FnMut() -> String,
) {
    measure_results(
        state,
        (category, name),
        iterations,
        path,
        memory_bytes,
        op,
        |samples| {
            let mut counts = BTreeMap::new();
            for value in samples {
                *counts.entry(value).or_insert(0usize) += 1;
            }
            if counts.len() == 1 {
                counts.into_keys().next().unwrap()
            } else {
                counts
                    .into_iter()
                    .map(|(value, count)| format!("{value} × {count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        },
    )
}

fn measure_results<R>(
    state: &mut BenchState,
    (category, name): (&str, &str),
    iterations: usize,
    path: &str,
    memory_bytes: usize,
    mut op: impl FnMut() -> R,
    summarize: impl FnOnce(Vec<R>) -> String,
) {
    assert!(iterations > 0);
    update_peak_rss(&mut state.peak_rss);
    let mut samples = Vec::with_capacity(iterations);
    let start = Instant::now();
    for _ in 0..iterations {
        samples.push(op());
        update_peak_rss(&mut state.peak_rss);
    }
    let elapsed = start.elapsed();
    let correctness = summarize(samples);
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    let ops_per_sec = if elapsed.as_secs_f64() > 0.0 {
        iterations as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };
    let current_rss = update_peak_rss(&mut state.peak_rss);
    state.metrics.push(BenchMetric {
        name: name.to_string(),
        category: category.to_string(),
        iterations,
        elapsed_ms,
        ops_per_sec,
        memory_bytes,
        rss_bytes: current_rss,
        rss_peak_bytes: state.peak_rss,
        disk_bytes: disk_bytes(path),
        correctness,
    });
}

fn build_report_db(path: &str) -> (Database<f32>, VectorCorpus, QueryVectors) {
    let mut rng = StdRng::seed_from_u64(20260430);
    let centers: Vec<Vec<f32>> = (0..12).map(|_| gen_vec(&mut rng, DIM, None)).collect();
    let mut db = Database::<f32>::open_with_config(
        path,
        Config {
            dim: DIM,
            storage_mode: StorageMode::Mmap,
            ..Default::default()
        },
    )
    .unwrap();
    db.disable_auto_compaction();

    let mut vectors = Vec::with_capacity(NODE_COUNT);
    let mut ids = Vec::with_capacity(NODE_COUNT);
    for i in 0..NODE_COUNT {
        let vector = gen_vec(&mut rng, DIM, Some(&centers[i % centers.len()]));
        let id = db
            .insert(
                &vector,
                json!({
                    "idx": i,
                    "type": match i % 5 {
                        0 => "event",
                        1 => "person",
                        2 => "document",
                        3 => "task",
                        _ => "concept",
                    },
                    "cluster": i % centers.len(),
                    "active": i % 3 != 0,
                    "score": (i % 100) as f64 / 100.0,
                    "title": format!("node_{i}"),
                }),
            )
            .unwrap();
        db.index_text(
            id,
            if i % 7 == 0 {
                "rust database vector graph search"
            } else {
                "general memory benchmark payload"
            },
        )
        .unwrap();
        vectors.push((id, vector));
        ids.push(id);
    }

    for i in 0..NODE_COUNT {
        let src = ids[i];
        let next = ids[(i + 1) % NODE_COUNT];
        let skip = ids[(i + 17) % NODE_COUNT];
        db.link(src, next, "next", 1.0).unwrap();
        db.link(src, skip, if i % 2 == 0 { "related" } else { "ref" }, 0.7)
            .unwrap();
    }

    db.create_index("type").unwrap();
    db.create_index("cluster").unwrap();
    db.build_text_index().unwrap();
    db.flush().unwrap();

    let queries = (0..QUERY_COUNT)
        .map(|i| gen_vec(&mut rng, DIM, Some(&centers[i % centers.len()])))
        .collect();
    (db, vectors, queries)
}

fn write_reports(metrics: &[BenchMetric], path: &str) {
    fs::create_dir_all("target/bench-reports").unwrap();
    let mut md = String::new();
    md.push_str("# TriviumDB CI Benchmark Report\n\n");
    md.push_str(&format!(
        "数据规模：{} 节点，{} 维，{} 个查询\n\n",
        NODE_COUNT, DIM, QUERY_COUNT
    ));
    md.push_str(
        "| 类别 | 场景 | 迭代 | 总耗时(ms) | ops/s | 内存估算(bytes) | RSS(bytes) | RSS峰值(bytes) | 磁盘(bytes) | 正确率/校验 |\n",
    );
    md.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---|\n");
    for metric in metrics {
        md.push_str(&format!(
            "| {} | {} | {} | {:.3} | {:.3} | {} | {} | {} | {} | {} |\n",
            metric.category,
            metric.name,
            metric.iterations,
            metric.elapsed_ms,
            metric.ops_per_sec,
            metric.memory_bytes,
            metric.rss_bytes,
            metric.rss_peak_bytes,
            metric.disk_bytes,
            metric.correctness
        ));
    }
    fs::write("target/bench-reports/ci_benchmark_report.md", md).unwrap();

    let json = metrics
        .iter()
        .map(|metric| {
            format!(
                "{{\"category\":\"{}\",\"name\":\"{}\",\"iterations\":{},\"elapsed_ms\":{:.6},\"ops_per_sec\":{:.6},\"memory_bytes\":{},\"rss_bytes\":{},\"rss_peak_bytes\":{},\"disk_bytes\":{},\"correctness\":\"{}\"}}",
                metric.category,
                metric.name,
                metric.iterations,
                metric.elapsed_ms,
                metric.ops_per_sec,
                metric.memory_bytes,
                metric.rss_bytes,
                metric.rss_peak_bytes,
                metric.disk_bytes,
                metric.correctness.replace('"', "'")
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    fs::write(
        "target/bench-reports/ci_benchmark_report.json",
        format!("[\n{}\n]\n", json),
    )
    .unwrap();
    println!("报告已生成: target/bench-reports/ci_benchmark_report.md");
    println!("数据库文件总占用: {} bytes", disk_bytes(path));
}

fn main() {
    fs::create_dir_all("target/bench-reports").unwrap();
    let directory = tempfile::tempdir_in("target/bench-reports").unwrap();
    let file = directory.path().join("source.tdb");
    let path = file.to_str().unwrap();
    let (mut db, vectors, queries) = build_report_db(path);
    let mut state = BenchState {
        metrics: Vec::new(),
        peak_rss: rss_bytes(),
    };
    let ids = db.all_node_ids().unwrap();
    let anchor = ids[ids.len() / 2];

    // 先在生成真值的同一批向量上测量，再开始写入、更新和事务基准。
    let truth: Vec<_> = queries
        .iter()
        .map(|query| brute_force_top_k(&vectors, query, 10))
        .collect();
    let hybrid_config = SearchConfig {
        top_k: 10,
        min_score: -1.0,
        ..Default::default()
    };
    for (name, hybrid) in [
        ("search_bruteforce_top10", false),
        ("search_hybrid_bq_recall", true),
    ] {
        let mut inputs = queries.iter();
        measure_results(
            &mut state,
            ("向量", name),
            queries.len(),
            path,
            db.estimated_memory(),
            || {
                let query = inputs.next().unwrap();
                if hybrid {
                    db.search_hybrid(None, Some(query), &hybrid_config).unwrap()
                } else {
                    db.search_exact(query, 10).unwrap()
                }
            },
            |answers| {
                let recall = recall::at_k(&answers, &truth, 10, 1..=vectors.len() as u64, |hit| {
                    (hit.id, hit.score)
                });
                format!("recall@10={recall:.3}")
            },
        );
    }

    measure(
        &mut state,
        "写入",
        "insert",
        80,
        path,
        db.estimated_memory(),
        || {
            let i = db.node_count().unwrap();
            let vector = vec![0.01f32; DIM];
            let id = db
                .insert(&vector, json!({"idx": i, "type": "bench_insert"}))
                .unwrap();
            if db.contains(id).unwrap() {
                "ok".to_string()
            } else {
                "missing_insert".to_string()
            }
        },
    );

    let update_ids = db.all_node_ids().unwrap();
    let mut update_pos = 0usize;
    measure(
        &mut state,
        "写入",
        "update_payload_update_vector_link_unlink",
        60,
        path,
        db.estimated_memory(),
        || {
            let id = update_ids[update_pos % update_ids.len()];
            let dst = update_ids[(update_pos + 3) % update_ids.len()];
            update_pos += 1;
            db.update_payload(id, json!({"idx": id, "type": "updated"}))
                .unwrap();
            db.update_vector(id, &vec![0.02f32; DIM]).unwrap();
            db.link(id, dst, "bench", 0.5).unwrap();
            db.unlink(id, dst).unwrap();
            match db.get_payload(id).unwrap() {
                Some(payload)
                    if payload.get("type").and_then(|v| v.as_str()) == Some("updated") =>
                {
                    "ok".to_string()
                }
                _ => "bad_update".to_string(),
            }
        },
    );

    let mut last_tx_id = None;
    measure(
        &mut state,
        "写入",
        "transaction_commit",
        30,
        path,
        db.estimated_memory(),
        || {
            let id = 1_000_000 + db.node_count().unwrap() as u64;
            let mut tx = db.begin_tx();
            tx.insert_with_id(id, &vec![0.03f32; DIM], json!({"type": "tx"}));
            tx.commit().unwrap();
            last_tx_id = Some(id);
            if db.contains(id).unwrap() {
                "ok".to_string()
            } else {
                "missing_tx".to_string()
            }
        },
    );

    measure(
        &mut state,
        "持久化",
        "flush_compact_reopen",
        3,
        path,
        db.estimated_memory(),
        || {
            db.flush().unwrap();
            db.compact().unwrap();
            let count = db.node_count().unwrap();
            db.close().unwrap();
            db = Database::<f32>::open(path, DIM).unwrap();
            db.disable_auto_compaction();
            assert_eq!(db.node_count().unwrap(), count);
            let committed = db.get(last_tx_id.unwrap()).unwrap().unwrap();
            assert_eq!(committed.vector, vec![0.03f32; DIM]);
            assert_eq!(committed.payload, json!({"type":"tx"}));
            format!("nodes={count}")
        },
    );

    let mut read_pos = 0usize;
    measure(
        &mut state,
        "读取",
        "get_get_payload_get_edges_contains_neighbors",
        300,
        path,
        db.estimated_memory(),
        || {
            let id = ids[read_pos % ids.len()];
            read_pos += 1;
            let ok = db.get(id).unwrap().is_some()
                && db.get_payload(id).unwrap().is_some()
                && db.contains(id).unwrap()
                && !db.get_edges(id).unwrap().is_empty()
                && !db.neighbors(id, 2).unwrap().is_empty();
            if ok {
                "ok".to_string()
            } else {
                "bad_read".to_string()
            }
        },
    );

    let text_config = SearchConfig {
        top_k: 20,
        min_score: -1.0,
        enable_advanced_pipeline: true,
        enable_text_hybrid_search: true,
        ..Default::default()
    };
    measure(
        &mut state,
        "向量/文本",
        "search_hybrid_text_only",
        80,
        path,
        db.estimated_memory(),
        || {
            let hits = db
                .search_hybrid(Some("rust vector"), None, &text_config)
                .unwrap();
            let ok = hits.iter().all(|hit| hit.payload.get("idx").is_some());
            format!("hits={},ok={ok}", hits.len())
        },
    );

    let advanced_config = SearchConfig {
        top_k: 10,
        expand_depth: 2,
        min_score: -1.0,
        teleport_alpha: 0.15,
        enable_advanced_pipeline: true,
        enable_sparse_residual: true,
        enable_dpp: true,
        enable_text_hybrid_search: true,
        ..Default::default()
    };
    measure(
        &mut state,
        "向量/图",
        "search_advanced_with_context",
        40,
        path,
        db.estimated_memory(),
        || {
            let (hits, ctx) = db
                .search_hybrid_with_context(
                    Some("database graph"),
                    Some(queries[0].as_slice()),
                    &advanced_config,
                )
                .unwrap();
            format!("hits={},stages={}", hits.len(), ctx.stage_timings.len())
        },
    );

    let search_tql = format!("SEARCH VECTOR {} TOP 5 RETURN *", json!(vec![0.01; DIM]));
    measure(
        &mut state,
        "TQL",
        "find_match_optional_search_order_limit",
        60,
        path,
        db.estimated_memory(),
        || {
            let find = db.tql(r#"FIND {type: "event"} RETURN * LIMIT 20"#).unwrap();
            let matched = db
                .tql(&format!(
                    "MATCH (a {{id: {anchor}}})-[:next]->(b) RETURN b LIMIT 5"
                ))
                .unwrap();
            let optional = db
                .tql(r#"OPTIONAL MATCH (a {type: "missing"})-[]->(b) RETURN b LIMIT 5"#)
                .unwrap();
            let search = db.tql(&search_tql).unwrap();
            assert_eq!(search.len(), 5);
            format!(
                "find={},match={},optional={},search={}",
                find.len(),
                matched.len(),
                optional.len(),
                search.len()
            )
        },
    );

    measure(
        &mut state,
        "TQL写入",
        "create_set_read_detach_delete",
        20,
        path,
        db.estimated_memory(),
        || {
            let create = db
                .tql_mut(r#"CREATE ({name: "ci_bench", type: "temp"})"#)
                .unwrap();
            let set = db
                .tql_mut(r#"MATCH (a {type: "temp"}) SET a.type == "temp_updated""#)
                .unwrap();
            let rows = db.tql(r#"FIND {type: "event"} RETURN * LIMIT 1"#).unwrap();
            let delete = db
                .tql_mut(r#"MATCH (a {type: "temp_updated"}) DETACH DELETE a"#)
                .unwrap();
            format!(
                "创建={},修改={},读取={},删除={}",
                create.affected,
                set.affected,
                rows.len(),
                delete.affected
            )
        },
    );

    measure(
        &mut state,
        "索引",
        "create_drop_property_index",
        20,
        path,
        db.estimated_memory(),
        || {
            db.create_index("active").unwrap();
            let rows = db.tql(r#"FIND {active: true} RETURN * LIMIT 20"#).unwrap();
            db.drop_index("active").unwrap();
            format!("rows={}", rows.len())
        },
    );

    measure(
        &mut state,
        "图算法",
        "neighbors_and_leiden_cluster",
        5,
        path,
        db.estimated_memory(),
        || {
            let ns = db.neighbors(anchor, 3).unwrap();
            let communities = db.leiden_cluster(3, Some(8), Some(true)).unwrap();
            format!(
                "neighbors={},clusters={}",
                ns.len(),
                communities.num_clusters
            )
        },
    );

    measure(
        &mut state,
        "生命周期",
        "migrate_to_close",
        1,
        path,
        db.estimated_memory(),
        || {
            let target = directory.path().join("migrated.tdb");
            let (mut migrated, migrated_ids) = db.migrate_to(target.to_str().unwrap(), 64).unwrap();
            let count = migrated.node_count().unwrap();
            migrated.close().unwrap();
            format!("nodes={count},ids={}", migrated_ids.len())
        },
    );

    measure(
        &mut state,
        "元数据",
        "node_count_dim_all_node_ids_estimated_memory",
        200,
        path,
        db.estimated_memory(),
        || {
            format!(
                "nodes={},dim={},ids={},memory={}",
                db.node_count().unwrap(),
                db.dim(),
                db.all_node_ids().unwrap().len(),
                db.estimated_memory()
            )
        },
    );

    write_reports(&state.metrics, path);
    db.close().unwrap();
}
