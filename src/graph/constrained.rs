//! GraphFirst 候选集合内的精确向量排名门面。
//!
//! 图模式先产生合法 anchor，本模块负责维度、候选预算与节点存在性检查，再委托 exact
//! Top-K 执行器评分。它绝不从全库补充近邻，因此图约束是硬边界；候选超过预算时在
//! 分配和打分前 fail-closed。

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::node::{NodeId, SearchHit};
use crate::storage::memtable::MemTable;

pub fn rank_within<T: VectorType>(
    db: &MemTable<T>,
    query: &[T],
    anchor_ids: &[NodeId],
    top_k: usize,
    max_anchor_nodes: usize,
) -> Result<Vec<SearchHit>> {
    db.ensure_readable()?;
    crate::vector::validate_query_vector(query, db.dim())?;
    if top_k == 0 || max_anchor_nodes == 0 {
        return Err(TriviumError::InvalidInput(
            "GraphFirst top_k 和 max_anchor_nodes 必须大于 0".into(),
        ));
    }
    let mut workspace = db.reserve_instance_workspace(0)?;
    let mut anchors = std::collections::BTreeSet::new();
    let mut admitted = 0usize;
    for &id in anchor_ids {
        crate::request::check_current()?;
        if anchors.contains(&id) {
            continue;
        }
        if anchors.len() == max_anchor_nodes {
            return Err(TriviumError::QueryExecution(format!(
                "GraphFirst 唯一候选超过预算 {max_anchor_nodes} (GraphFirst unique anchor limit exceeded)"
            )));
        }
        if anchors.len() == admitted {
            admitted = admitted.saturating_mul(2).max(1).min(max_anchor_nodes);
            // 首个树节点及后续分裂的空间随唯一候选数增长。
            db.resize_instance_workspace(
                &mut workspace,
                128usize.saturating_add(admitted.saturating_mul(32)),
            )?;
        }
        anchors.insert(id);
    }
    crate::index::exact::rank_unique_ids(db, query, anchors.iter().copied(), top_k)
}
