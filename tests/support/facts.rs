//! 通过公开读取接口采集节点和完整出边；整数指纹始终按原生整数比对。
use serde_json::{Value, json};
use triviumdb::{Database, VectorType};

pub fn nodes<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    database: &Database<T>,
) -> Value {
    let mut ids = database.all_node_ids().unwrap();
    assert_eq!(database.node_count().unwrap(), ids.len());
    ids.sort_unstable();
    let nodes: Vec<Value> = ids
        .into_iter()
        .map(|id| {
            let node = database.get(id).unwrap().unwrap();
            assert_eq!(node.id, id);
            let mut edges = node.edges;
            edges.sort_by(|left, right| {
                (left.target_id, &left.label).cmp(&(right.target_id, &right.label))
            });
            let edges: Vec<Value> = edges
                .iter()
                .map(|edge| {
                    json!({
                        "target": edge.target_id,
                        "label": edge.label,
                        "weight": edge.weight,
                        "metadata": edge.metadata,
                    })
                })
                .collect();
            json!({
                "id": id,
                "vector": node.vector.into_iter().map(|value| value.to_json_number().unwrap()).collect::<Vec<_>>(),
                "payload": node.payload,
                "edges": edges,
            })
        })
        .collect();
    json!(nodes)
}

/// 历史共享样例中用小数形式书写的整数向量，按目标 dtype 解读。
#[allow(dead_code)] // 仅复用实际事实采集的测试入口无需转换样例。
pub fn fixture_nodes<T: VectorType>(mut nodes: Value) -> Value {
    if std::any::TypeId::of::<T>() == std::any::TypeId::of::<u64>() {
        for node in nodes.as_array_mut().unwrap() {
            for value in node["vector"].as_array_mut().unwrap() {
                if value.as_u64().is_none() {
                    let n = value.as_f64().unwrap();
                    assert!(
                        n.fract() == 0. && (0.0..9_007_199_254_740_992.0).contains(&n),
                        "大整数预期须在样例中使用整数字面量：{value}"
                    );
                    *value = json!(n as u64);
                }
            }
        }
    }
    nodes
}
