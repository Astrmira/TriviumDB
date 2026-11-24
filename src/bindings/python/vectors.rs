//! 向量的标量协议转换与连续缓冲快照。

use super::arguments::{
    float_value, half_value, integer_value, sequence_items, sequence_with_fast_path, write_list,
};
use super::errors::argument_input_error;
use crate::error::WriteErrorInfo;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyFloat, PyInt, PyList, PyMemoryView};

pub(super) trait Component: bytemuck::Pod {
    const FORMAT: &'static str;
    fn builtin(value: &Bound<'_, PyAny>) -> Option<Self>;
    fn parse(
        value: &Bound<'_, PyAny>,
        field: &str,
        write: Option<WriteErrorInfo>,
    ) -> PyResult<Self>;
}

impl Component for f32 {
    const FORMAT: &'static str = "f";
    fn builtin(value: &Bound<'_, PyAny>) -> Option<Self> {
        value.cast_exact::<PyFloat>().ok().map(|v| v.value() as f32)
    }
    fn parse(
        value: &Bound<'_, PyAny>,
        field: &str,
        write: Option<WriteErrorInfo>,
    ) -> PyResult<Self> {
        float_value(value, field, write)
    }
}

impl Component for half::f16 {
    const FORMAT: &'static str = "e";
    fn builtin(value: &Bound<'_, PyAny>) -> Option<Self> {
        value
            .cast_exact::<PyFloat>()
            .ok()
            .map(|v| crate::VectorType::from_f64_component(v.value()))
    }
    fn parse(
        value: &Bound<'_, PyAny>,
        field: &str,
        write: Option<WriteErrorInfo>,
    ) -> PyResult<Self> {
        half_value(value, field, write)
    }
}

impl Component for u64 {
    const FORMAT: &'static str = "Q";
    fn builtin(value: &Bound<'_, PyAny>) -> Option<Self> {
        value.cast_exact::<PyInt>().ok()?.extract().ok()
    }
    fn parse(
        value: &Bound<'_, PyAny>,
        field: &str,
        write: Option<WriteErrorInfo>,
    ) -> PyResult<Self> {
        integer_value(value, field, write)
    }
}

pub(super) fn vector<T: Component>(
    value: &Bound<'_, PyAny>,
    field: &str,
    write: Option<WriteErrorInfo>,
) -> PyResult<Vec<T>> {
    sequence_with_fast_path(value, field, write, T::builtin, |item, path| {
        T::parse(item, path, write)
    })
}

fn native_contiguous<T: Component>(view: &Bound<'_, PyMemoryView>) -> PyResult<bool> {
    let format: String = view.getattr("format")?.extract()?;
    let native = if cfg!(target_endian = "little") {
        '<'
    } else {
        '>'
    };
    let format = format.strip_prefix(['@', '=', native]).unwrap_or(&format);
    Ok((format == T::FORMAT || (T::FORMAT == "Q" && format == "L"))
        && view.getattr("itemsize")?.extract::<usize>()? == size_of::<T>()
        && view.getattr("c_contiguous")?.is_truthy()?)
}

/// 匹配的单行缓冲只做一次 bytes 快照；其他序列继续按标量协议读取。
pub(super) fn with_query<T: Component + Sync + Send, R: Send>(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    operation: impl FnOnce(&[T]) -> crate::Result<R> + Send,
) -> PyResult<R> {
    const FIELD: &str = "query_vector";
    let argument_error = |error| argument_input_error(error, FIELD, None);
    let result = if let Ok(view) = value.cast::<PyMemoryView>()
        && view
            .getattr("ndim")
            .and_then(|value| value.extract::<usize>())
            .map_err(argument_error)?
            == 1
        && native_contiguous::<T>(view).map_err(argument_error)?
    {
        // bytes 的只读快照独立于宿主可变数组。快照对象留在调用栈，借出的切片只在本次调用使用。
        let snapshot = view
            .call_method0("tobytes")
            .map_err(argument_error)?
            .cast_into::<PyBytes>()
            .map_err(|error| argument_error(error.into()))?;
        let raw = snapshot.as_bytes();
        match bytemuck::try_cast_slice::<u8, T>(raw) {
            Ok(query) => py.detach(move || operation(query)),
            Err(_) => {
                let query: Vec<T> = bytemuck::allocation::pod_collect_to_vec(raw);
                py.detach(move || operation(&query))
            }
        }
    } else {
        let query = vector(value, FIELD, None)?;
        py.detach(move || operation(&query))
    };
    result.map_err(super::errors::to_py_error)
}

fn snapshot<T: Component>(
    view: &Bound<'_, PyMemoryView>,
    dim: usize,
    field: &str,
    write: Option<WriteErrorInfo>,
    admit: impl FnOnce(usize, usize) -> PyResult<()>,
) -> PyResult<Vec<Vec<T>>> {
    let invalid = || {
        argument_input_error(
            pyo3::exceptions::PyValueError::new_err(
                "需要与数据库 dtype 一致的本机字节序、C 连续二维向量 (Expected a C-contiguous native-endian matrix matching the database dtype)",
            ),
            field,
            write,
        )
    };
    let attribute_error = |error| argument_input_error(error, field, write);
    if !native_contiguous::<T>(view).map_err(attribute_error)? {
        return Err(invalid());
    }
    let (rows, columns): (usize, usize) = view
        .getattr("shape")
        .and_then(|value| value.extract())
        .map_err(attribute_error)?;
    if columns != dim {
        let error = crate::TriviumError::DimensionMismatch {
            expected: dim,
            got: columns,
        }
        .at_field(field);
        return Err(super::errors::to_py_error(match write {
            Some(write) => error.with_write_info(write),
            None => error,
        }));
    }
    let row_bytes = columns.checked_mul(size_of::<T>()).ok_or_else(invalid)?;
    let total = rows.checked_mul(row_bytes).ok_or_else(invalid)?;
    if rows != 0 && columns == 0 {
        return Err(invalid());
    }
    admit(rows, total)?;
    // abi3-py39 通过只读 bytes 快照访问缓冲；释放 GIL 或转换 Payload 时宿主可重新使用原数组。
    let bytes = view
        .call_method0("tobytes")
        .map_err(attribute_error)?
        .cast_into::<PyBytes>()
        .map_err(|_| invalid())?;
    if bytes.as_bytes().len() != total {
        return Err(invalid());
    }
    Ok(bytes
        .as_bytes()
        .chunks_exact(row_bytes.max(1))
        .map(bytemuck::allocation::pod_collect_to_vec)
        .collect())
}

pub(super) fn queries<
    T: Component + crate::VectorType + serde::Serialize + serde::de::DeserializeOwned,
>(
    value: &Bound<'_, PyAny>,
    db: &crate::Database<T>,
) -> PyResult<(Vec<Vec<T>>, crate::resources::RequestWorkspace)> {
    const FIELD: &str = "query_vectors";
    let dim = db.dim();
    let reserve = |bytes| {
        db.reserve_request_workspace(bytes)
            .map_err(super::errors::to_py_error)
    };
    let mut workspace = reserve(0)?;
    if let Ok(view) = value.cast::<PyMemoryView>() {
        let mut temporary = None;
        let rows = snapshot(view, dim, FIELD, None, |rows, bytes| {
            temporary = Some(reserve(bytes)?);
            workspace.absorb(reserve(
                bytes.saturating_add(rows.saturating_mul(size_of::<Vec<T>>())),
            )?);
            Ok(())
        })?;
        drop(temporary);
        return Ok((rows, workspace));
    }
    let mut rows = Vec::new();
    for (i, item) in sequence_items(value, FIELD, None)?.enumerate() {
        let row = item?;
        let field = crate::sdk::field_member(FIELD, &i.to_string());
        let items = sequence_items(&row, &field, None)?;
        workspace.absorb(reserve(
            dim.saturating_mul(size_of::<T>())
                .saturating_add(4 * size_of::<Vec<T>>()),
        )?);
        let mut parsed = Vec::new();
        parsed.try_reserve_exact(dim).map_err(|error| {
            super::errors::to_py_error(
                crate::TriviumError::CapacityAllocationFailed {
                    reason: format!("向量行分配失败 (Vector row allocation failed): {error}"),
                }
                .at_field(&field),
            )
        })?;
        let mismatch = |got| {
            super::errors::to_py_error(
                crate::TriviumError::DimensionMismatch { expected: dim, got }.at_field(&field),
            )
        };
        for item in items {
            if parsed.len() == dim {
                return Err(mismatch(dim + 1));
            }
            let item = item?;
            let component = match T::builtin(&item) {
                Some(value) => value,
                None => T::parse(
                    &item,
                    &crate::sdk::field_member(&field, &parsed.len().to_string()),
                    None,
                )?,
            };
            parsed.push(component);
        }
        if parsed.len() != dim {
            return Err(mismatch(parsed.len()));
        }
        rows.push(parsed);
    }
    Ok((rows, workspace))
}

pub(super) enum WriteRows<'py, T> {
    List(Bound<'py, PyList>),
    Matrix(std::vec::IntoIter<Vec<T>>),
}
impl<'py, T: Component> WriteRows<'py, T> {
    pub(super) fn new(value: &Bound<'py, PyAny>, dim: usize) -> PyResult<Self> {
        if let Ok(view) = value.cast::<PyMemoryView>() {
            return Ok(Self::Matrix(
                snapshot(
                    view,
                    dim,
                    "vectors",
                    Some(WriteErrorInfo::MAINTENANCE_VALIDATION),
                    |_, _| Ok(()),
                )?
                .into_iter(),
            ));
        }
        write_list(value, "vectors").map(Self::List)
    }
    pub(super) fn len(&self) -> usize {
        match self {
            Self::List(rows) => rows.len(),
            Self::Matrix(rows) => rows.len(),
        }
    }
    pub(super) fn next_row(&mut self, index: usize, field: &str) -> PyResult<Vec<T>> {
        let write = Some(WriteErrorInfo::MAINTENANCE_VALIDATION);
        match self {
            Self::List(list) => vector(
                &list
                    .get_item(index)
                    .map_err(|e| argument_input_error(e, field, write))?,
                field,
                write,
            ),
            Self::Matrix(rows) => rows.next().ok_or_else(|| {
                argument_input_error(
                    pyo3::exceptions::PyIndexError::new_err(
                        "向量行超出范围 (Vector row out of bounds)",
                    ),
                    field,
                    write,
                )
            }),
        }
    }
}
