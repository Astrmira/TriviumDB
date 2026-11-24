use super::expression::RefValue;
use std::collections::BTreeMap;
use triviumdb::query::tql_executor::TqlValue;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanonicalValue {
    Null,
    Bool(bool),
    Integer(i64),
    Unsigned(u64),
    Float(u64),
    String(String),
    Node {
        id: u64,
        vector: Vec<u32>,
        payload: Box<CanonicalValue>,
        edges: Vec<CanonicalValue>,
    },
    Edge {
        graph_id: Option<String>,
        source: u64,
        target: u64,
        label: String,
        weight_bits: u32,
        metadata: Box<CanonicalValue>,
    },
    Path(Vec<u64>),
    List(Vec<CanonicalValue>),
    Object(BTreeMap<String, CanonicalValue>),
}

pub type CanonicalValueRow = BTreeMap<String, CanonicalValue>;

fn json_value(value: &serde_json::Value) -> CanonicalValue {
    match value {
        serde_json::Value::Null => CanonicalValue::Null,
        serde_json::Value::Bool(value) => CanonicalValue::Bool(*value),
        serde_json::Value::Number(value) => value.as_i64().map_or_else(
            || {
                value.as_u64().map_or_else(
                    || CanonicalValue::Float(value.as_f64().unwrap().to_bits()),
                    CanonicalValue::Unsigned,
                )
            },
            CanonicalValue::Integer,
        ),
        serde_json::Value::String(value) => CanonicalValue::String(value.clone()),
        serde_json::Value::Array(values) => {
            CanonicalValue::List(values.iter().map(json_value).collect())
        }
        serde_json::Value::Object(value) => CanonicalValue::Object(
            value
                .iter()
                .map(|(k, v)| (k.clone(), json_value(v)))
                .collect(),
        ),
    }
}

pub fn from_tql_value(value: &TqlValue<f32>) -> CanonicalValue {
    match value {
        TqlValue::Node(node) => {
            let mut edges = node
                .edges
                .iter()
                .map(|edge| CanonicalValue::Edge {
                    graph_id: None,
                    source: node.id,
                    target: edge.target_id,
                    label: edge.label.clone(),
                    weight_bits: edge.weight.to_bits(),
                    metadata: Box::new(json_value(&edge.metadata)),
                })
                .collect::<Vec<_>>();
            edges.sort();
            CanonicalValue::Node {
                id: node.id,
                vector: node.vector.iter().map(|v| v.to_bits()).collect(),
                payload: Box::new(json_value(&node.payload)),
                edges,
            }
        }
        TqlValue::Edge(edge) => CanonicalValue::Edge {
            graph_id: edge.graph_id.clone(),
            source: edge.source_id,
            target: edge.target_id,
            label: edge.label.clone(),
            weight_bits: edge.weight.to_bits(),
            metadata: Box::new(json_value(&edge.metadata)),
        },
        TqlValue::Int(value) => CanonicalValue::Integer(*value),
        TqlValue::UInt(value) => CanonicalValue::Unsigned(*value),
        TqlValue::Float(value) => CanonicalValue::Float(value.to_bits()),
        TqlValue::String(value) => CanonicalValue::String(value.clone()),
        TqlValue::Bool(value) => CanonicalValue::Bool(*value),
        TqlValue::Path(value) => CanonicalValue::Path(value.clone()),
        TqlValue::List(value) => CanonicalValue::List(value.iter().map(json_value).collect()),
        TqlValue::Object(value) => CanonicalValue::Object(
            value
                .iter()
                .map(|(k, v)| (k.clone(), json_value(v)))
                .collect(),
        ),
        TqlValue::Null => CanonicalValue::Null,
    }
}

/// 预期节点取自写入样例，出边和向量按该样例的构造约定生成。
pub fn expected_node(reference: &super::model::RefDatabase, id: u64) -> CanonicalValue {
    let edges = reference
        .edges
        .iter()
        .filter(|edge| edge.source == id)
        .map(|edge| CanonicalValue::Edge {
            graph_id: None,
            source: id,
            target: edge.target,
            label: edge.label.clone(),
            weight_bits: 1f32.to_bits(),
            metadata: Box::new(CanonicalValue::Null),
        })
        .collect();
    CanonicalValue::Node {
        id,
        vector: [id as f32, 1., 0.].map(f32::to_bits).to_vec(),
        payload: Box::new(json_value(&reference.nodes[&id].payload)),
        edges,
    }
}

pub fn from_reference(value: &RefValue) -> CanonicalValue {
    match value {
        RefValue::Missing | RefValue::Null => CanonicalValue::Null,
        RefValue::Bool(value) => CanonicalValue::Bool(*value),
        RefValue::Integer(value) => i64::try_from(*value)
            .map(CanonicalValue::Integer)
            .unwrap_or_else(|_| {
                CanonicalValue::Unsigned(u64::try_from(*value).expect("整数应属于 JSON 域"))
            }),
        RefValue::Float(value) => CanonicalValue::Float(value.to_bits()),
        RefValue::String(value) => CanonicalValue::String(value.clone()),
        RefValue::List(value) => CanonicalValue::List(value.iter().map(from_reference).collect()),
        RefValue::Path(value) => CanonicalValue::Path(value.clone()),
    }
}

pub fn canonicalize_rows(
    rows: Vec<std::collections::HashMap<String, TqlValue<f32>>>,
    ordered: bool,
) -> Vec<CanonicalValueRow> {
    let mut rows = rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(name, value)| (name, from_tql_value(&value)))
                .collect()
        })
        .collect::<Vec<_>>();
    if !ordered {
        rows.sort();
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical覆盖全部一等TQL值且无序行按多重集合比较() {
        let values = [
            TqlValue::Int(7),
            TqlValue::Float(1.5),
            TqlValue::String("x".into()),
            TqlValue::Bool(true),
            TqlValue::Path(vec![1, 2]),
            TqlValue::List(vec![serde_json::json!(1), serde_json::json!(null)]),
            TqlValue::Null,
        ];
        let converted = values.iter().map(from_tql_value).collect::<Vec<_>>();
        assert_eq!(
            converted,
            [
                CanonicalValue::Integer(7),
                CanonicalValue::Float(1.5f64.to_bits()),
                CanonicalValue::String("x".into()),
                CanonicalValue::Bool(true),
                CanonicalValue::Path(vec![1, 2]),
                CanonicalValue::List(vec![CanonicalValue::Integer(1), CanonicalValue::Null]),
                CanonicalValue::Null
            ]
        );
        assert_eq!(from_reference(&RefValue::Missing), CanonicalValue::Null);
    }

    #[test]
    fn 边的图身份权重和元数据分别参与差分() {
        // 两张图共享端点和标签，投影仍须保留每一条边的身份及独立属性。
        let root = crate::support::TestDir::new("canonical-edges");
        let mut db = triviumdb::Database::<f32>::open(&root.file("named.tdb"), 1).unwrap();
        for id in [7, 9] {
            db.insert_with_id(id, &[1.], serde_json::Value::Null)
                .unwrap();
        }
        let cases = [("A", 1f32, 3), ("B", 2f32, 4)];
        for (graph, weight, rank) in cases {
            let data = serde_json::json!({"graph_id":graph,"metadata":{},"members":["7","9"],"edges":[{
                "source_id":"7","target_id":"9","label":"rel","weight":weight,"metadata":{"rank":rank}
            }]});
            db.put_named_graph(serde_json::from_value(data).unwrap(), None)
                .unwrap();
        }
        let rows = db
            .tql_values("GRAPH {graph_ids:['A','B']} MATCH (a)-[e:rel]->(b) RETURN e")
            .unwrap();
        let mut expected = cases.map(|(graph, weight, rank)| {
            BTreeMap::from([(
                "e".into(),
                CanonicalValue::Edge {
                    graph_id: Some(graph.into()),
                    source: 7,
                    target: 9,
                    label: "rel".into(),
                    weight_bits: weight.to_bits(),
                    metadata: Box::new(CanonicalValue::Object(BTreeMap::from([(
                        "rank".into(),
                        CanonicalValue::Integer(rank),
                    )]))),
                },
            )])
        });
        expected.sort();
        assert_eq!(canonicalize_rows(rows, false), expected);
    }
}
