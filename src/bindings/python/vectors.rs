//! 向量的标量协议转换与连续二维缓冲快照。

use super::arguments::{
    float_value, half_value, integer_value, read_sequence, sequence_with_fast_path, write_list,
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

fn snapshot<T: Component>(
    view: &Bound<'_, PyMemoryView>,
    dim: usize,
    field: &str,
    write: Option<WriteErrorInfo>,
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
    let format: String = view
        .getattr("format")
        .and_then(|value| value.extract())
        .map_err(attribute_error)?;
    let native = if cfg!(target_endian = "little") {
        '<'
    } else {
        '>'
    };
    let format = format.strip_prefix(['@', '=', native]).unwrap_or(&format);
    let itemsize: usize = view
        .getattr("itemsize")
        .and_then(|value| value.extract())
        .map_err(attribute_error)?;
    if !(format == T::FORMAT || (T::FORMAT == "Q" && format == "L"))
        || itemsize != size_of::<T>()
        || !view
            .getattr("c_contiguous")
            .and_then(|value| value.is_truthy())
            .map_err(attribute_error)?
    {
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
    let row_bytes = columns.checked_mul(itemsize).ok_or_else(invalid)?;
    let total = rows.checked_mul(row_bytes).ok_or_else(invalid)?;
    if rows != 0 && columns == 0 {
        return Err(invalid());
    }
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

pub(super) fn queries<T: Component>(value: &Bound<'_, PyAny>, dim: usize) -> PyResult<Vec<Vec<T>>> {
    if let Ok(view) = value.cast::<PyMemoryView>() {
        return snapshot(view, dim, "query_vectors", None);
    }
    read_sequence(value, "query_vectors", |row, path| vector(row, path, None))
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
