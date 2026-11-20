#![allow(non_snake_case)]
//! 按领域聚合的集成测试入口。

mod aggregation_tx;
mod proptest_query;
mod tql_dml;
mod tql_executor;
mod tql_index;
mod tql_parser;
mod tql_phase2;
mod tql_pipeline_parser;
mod tql_planner;
mod tql_reverse;

mod diagnostics;
mod match_boundaries;
mod parser_limits;
mod source_locations;

#[path = "../support/mod.rs"]
mod support;

/// 每次调用独占制品名称，多个测试进程和同名用例可以同时运行。
fn tmp_db(name: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!(
        "query_{}_{}_{name}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
    .to_str()
    .unwrap()
    .to_owned()
}

// Cargo 的仿真启动参数不会自动传给测试内部创建的子进程。
fn query_child_command() -> std::process::Command {
    let executable = std::env::current_exe().unwrap();
    if std::env::var_os("TRIVIUM_TEST_QEMU_AARCH64").is_some() {
        let mut command = std::process::Command::new("qemu-aarch64");
        command
            .arg("-L")
            .arg("/usr/aarch64-linux-gnu")
            .arg(executable);
        command
    } else {
        std::process::Command::new(executable)
    }
}

#[path = "../support/cleanup.rs"]
mod cleanup_files;
