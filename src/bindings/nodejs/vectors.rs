//! 批量向量输入在 JS 线程完成快照；后台任务只持有 Rust 数据。

use super::arguments::{read_array, validate_u64_component, vector_numbers_admitted, write_array};
use super::errors::argument_input_error;
use crate::VectorType;
use napi::{Env, JsTypedArray, JsTypedArrayValue, JsUnknown, NapiRaw, TypedArrayType};

pub(super) enum Batch {
    Numbers(Vec<Vec<f64>>),
    Float32(Vec<Vec<f32>>),
    Uint64(Vec<Vec<u64>>),
}

impl Batch {
    pub(super) fn read(
        env: Env,
        value: JsUnknown,
        dim: usize,
        dtype: &str,
        writing: bool,
    ) -> napi::Result<Self> {
        Self::read_admitted(env, value, dim, dtype, writing, |_| Ok(()))
    }

    pub(super) fn read_admitted(
        env: Env,
        value: JsUnknown,
        dim: usize,
        dtype: &str,
        writing: bool,
        mut admit: impl FnMut(usize) -> napi::Result<()>,
    ) -> napi::Result<Self> {
        let write = writing.then_some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION);
        let field = if writing { "vectors" } else { "queryVectors" };
        if !value.is_typedarray()? {
            let row = |env, item: JsUnknown, field: &str| {
                vector_numbers_admitted(
                    env,
                    item,
                    field,
                    writing && dtype == "u64",
                    write,
                    |cells| {
                        // JS 数组与目标 dtype 可同时驻留，长度取自执行本次读取的同一个数组。
                        admit(
                            cells
                                .max(4)
                                .saturating_mul(3 * size_of::<f64>())
                                .saturating_add(4 * size_of::<Vec<f64>>()),
                        )
                    },
                )
            };
            return (if writing {
                write_array(env, value, field, row)
            } else {
                read_array(env, value, field, row)
            })
            .map(Self::Numbers);
        }
        let array = JsTypedArray::try_from(value)?.into_value()?;
        let reject = |reason| argument_input_error(env, field, reason, write);
        let mut ordinary = false;
        // SAFETY: env 与缓冲区句柄均属于当前 JS 调用，仅询问 VM 是否为普通 ArrayBuffer。
        napi::check_status!(unsafe {
            napi::sys::napi_is_arraybuffer(env.raw(), array.arraybuffer.raw(), &mut ordinary)
        })?;
        if !ordinary || array.arraybuffer.is_detached()? {
            return Err(reject(
                "向量需要普通且已连接的 ArrayBuffer (Expected a non-shared, attached ArrayBuffer)",
            ));
        }
        if dim == 0 || !array.length.is_multiple_of(dim) {
            return Err(reject(
                "缓冲长度必须是向量维数的整数倍 (Buffer length must be a multiple of the dimension)",
            ));
        }
        let bounds = || reject("向量视图超出缓冲范围 (Vector view exceeds the backing buffer)");
        match (dtype, array.typedarray_type) {
            ("f32" | "f16", TypedArrayType::Float32) => copy_rows(
                array,
                dim,
                bounds,
                if dtype == "f16" { 2 } else { 0 },
                admit,
            )
            .map(Self::Float32),
            ("u64", TypedArrayType::BigUint64) => {
                copy_rows(array, dim, bounds, 0, admit).map(Self::Uint64)
            }
            _ => Err(reject(
                "缓冲区元素类型与数据库 dtype 不匹配 (Typed array does not match the database dtype)",
            )),
        }
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Numbers(rows) => rows.len(),
            Self::Float32(rows) => rows.len(),
            Self::Uint64(rows) => rows.len(),
        }
    }

    pub(super) fn into_f32(self) -> crate::Result<Vec<Vec<f32>>> {
        match self {
            Self::Float32(rows) => Ok(rows),
            Self::Numbers(rows) => Ok(rows
                .into_iter()
                .map(|row| row.into_iter().map(|v| v as f32).collect())
                .collect()),
            Self::Uint64(_) => Err(Self::dtype_error()),
        }
    }

    pub(super) fn into_half(self) -> crate::Result<Vec<Vec<half::f16>>> {
        match self {
            Self::Float32(rows) => Ok(rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|v| half::f16::from_f64_component(f64::from(v)))
                        .collect()
                })
                .collect()),
            Self::Numbers(rows) => Ok(rows
                .into_iter()
                .map(|row| row.into_iter().map(half::f16::from_f64_component).collect())
                .collect()),
            Self::Uint64(_) => Err(Self::dtype_error()),
        }
    }

    pub(super) fn into_u64(self, field: &str) -> crate::Result<Vec<Vec<u64>>> {
        match self {
            Self::Uint64(rows) => Ok(rows),
            Self::Numbers(rows) => rows
                .into_iter()
                .enumerate()
                .map(|(i, row)| {
                    row.into_iter()
                        .enumerate()
                        .map(|(j, v)| {
                            validate_u64_component(v)
                                .map_err(|e| e.at_field(&format!("/{field}/{i}/{j}")))
                        })
                        .collect()
                })
                .collect(),
            Self::Float32(_) => Err(Self::dtype_error()),
        }
    }

    fn dtype_error() -> crate::TriviumError {
        crate::TriviumError::InvalidInput("批次与数据库 dtype 不匹配 (Batch dtype mismatch)".into())
    }
}

fn copy_rows<T: bytemuck::Pod>(
    array: JsTypedArrayValue,
    dim: usize,
    invalid: impl Fn() -> napi::Error,
    converted_width: usize,
    admit: impl FnOnce(usize) -> napi::Result<()>,
) -> napi::Result<Vec<Vec<T>>> {
    if array.length == 0 {
        return Ok(Vec::new());
    }
    let row_bytes = dim.checked_mul(size_of::<T>()).ok_or_else(&invalid)?;
    let end = array
        .length
        .checked_mul(size_of::<T>())
        .and_then(|n| array.byte_offset.checked_add(n))
        .ok_or_else(&invalid)?;
    // 外部创建的普通 ArrayBuffer 可从未对齐的地址开始。只借用字节，在当前 JS 线程复制到对齐的 Rust 行缓冲。
    let buffer = array.arraybuffer.into_value()?;
    let bytes = buffer
        .as_ref()
        .get(array.byte_offset..end)
        .ok_or_else(invalid)?;
    let header_copies = if converted_width == 0 { 1 } else { 2 };
    admit(
        bytes
            .len()
            .saturating_add(array.length.saturating_mul(converted_width))
            .saturating_add(
                (array.length / dim).saturating_mul(header_copies * size_of::<Vec<T>>()),
            ),
    )?;
    Ok(bytes
        .chunks_exact(row_bytes)
        .map(bytemuck::allocation::pod_collect_to_vec)
        .collect())
}
