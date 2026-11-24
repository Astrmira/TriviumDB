#![allow(non_snake_case)]
//! 磁盘格式规格测试入口。规格模型独立于生产解析器，用字段级变异验证 loader 的边界与完整性契约。
#[path = "../support/artifact.rs"]
mod artifact;

mod cross_generation;
mod fixture;
mod mutation;
mod replay;
mod sidecar_spec;
mod snapshot_spec;
mod spec;
mod wal_spec;

#[path = "../support/cleanup.rs"]
mod cleanup_files;

#[path = "../support/files.rs"]
mod physical_files;
#[path = "../support/mod.rs"]
mod support;

#[path = "../support/read_only.rs"]
mod read_only;

#[path = "../support/facts.rs"]
mod node_facts;
