//! 比对独占 ResourceRuntime 的共享工作区与对应实例的计量。
use triviumdb::Database;

pub(super) struct Workspace {
    shared: usize,
    resident: usize,
}

impl Workspace {
    pub(super) fn capture(db: &Database<f32>) -> Self {
        Self {
            shared: db.resource_stats().reserved_workspace_bytes,
            resident: db.estimated_memory(),
        }
    }

    pub(super) fn held(&self, db: &Database<f32>) -> usize {
        let shared = db
            .resource_stats()
            .reserved_workspace_bytes
            .checked_sub(self.shared)
            .unwrap();
        let instance = db.estimated_memory().checked_sub(self.resident).unwrap();
        assert!(
            shared > 0 && instance >= shared,
            "共享工作区增量 {shared}，实例增量 {instance}"
        );
        shared
    }

    pub(super) fn released(&self, db: &Database<f32>) {
        assert_eq!(db.resource_stats().reserved_workspace_bytes, self.shared);
    }
}
