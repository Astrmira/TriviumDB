use super::{publish, support::TestDir};
use triviumdb::{GenerationStore, TriviumError};

use crate::physical_files::files as artifacts;

#[cfg(windows)]
#[test]
fn windows代际大小写别名共享当前代保护和旧代租约() {
    let parent = TestDir::new("generation_case_alias");
    let store = GenerationStore::new(&parent.0);
    publish(&store, "April", 1.);
    store.publish_current("April", "data.tdb").unwrap();
    let directory = parent.0.join("April");
    let before = artifacts(&directory);
    assert!(same_file::is_same_file(&directory, parent.0.join("april")).unwrap());
    assert!(matches!(
        store.reclaim_generation("april"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    assert!(matches!(
        store.reclaim_generation("April/"),
        Err(TriviumError::InvalidInput(_))
    ));
    assert_eq!(store.resolve_current().unwrap().generation_id, "April");
    assert_eq!(artifacts(&directory), before);

    let reader = store.open_current::<f32>(2).unwrap();
    publish(&store, "May", 0.);
    store.publish_current("May", "data.tdb").unwrap();
    assert!(matches!(
        store.reclaim_generation("april"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    assert_eq!(artifacts(&directory), before);
    assert_eq!(
        reader.get(1).unwrap().unwrap().payload["generation"],
        "April"
    );
    drop(reader);
    store.reclaim_generation("april").unwrap();
    assert!(!directory.exists());
    let current = store.open_current::<f32>(2).unwrap();
    assert_eq!(
        current.get(1).unwrap().unwrap().payload["generation"],
        "May"
    );
}

#[test]
fn 根目录创建前后构造的默认store共享reader租约() {
    let parent = TestDir::new("generation_identity");
    let root = std::path::PathBuf::from(parent.file("代际 数据"));
    let before = GenerationStore::new(&root);
    publish(&before, "old", 1.);
    before.publish_current("old", "data.tdb").unwrap();
    let reader = before.open_current::<f32>(2).unwrap();
    let old_files = artifacts(&root.join("old"));
    let after = GenerationStore::new(root.canonicalize().unwrap());
    publish(&after, "new", 0.);
    after.publish_current("new", "data.tdb").unwrap();
    assert!(matches!(
        after.reclaim_generation("old"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    assert!(root.join("old/data.tdb").exists());
    assert_eq!(artifacts(&root.join("old")), old_files);
    assert_eq!(reader.get(1).unwrap().unwrap().payload["generation"], "old");
    drop(reader);
    after.reclaim_generation("old").unwrap();
    assert!(!root.join("old").exists());
}

#[test]
fn 同一物理根目录的路径别名和显式运行目录共享租约() {
    for custom in [false, true] {
        let parent = TestDir::new("generation_alias");
        let root = parent.0.join("store");
        let runtime = parent.0.join("runtime");
        let first = if custom {
            GenerationStore::with_runtime_dir(&root, &runtime)
        } else {
            GenerationStore::new(&root)
        };
        publish(&first, "old", 1.);
        first.publish_current("old", "data.tdb").unwrap();
        let reader = first.open_current::<f32>(2).unwrap();
        let old_files = artifacts(&root.join("old"));
        let mut aliases = vec![root.join("."), root.canonicalize().unwrap()];
        #[cfg(windows)]
        aliases.push(std::path::PathBuf::from(
            root.to_str().unwrap().to_uppercase(),
        ));
        #[cfg(unix)]
        {
            let link = parent.0.join("symlink");
            std::os::unix::fs::symlink(&root, &link).unwrap();
            aliases.push(link);
        }
        for (index, alias) in aliases.drain(..).enumerate() {
            let second = if custom {
                GenerationStore::with_runtime_dir(alias, &runtime)
            } else {
                GenerationStore::new(alias)
            };
            let generation = format!("new-{index}");
            publish(&second, &generation, 0.);
            second.publish_current(&generation, "data.tdb").unwrap();
            assert!(matches!(
                second.reclaim_generation("old"),
                Err(TriviumError::GenerationBusy { .. })
            ));
            assert_eq!(artifacts(&root.join("old")), old_files);
        }
        assert_eq!(reader.get(1).unwrap().unwrap().payload["generation"], "old");
    }
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn 默认租约跨进程和构造后工作目录变化仍保护旧代() {
    let parent = TestDir::new("generation_default_process");
    let root = parent.0.join("store");
    let elsewhere = parent.0.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let before = GenerationStore::new(&root);
    publish(&before, "generation-1", 1.);
    before.publish_current("generation-1", "data.tdb").unwrap();
    let ready = parent.file("ready");
    let mut child = Child(
        super::generation_child_command()
            .args(["--exact", "generation跨进程租约辅助进程", "--nocapture"])
            .current_dir(&parent.0)
            .env("TRIVIUM_GENERATION_CHILD_ROOT", "store")
            .env_remove("TRIVIUM_GENERATION_CHILD_RUNTIME")
            .env("TRIVIUM_GENERATION_CHILD_CWD", &elsewhere)
            .env("TRIVIUM_GENERATION_CHILD_READY", &ready)
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !std::path::Path::new(&ready).exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "子进程在取得租约前退出"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "子进程未在期限内取得租约"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(std::fs::read(&ready).unwrap(), b"generation-1");
    let old_files = artifacts(&root.join("generation-1"));
    publish(&before, "generation-2", 0.);
    before.publish_current("generation-2", "data.tdb").unwrap();
    assert!(matches!(
        before.reclaim_generation("generation-1"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    assert_eq!(artifacts(&root.join("generation-1")), old_files);
    drop(child);
    before.reclaim_generation("generation-1").unwrap();
    assert!(!root.join("generation-1").exists());
}

#[cfg(windows)]
#[test]
fn 文件解除占用后可由新的代际管理器继续回收() {
    use std::os::windows::fs::OpenOptionsExt;
    let parent = TestDir::new("generation_reclaim_resume");
    let store = GenerationStore::new(&parent.0);
    for generation in ["old", "current"] {
        publish(&store, generation, 1.);
        store.publish_current(generation, "data.tdb").unwrap();
    }
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(parent.0.join("old/data.tdb"))
        .unwrap();
    assert!(matches!(
        store.reclaim_generation("old"),
        Err(TriviumError::Io(_))
    ));
    assert!(matches!(
        store.prepare_generation("old", "data.tdb"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    drop(held);
    let reopened = GenerationStore::new(&parent.0);
    reopened.reclaim_generation("old").unwrap();
    reopened.reclaim_generation("old").unwrap();
    assert!(!parent.0.join("old").exists());
    assert_eq!(
        reopened
            .open_current::<f32>(2)
            .unwrap()
            .get(1)
            .unwrap()
            .unwrap()
            .payload["generation"],
        "current"
    );
}

#[cfg(feature = "test-hooks")]
#[test]
fn 旧代部分删除后重建管理器可完成回收并复用名称() {
    let parent = TestDir::new("generation_resume_after_remove");
    let store = GenerationStore::new(&parent.0);
    for generation in ["g1", "g2"] {
        publish(&store, generation, 1.);
        store.publish_current(generation, "data.tdb").unwrap();
    }
    let current = artifacts(&parent.0.join("g2"));
    {
        use triviumdb::test_hooks::{IoPoint, fail_io_at};
        let _fault = fail_io_at(IoPoint::GenerationArtifactRemoved);
        assert!(matches!(
            store.reclaim_generation("g1"),
            Err(TriviumError::Io(_))
        ));
    }
    let reopened = GenerationStore::new(&parent.0);
    assert!(matches!(
        reopened.prepare_generation("g1", "data.tdb"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    reopened.reclaim_generation("g1").unwrap();
    assert_eq!(artifacts(&parent.0.join("g2")), current);
    publish(&reopened, "g1", 0.);
    reopened.publish_current("g1", "data.tdb").unwrap();
    assert_eq!(
        reopened
            .open_current::<f32>(2)
            .unwrap()
            .get(1)
            .unwrap()
            .unwrap()
            .vector,
        vec![0., 1.]
    );
}
