//! 节点读取、批量写入与 CRUD 参数转换。

use super::arguments::{extract_argument, write_id, write_list, write_sequence, write_string};
use super::database::{DbBackend, PyTriviumDB, database_read, database_write};
use super::errors::{to_py_error, write_admission_error, write_input_error};
use super::json::{json_to_pyobject, write_json};
use super::types::{PyEdge, PyNodeView};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

#[pymethods]
impl PyTriviumDB {
    fn insert(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        vector: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<u64> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let json = write_json(payload, "payload")?;
        match &mut this.inner {
            DbBackend::F32(db) => {
                let vec: Vec<f32> = super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?;
                db.insert(&vec, json).map_err(to_py_error)
            }
            DbBackend::F16(db) => {
                let vec16: Vec<half::f16> = super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?;
                db.insert(&vec16, json).map_err(to_py_error)
            }
            DbBackend::U64(db) => {
                let vec: Vec<u64> = super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?;
                db.insert(&vec, json).map_err(to_py_error)
            }
        }
    }

    fn insert_with_id(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        id: Bound<'_, PyAny>,
        vector: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let json = write_json(payload, "payload")?;
        match &mut this.inner {
            DbBackend::F32(db) => {
                let vec: Vec<f32> = super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?;
                db.insert_with_id(id, &vec, json)
            }
            DbBackend::F16(db) => {
                let vec16: Vec<half::f16> = super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?;
                db.insert_with_id(id, &vec16, json)
            }
            DbBackend::U64(db) => {
                let vec: Vec<u64> = super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?;
                db.insert_with_id(id, &vec, json)
            }
        }
        .map_err(to_py_error)
    }

    fn upsert_with_id(
        slf: &Bound<'_, Self>,
        id: Bound<'_, PyAny>,
        vector: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let json = write_json(payload, "payload")?;
        match &mut this.inner {
            DbBackend::F32(db) => db.upsert_with_id(
                id,
                &super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?,
                json,
            ),
            DbBackend::F16(db) => {
                let vector = super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?;
                db.upsert_with_id(id, &vector, json)
            }
            DbBackend::U64(db) => db.upsert_with_id(
                id,
                &super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?,
                json,
            ),
        }
        .map_err(to_py_error)
    }

    fn delete_many_atomic(slf: &Bound<'_, Self>, ids: Bound<'_, PyAny>) -> PyResult<usize> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let ids = write_sequence(&ids, "ids", write_id)?;
        dispatch!(this, mut db => db.delete_many_atomic(&ids)).map_err(to_py_error)
    }

    fn compare_and_set_payload_field(
        slf: &Bound<'_, Self>,
        id: Bound<'_, PyAny>,
        field: Bound<'_, PyAny>,
        expected: &Bound<'_, PyAny>,
        replacement: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let field: String = write_string(&field, "field")?;
        let expected = write_json(expected, "expected")?;
        let replacement = write_json(replacement, "replacement")?;
        dispatch!(this, mut db => db.compare_and_set_payload_field(id, &field, &expected, replacement))
                .map_err(to_py_error)
    }

    fn delete(slf: &Bound<'_, Self>, id: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        dispatch!(this, mut db => db.delete(id)).map_err(to_py_error)
    }

    fn update_payload(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        id: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let json = write_json(payload, "payload")?;
        dispatch!(this, mut db => db.update_payload(id, json)).map_err(to_py_error)
    }

    /// 部分更新节点 Payload（$set / $inc / $unset）
    ///
    /// 只修改指定字段，其他字段保持不变。
    ///
    /// 示例：
    /// ```python
    /// db.patch_payload(id, {"$set": {"name": "Alice"}})
    /// db.patch_payload(id, {"$inc": {"visits": 1}})
    /// db.patch_payload(id, {"$unset": {"old_field": True}})
    /// ```
    fn patch_payload(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        id: Bound<'_, PyAny>,
        patch: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let json = write_json(patch, "patch")?;
        dispatch!(this, mut db => db.patch_payload(id, json)).map_err(to_py_error)
    }

    fn update_vector(
        slf: &Bound<'_, Self>,
        vector: Bound<'_, PyAny>,
        id: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        match &mut this.inner {
            DbBackend::F32(db) => {
                let vec: Vec<f32> = super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?;
                db.update_vector(id, &vec).map_err(to_py_error)
            }
            DbBackend::F16(db) => {
                let vec16: Vec<half::f16> = super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?;
                db.update_vector(id, &vec16).map_err(to_py_error)
            }
            DbBackend::U64(db) => {
                let vec: Vec<u64> = super::vectors::vector(
                    &vector,
                    "vector",
                    Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )?;
                db.update_vector(id, &vec).map_err(to_py_error)
            }
        }
    }

    // ════════ 轻量级单字段查询 ════════

    /// 获取节点的 payload（不含向量，比 get() 更轻量）
    fn get_payload(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        id: Bound<'_, PyAny>,
    ) -> PyResult<Option<Py<PyAny>>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        Ok({
            dispatch!(this, db => db.get_payload(id).map_err(to_py_error)?)
                .map(|p| json_to_pyobject(py, &p))
                .transpose()?
        })
    }

    fn get(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        id: Bound<'_, PyAny>,
    ) -> PyResult<Option<PyNodeView>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        match &this.inner {
            DbBackend::F32(db) => {
                if let Some(n) = db.get(id).map_err(to_py_error)? {
                    return Ok(Some(PyNodeView {
                        id: n.id,
                        vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                        payload: json_to_pyobject(py, &n.payload)?,
                        edges: n
                            .edges
                            .iter()
                            .map(|e| {
                                Ok(PyEdge {
                                    target_id: e.target_id,
                                    label: e.label.clone(),
                                    weight: f64::from(e.weight),
                                    metadata: json_to_pyobject(py, &e.metadata)?,
                                })
                            })
                            .collect::<PyResult<Vec<_>>>()?,
                        num_edges: n.edges.len(),
                    }));
                }
            }
            DbBackend::F16(db) => {
                if let Some(n) = db.get(id).map_err(to_py_error)? {
                    let f32_vec: Vec<f32> = n.vector.into_iter().map(|f| f.to_f32()).collect();
                    return Ok(Some(PyNodeView {
                        id: n.id,
                        vector: f32_vec.into_pyobject(py).unwrap().into_any().unbind(),
                        payload: json_to_pyobject(py, &n.payload)?,
                        edges: n
                            .edges
                            .iter()
                            .map(|e| {
                                Ok(PyEdge {
                                    target_id: e.target_id,
                                    label: e.label.clone(),
                                    weight: f64::from(e.weight),
                                    metadata: json_to_pyobject(py, &e.metadata)?,
                                })
                            })
                            .collect::<PyResult<Vec<_>>>()?,
                        num_edges: n.edges.len(),
                    }));
                }
            }
            DbBackend::U64(db) => {
                if let Some(n) = db.get(id).map_err(to_py_error)? {
                    return Ok(Some(PyNodeView {
                        id: n.id,
                        vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                        payload: json_to_pyobject(py, &n.payload)?,
                        edges: n
                            .edges
                            .iter()
                            .map(|e| {
                                Ok(PyEdge {
                                    target_id: e.target_id,
                                    label: e.label.clone(),
                                    weight: f64::from(e.weight),
                                    metadata: json_to_pyobject(py, &e.metadata)?,
                                })
                            })
                            .collect::<PyResult<Vec<_>>>()?,
                        num_edges: n.edges.len(),
                    }));
                }
            }
        }
        Ok(None)
    }

    fn batch_insert(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        vectors: Bound<'_, PyAny>,
        payloads: &Bound<'_, PyAny>,
    ) -> PyResult<Vec<u64>> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        dispatch!(borrowed, mut db => insert_rows(py, db, None, &vectors, payloads))
    }

    fn batch_insert_with_ids(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        ids: Bound<'_, PyAny>,
        vectors: Bound<'_, PyAny>,
        payloads: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let ids = write_sequence(&ids, "ids", write_id)?;
        dispatch!(borrowed, mut db => insert_rows(py, db, Some(&ids), &vectors, payloads))
            .map(|_| ())
    }
}

fn insert_rows<
    T: crate::VectorType + super::vectors::Component + serde::Serialize + serde::de::DeserializeOwned,
>(
    py: Python<'_>,
    db: &mut crate::database::Database<T>,
    ids: Option<&[u64]>,
    vectors: &Bound<'_, PyAny>,
    payloads: &Bound<'_, PyAny>,
) -> PyResult<Vec<u64>> {
    let mut vectors = super::vectors::WriteRows::<T>::new(vectors)?;
    let payloads = write_list(payloads, "payloads")?;
    let count = vectors.len();
    if count != payloads.len() {
        return Err(write_input_error(
            PyValueError::new_err(
                "vectors 与 payloads 长度必须相同 (vectors and payloads must have the same length)",
            ),
            "payloads",
        ));
    }
    if ids.is_some_and(|ids| ids.len() != count) {
        return Err(write_input_error(
            PyValueError::new_err(
                "ids 与 vectors 长度必须相同 (ids and vectors must have the same length)",
            ),
            "ids",
        ));
    }
    let mut tx = crate::database::TxBuilder::new();
    for i in 0..count {
        let index = i.to_string();
        let payload_field = crate::sdk::field_member("payloads", &index);
        let payload = payloads
            .get_item(i)
            .map_err(|e| write_input_error(e, &payload_field))?;
        let row = vectors.next_row(i, &crate::sdk::field_member("vectors", &index))?;
        let json = write_json(&payload, &payload_field)?;
        tx.insert_owned(ids.map(|ids| ids[i]), row, json);
    }
    py.detach(|| db.commit_tx(tx))
        .map_err(crate::sdk::direct_write_error)
        .map_err(to_py_error)
}
