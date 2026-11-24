//! TQL、Prepared 参数与查询结果的语言适配。

use super::arguments::{extract_argument, extract_management_argument};
use super::database::{PyTriviumDB, database_read, database_write};
use super::errors::{argument_input_error, attach_error_info, to_py_error, write_admission_error};
use super::json::{json_to_pyobject, pyobject_to_json};
use super::types::PyQueryRow;
use pyo3::prelude::*;
use pyo3::types::PyDict;

fn collect_prepared_parameters(
    parameters: &Bound<'_, PyAny>,
    prepared: &crate::query::tql_prepared::PreparedTql,
) -> PyResult<std::collections::HashMap<String, crate::query::tql_prepared::TqlParamValue>> {
    let py = parameters.py();
    let mut values = std::collections::HashMap::new();
    let mut insert = |key: Bound<'_, PyAny>, value: Bound<'_, PyAny>| -> PyResult<()> {
        let name = key.extract::<String>().map_err(|error| {
            attach_error_info(
                error,
                crate::TriviumError::InvalidInput(
                    "参数映射键必须是字符串 (Parameter mapping keys must be strings)".into(),
                )
                .at_field("parameters"),
            )
        })?;
        let json = pyobject_to_json(&value).map_err(|error| {
            attach_error_info(
                error,
                prepared.locate_parameter_error(
                    crate::TriviumError::InvalidInput(
                        "参数值无法转换为 JSON (Parameter value cannot be converted to JSON)"
                            .into(),
                    )
                    .at_query_parameter(&name),
                ),
            )
        })?;
        let value = prepared
            .parameter_from_json(&name, &json)
            .map_err(to_py_error)?;
        values.insert(name, value);
        Ok(())
    };
    if let Ok(dictionary) = parameters.cast::<PyDict>() {
        for (key, value) in dictionary.iter() {
            insert(key, value)?;
        }
    } else {
        let mapping = py.import("collections.abc")?.getattr("Mapping")?;
        if !parameters.is_instance(&mapping)? {
            let reason = "Prepared TQL 参数必须是映射 (Prepared TQL parameters must be a mapping)";
            return Err(attach_error_info(
                pyo3::exceptions::PyTypeError::new_err(reason),
                crate::TriviumError::InvalidInput(reason.into()).at_field("parameters"),
            ));
        }
        // Mapping 的 items、迭代与取值可执行宿主代码，异常沿原路径传播。
        for pair in parameters.call_method0("items")?.try_iter()? {
            let (key, value) = pair?.extract::<(Bound<'_, PyAny>, Bound<'_, PyAny>)>()?;
            insert(key, value)?;
        }
    }
    Ok(values)
}

fn convert_rows<T: crate::VectorType>(
    py: Python<'_>,
    rows: crate::query::tql_executor::TqlValueResult<T>,
) -> PyResult<Vec<PyQueryRow>> {
    use crate::query::tql_executor::TqlValue;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let py_row = PyDict::new(py);
        for (name, value) in row {
            match value {
                TqlValue::Node(node) => {
                    let node_dict = PyDict::new(py);
                    node_dict.set_item("id", node.id)?;
                    node_dict.set_item("payload", json_to_pyobject(py, &node.payload)?)?;
                    node_dict.set_item("num_edges", node.edges.len())?;
                    py_row.set_item(name, node_dict)?;
                }
                TqlValue::Edge(edge) => {
                    let edge_dict = PyDict::new(py);
                    edge_dict.set_item("type", "edge")?;
                    edge_dict.set_item("source", edge.source_id)?;
                    if let Some(graph_id) = &edge.graph_id {
                        edge_dict.set_item("graph_id", graph_id)?;
                    }
                    edge_dict.set_item("target", edge.target_id)?;
                    edge_dict.set_item("label", edge.label)?;
                    edge_dict.set_item("weight", edge.weight)?;
                    edge_dict.set_item("metadata", json_to_pyobject(py, &edge.metadata)?)?;
                    py_row.set_item(name, edge_dict)?;
                }
                TqlValue::Int(value) => {
                    py_row.set_item(name, value)?;
                }
                TqlValue::UInt(value) => {
                    py_row.set_item(name, value)?;
                }
                TqlValue::Float(value) => {
                    py_row.set_item(name, value)?;
                }
                TqlValue::String(value) => {
                    py_row.set_item(name, value)?;
                }
                TqlValue::Bool(value) => {
                    py_row.set_item(name, value)?;
                }
                TqlValue::Path(value) => {
                    py_row.set_item(name, value)?;
                }
                TqlValue::List(value) => {
                    py_row.set_item(
                        name,
                        json_to_pyobject(py, &serde_json::Value::Array(value))?,
                    )?;
                }
                TqlValue::Object(value) => {
                    py_row.set_item(
                        name,
                        json_to_pyobject(py, &serde_json::Value::Object(value))?,
                    )?;
                }
                TqlValue::Null => {
                    py_row.set_item(name, py.None())?;
                }
            }
        }
        out.push(PyQueryRow {
            row: py_row.into_any().unbind(),
        });
    }
    Ok(out)
}

#[pyclass(name = "PreparedTql")]
pub struct PyPreparedTql {
    inner: crate::query::tql_prepared::PreparedTql,
}

#[pymethods]
impl PyPreparedTql {
    fn parameter_names(&self) -> Vec<String> {
        self.inner
            .parameter_names()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
}

#[pymethods]
impl PyTriviumDB {
    /// 执行 TQL (Trivium Query Language) 统一查询
    ///
    /// 支持三种入口：MATCH (图遍历) / FIND (文档过滤) / SEARCH (向量检索)
    ///
    /// 示例：
    /// ```python
    /// # 图遍历
    /// rows = db.tql('MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b')
    /// for row in rows:
    ///     node = row.row["b"]   # {"id": ..., "payload": {...}}
    ///
    /// # 文档过滤
    /// rows = db.tql('FIND {type: "event", heat: {$gte: 0.7}} RETURN *')
    /// ```
    fn tql(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query: Bound<'_, PyAny>,
    ) -> PyResult<Vec<PyQueryRow>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let query: String = extract_argument(py, &query, "query", None)?;
        dispatch!(this, db => py.detach(|| db.tql_values(&query))
            .map_err(to_py_error).and_then(|rows| convert_rows(py, rows)))
    }

    fn prepare_tql(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query: Bound<'_, PyAny>,
    ) -> PyResult<PyPreparedTql> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let query: String = extract_argument(py, &query, "query", None)?;
        dispatch!(this, db => py.detach(|| db.prepare_tql(&query)))
            .map(|inner| PyPreparedTql { inner })
            .map_err(to_py_error)
    }

    fn execute_prepared_tql(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        prepared: Bound<'_, PyAny>,
        parameters: &Bound<'_, PyAny>,
    ) -> PyResult<Vec<PyQueryRow>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let prepared = prepared
            .extract::<PyRef<'_, PyPreparedTql>>()
            .map_err(|error| argument_input_error(error.into(), "prepared", None))?;
        let values = collect_prepared_parameters(parameters, &prepared.inner)?;
        let plan = &prepared.inner;
        dispatch!(this, db => py.detach(|| db.execute_prepared_tql(plan, &values))
            .map_err(to_py_error).and_then(|rows| convert_rows(py, rows)))
    }

    /// 执行 TQL 写操作（CREATE / SET / DELETE / DETACH DELETE）
    ///
    /// 返回 dict: {"affected": int, "created_ids": list[int]}
    ///
    /// 示例：
    /// ```python
    /// result = db.tql_mut('CREATE (a {name: "Alice", age: 30})')
    /// print(result["affected"])      # 1
    /// print(result["created_ids"])   # [1]
    ///
    /// db.tql_mut('MATCH (a {name: "Alice"}) SET a.age == 31')
    /// db.tql_mut('MATCH (a {name: "Alice"}) DELETE a')
    /// ```
    fn tql_mut(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query: Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let query: String = extract_management_argument(py, &query, "query")?;
        let result =
            dispatch!(this, mut db => py.detach(|| db.tql_mut(&query))).map_err(to_py_error)?;
        let response_error = |err| {
            super::errors::python_write_error(err, dispatch!(this, db => db.response_write_info()))
        };
        let dict = PyDict::new(py);
        dict.set_item("affected", result.affected)
            .map_err(response_error)?;
        dict.set_item("created_ids", result.created_ids)
            .map_err(response_error)?;
        Ok(dict.into_any().unbind())
    }
}
