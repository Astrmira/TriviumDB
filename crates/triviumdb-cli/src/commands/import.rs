//! `import` 子命令：从 JSONL 批量导入节点。
//!
//! 完整解析后在同一内核事务中插入节点并建立边，自动 ID 在提交预检时解析。

use std::fs::File;
use std::io::{BufRead, BufReader, Read};

use colored::Colorize;
use serde_json::Value;

use crate::CliResult;
use crate::db_handle::DbHandle;
use triviumdb::database::TxBuilder;
use triviumdb::node::Edge;
use triviumdb::{Database, TriviumError, VectorType};

struct Record<T> {
    id: Option<u64>,
    vector: Vec<T>,
    payload: Value,
    edges: Vec<Edge>,
}

pub fn run(handle: &mut DbHandle, input: &str, max_bytes: usize, max_ops: usize) -> CliResult {
    match handle {
        DbHandle::F32(database) => run_typed(database, input, false, max_bytes, max_ops),
        DbHandle::F16(database) => run_typed(database, input, false, max_bytes, max_ops),
        DbHandle::U64(database) => run_typed(database, input, true, max_bytes, max_ops),
    }
}

fn run_typed<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    database: &mut Database<T>,
    input: &str,
    integer_literals: bool,
    max_bytes: usize,
    max_ops: usize,
) -> CliResult {
    let file = File::open(input)?;
    let size = usize::try_from(file.metadata()?.len()).unwrap_or(usize::MAX);
    let admit = |resource, used: usize, requested: usize, limit: usize| {
        if requested > limit.saturating_sub(used) {
            Err(TriviumError::ResourceBudgetExceeded {
                resource,
                used,
                requested,
                limit,
            })
        } else {
            Ok(())
        }
    };
    admit("import_input_bytes", 0, size, max_bytes)?;
    let mut reader = BufReader::new(file.take((max_bytes as u64).saturating_add(1)));
    let mut bytes = 0;
    let mut operations = 0;
    let mut line = String::new();
    let mut lineno = 0;

    let mut builder = TxBuilder::new();
    let mut outgoing_edges = Vec::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        admit("import_input_bytes", bytes, n, max_bytes)?;
        bytes += n;
        lineno += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(trimmed).map_err(|e| {
            format!(
                "第 {} 行 JSON 解析失败 / Failed to parse JSON at line {}: {e}",
                lineno, lineno
            )
        })?;
        let added =
            1usize.saturating_add(v.get("edges").and_then(Value::as_array).map_or(0, Vec::len));
        admit("import_operations", operations, added, max_ops)?;
        operations += added;
        let record = parse_record::<T>(&v, lineno, integer_literals)?;
        if let Some(id) = record.id {
            builder.insert_with_id(id, &record.vector, record.payload);
        } else {
            builder.insert(&record.vector, record.payload);
        }
        outgoing_edges.push(record.edges);
    }

    let inserted = outgoing_edges.len();
    let linked = outgoing_edges
        .iter()
        .try_fold(0usize, |total, edges| total.checked_add(edges.len()))
        .ok_or("导入边计数溢出 (Import edge count overflow)")?;
    // 原生向量直接缓冲到事务；完整解析后按插入序号连接本批节点。
    for (insert_index, edges) in outgoing_edges.into_iter().enumerate() {
        for edge in edges {
            builder.upsert_edge_from_insert(
                insert_index,
                edge.target_id,
                &edge.label,
                edge.weight,
                edge.metadata,
            );
        }
    }
    database.commit_tx(builder)?;
    super::checkpoint_after_commit(|| database.flush())?;
    println!(
        "{} 已导入 {inserted} 个节点、{linked} 条边 / Imported {inserted} nodes and {linked} edges <- {input}",
        "✓".green().bold()
    );
    Ok(())
}

fn parse_record<T: VectorType>(
    v: &Value,
    lineno: usize,
    integer_literals: bool,
) -> Result<Record<T>, Box<dyn std::error::Error>> {
    let obj = v.as_object().ok_or_else(|| {
        format!("第 {lineno} 行必须是 JSON 对象 (Line {lineno} must be a JSON object)")
    })?;
    let id = match obj.get("id") {
        Some(x) => Some(x.as_u64().ok_or_else(|| {
            format!(
                "第 {lineno} 行 id 必须是非负整数 (Line {lineno}: id must be an unsigned integer)"
            )
        })?),
        None => None,
    };
    let vector_value = obj.get("vector").ok_or_else(|| {
        format!("第 {lineno} 行缺少 vector 字段 (Line {lineno}: vector is required)")
    })?;
    let vector_array = vector_value.as_array().ok_or_else(|| {
        format!("第 {lineno} 行 vector 必须是数组 (Line {lineno}: vector must be an array)")
    })?;
    if vector_array.is_empty() {
        return Err(format!(
            "第 {lineno} 行 vector 不能为空 (Line {lineno}: vector must be nonempty)"
        )
        .into());
    }
    let mut vector = Vec::with_capacity(vector_array.len());
    for (idx, item) in vector_array.iter().enumerate() {
        let value = item.as_number()
            .ok_or_else(|| format!("第 {lineno} 行 vector[{idx}] 必须是数字 (Line {lineno}: vector[{idx}] must be numeric)"))?;
        if integer_literals && !value.is_u64() {
            return Err(TriviumError::InvalidVector {
                reason: "u64 JSONL 分量须使用无符号整数字面量 (u64 JSONL components require unsigned integer literals)".into(),
            }.at_field(&format!("vector[{idx}]" )).into());
        }
        vector.push(
            T::from_json_number(value)
                .map_err(|error| error.at_field(&format!("vector[{idx}]")))?,
        );
    }
    let payload = obj.get("payload").cloned().unwrap_or(Value::Null);
    triviumdb::document::validate_node_import_payload(&payload, lineno)?;
    let edges = match obj.get("edges") {
        Some(value) => {
            let arr = value.as_array().ok_or_else(|| {
                format!("第 {lineno} 行 edges 必须是数组 (Line {lineno}: edges must be an array)")
            })?;
            let mut edges = Vec::with_capacity(arr.len());
            for (idx, edge) in arr.iter().enumerate() {
                let edge_obj = edge
                    .as_object()
                    .ok_or_else(|| format!("第 {lineno} 行 edges[{idx}] 必须是对象 (Line {lineno}: edges[{idx}] must be an object)"))?;
                let target = edge_obj
                    .get("target")
                    .and_then(|t| t.as_u64())
                    .ok_or_else(|| format!("第 {lineno} 行 edges[{idx}].target 必须是非负整数 (Line {lineno}: edges[{idx}].target must be an unsigned integer)"))?;
                let label = match edge_obj.get("label") {
                    Some(label) => label
                        .as_str()
                        .ok_or_else(|| format!("第 {lineno} 行 edges[{idx}].label 必须是字符串 (Line {lineno}: edges[{idx}].label must be a string)"))?
                        .to_string(),
                    None => String::new(),
                };
                let weight = match edge_obj.get("weight") {
                    Some(weight) => {
                        let value = weight.as_f64().ok_or_else(|| {
                            format!("第 {lineno} 行 edges[{idx}].weight 必须是数字 (Line {lineno}: edges[{idx}].weight must be numeric)")
                        })? as f32;
                        if !value.is_finite() {
                            return Err(format!(
                                "第 {lineno} 行 edges[{idx}].weight 超出有限 f32 范围 (Line {lineno}: edges[{idx}].weight exceeds the finite f32 range)"
                            ).into());
                        }
                        value
                    }
                    None => 1.0,
                };
                let metadata = edge_obj.get("metadata").cloned().unwrap_or(Value::Null);
                edges.push(Edge {
                    target_id: target,
                    label,
                    weight,
                    metadata,
                });
            }
            edges
        }
        None => Vec::new(),
    };

    Ok(Record {
        id,
        vector,
        payload,
        edges,
    })
}
