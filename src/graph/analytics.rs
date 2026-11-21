//! 面向 TQL NodeSet 的确定性、有预算图结构分析算法。
//!
//! 所有算法只读取调用方给定的诱导子图；节点与邻居均按 NodeId 排序。
//! 构图先准入节点目录，再逐边检查扫描额度与邻接表增长。

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::graph::view::GraphRead;
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct GraphWorkspace {
    pub directed: BTreeMap<NodeId, Vec<NodeId>>,
    pub reverse: BTreeMap<NodeId, Vec<NodeId>>,
    pub undirected: BTreeMap<NodeId, Vec<NodeId>>,
    pub weighted: BTreeMap<NodeId, Vec<(NodeId, f64)>>,
    pub examined_edges: usize,
}

pub fn build_workspace<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    label_filter: Option<&str>,
    max_examined_edges: usize,
    max_bytes: usize,
) -> Result<GraphWorkspace> {
    build_workspace_view(
        mt,
        nodes,
        label_filter,
        max_examined_edges,
        max_bytes,
        false,
    )
}

pub(crate) fn build_workspace_view<T: VectorType>(
    mt: &impl GraphRead<T>,
    nodes: &BTreeSet<NodeId>,
    label_filter: Option<&str>,
    max_examined_edges: usize,
    max_bytes: usize,
    require_nonnegative_weights: bool,
) -> Result<GraphWorkspace> {
    let control = crate::request::control().unwrap_or_default();
    control.check()?;
    // 四张 B 树都拥有键、向量头和分支。每个树节点最多 11 个键和 12 个子指针，
    // 除根以外至少含 5 个键，按最疏的内部节点为所有层预留空间。
    let map_node_bytes = 11 * size_of::<(NodeId, Vec<NodeId>)>() + 16 * size_of::<usize>();
    let tree_nodes = if nodes.is_empty() {
        0
    } else {
        1 + (nodes.len() - 1) / 5
    };
    let base_bytes = tree_nodes
        .checked_mul(4 * map_node_bytes)
        .ok_or_else(|| TriviumError::QueryExecution("图算法工作区大小溢出".into()))?;
    if base_bytes > max_bytes {
        return Err(TriviumError::QueryExecution(
            "图算法工作区超过临时内存预算 (Graph workspace exceeds temporary memory budget)".into(),
        ));
    }
    let mut examined_edges = 0usize;
    let mut bytes = base_bytes;
    let mut directed = BTreeMap::new();
    let mut reverse = BTreeMap::new();
    let mut weighted = BTreeMap::new();
    let mut undirected = BTreeMap::new();
    for &id in nodes {
        crate::graph::budget::check_control(&control)?;
        directed.insert(id, Vec::new());
        reverse.insert(id, Vec::new());
        weighted.insert(id, Vec::new());
        undirected.insert(id, Vec::new());
    }
    for &source in nodes {
        crate::graph::budget::check_control(&control)?;
        for edge in mt.get_edges(source).unwrap_or_default() {
            crate::graph::budget::check_control(&control)?;
            charge_edge(&mut examined_edges, max_examined_edges)?;
            if nodes.contains(&edge.target_id)
                && label_filter.is_none_or(|label| edge.label == label)
            {
                // 正、反、双向和带权邻接表的首次分配、倍增和旧缓冲同时存活。
                bytes = bytes
                    .saturating_add(6 * (4 * size_of::<NodeId>() + size_of::<(NodeId, f64)>()));
                if bytes > max_bytes {
                    return Err(TriviumError::QueryExecution(
                        "图算法邻接表超过临时内存预算 (Graph adjacency exceeds temporary memory budget)".into(),
                    ));
                }
                if require_nonnegative_weights {
                    validate_weight(f64::from(edge.weight))?;
                }
                directed.entry(source).or_default().push(edge.target_id);
                reverse.entry(edge.target_id).or_default().push(source);
                weighted
                    .entry(source)
                    .or_default()
                    .push((edge.target_id, f64::from(edge.weight)));
            }
        }
    }
    for targets in directed.values_mut() {
        crate::graph::budget::check_control(&control)?;
        targets.sort_unstable();
        targets.dedup();
    }
    for sources in reverse.values_mut() {
        crate::graph::budget::check_control(&control)?;
        sources.sort_unstable();
        sources.dedup();
    }
    for targets in weighted.values_mut() {
        crate::graph::budget::check_control(&control)?;
        targets.sort_unstable_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.total_cmp(&right.1))
        });
        targets.dedup_by_key(|entry| entry.0);
    }
    for (&source, targets) in &directed {
        crate::graph::budget::check_control(&control)?;
        for &target in targets {
            crate::graph::budget::check_control(&control)?;
            if source != target {
                undirected.entry(source).or_default().push(target);
                undirected.entry(target).or_default().push(source);
            }
        }
    }
    for neighbors in undirected.values_mut() {
        crate::graph::budget::check_control(&control)?;
        neighbors.sort_unstable();
        neighbors.dedup();
    }
    Ok(GraphWorkspace {
        directed,
        reverse,
        undirected,
        weighted,
        examined_edges,
    })
}

pub fn strongly_connected_components(graph: &GraphWorkspace) -> BTreeMap<NodeId, NodeId> {
    strongly_connected_components_checked(graph, &|| Ok::<(), std::convert::Infallible>(()))
        .unwrap_or_else(|never| match never {})
}

pub(crate) fn strongly_connected_components_controlled(
    graph: &GraphWorkspace,
) -> Result<BTreeMap<NodeId, NodeId>> {
    let control = crate::request::control().unwrap_or_default();
    strongly_connected_components_checked(graph, &|| crate::graph::budget::check_control(&control))
}

fn strongly_connected_components_checked<E>(
    graph: &GraphWorkspace,
    check: &impl Fn() -> std::result::Result<(), E>,
) -> std::result::Result<BTreeMap<NodeId, NodeId>, E> {
    check()?;
    fn finish_order<E>(
        start: NodeId,
        adjacency: &BTreeMap<NodeId, Vec<NodeId>>,
        visited: &mut BTreeSet<NodeId>,
        order: &mut Vec<NodeId>,
        check: &impl Fn() -> std::result::Result<(), E>,
    ) -> std::result::Result<(), E> {
        let mut stack = vec![(start, false)];
        while let Some((node, expanded)) = stack.pop() {
            check()?;
            if expanded {
                order.push(node);
                continue;
            }
            if !visited.insert(node) {
                continue;
            }
            stack.push((node, true));
            if let Some(neighbors) = adjacency.get(&node) {
                for &neighbor in neighbors.iter().rev() {
                    check()?;
                    if !visited.contains(&neighbor) {
                        stack.push((neighbor, false));
                    }
                }
            }
        }
        Ok(())
    }
    let mut visited = BTreeSet::new();
    let mut order = Vec::new();
    for &node in graph.directed.keys() {
        check()?;
        if !visited.contains(&node) {
            finish_order(node, &graph.directed, &mut visited, &mut order, check)?;
        }
    }
    let mut result = BTreeMap::new();
    visited.clear();
    for &start in order.iter().rev() {
        check()?;
        if visited.contains(&start) {
            continue;
        }
        let mut stack = vec![start];
        let mut component = Vec::new();
        visited.insert(start);
        while let Some(node) = stack.pop() {
            check()?;
            component.push(node);
            if let Some(neighbors) = graph.reverse.get(&node) {
                for &neighbor in neighbors.iter().rev() {
                    check()?;
                    if visited.insert(neighbor) {
                        stack.push(neighbor);
                    }
                }
            }
        }
        let component_id = component.iter().copied().min().unwrap_or(start);
        for node in component {
            check()?;
            result.insert(node, component_id);
        }
    }
    Ok(result)
}

pub fn k_core(graph: &GraphWorkspace) -> BTreeMap<NodeId, u64> {
    k_core_checked(graph, &|| Ok::<(), std::convert::Infallible>(()))
        .unwrap_or_else(|never| match never {})
}

pub(crate) fn k_core_controlled(graph: &GraphWorkspace) -> Result<BTreeMap<NodeId, u64>> {
    let control = crate::request::control().unwrap_or_default();
    k_core_checked(graph, &|| crate::graph::budget::check_control(&control))
}

fn k_core_checked<E>(
    graph: &GraphWorkspace,
    check: &impl Fn() -> std::result::Result<(), E>,
) -> std::result::Result<BTreeMap<NodeId, u64>, E> {
    check()?;
    let mut degree = BTreeMap::from_iter(
        graph
            .undirected
            .iter()
            .map(|(&id, neighbors)| (id, neighbors.len())),
    );
    let mut remaining = degree
        .iter()
        .map(|(&id, &d)| (d, id))
        .collect::<BTreeSet<_>>();
    let mut output = BTreeMap::new();
    let mut core = 0usize;
    while let Some((node_degree, node)) = remaining.pop_first() {
        check()?;
        degree.remove(&node);
        core = core.max(node_degree);
        output.insert(node, core as u64);
        for neighbor in graph.undirected.get(&node).into_iter().flatten() {
            check()?;
            if let Some(d) = degree.get_mut(neighbor) {
                remaining.remove(&(*d, *neighbor));
                *d = d.saturating_sub(1);
                remaining.insert((*d, *neighbor));
            }
        }
    }
    Ok(output)
}

pub fn articulation_points(graph: &GraphWorkspace) -> BTreeSet<NodeId> {
    articulation_points_checked(graph, &|| Ok::<(), std::convert::Infallible>(()))
        .unwrap_or_else(|never| match never {})
}

pub(crate) fn articulation_points_controlled(graph: &GraphWorkspace) -> Result<BTreeSet<NodeId>> {
    let control = crate::request::control().unwrap_or_default();
    articulation_points_checked(graph, &|| crate::graph::budget::check_control(&control))
}

fn articulation_points_checked<E>(
    graph: &GraphWorkspace,
    check: &impl Fn() -> std::result::Result<(), E>,
) -> std::result::Result<BTreeSet<NodeId>, E> {
    check()?;
    let mut discovery = BTreeMap::<NodeId, usize>::new();
    let mut low = BTreeMap::<NodeId, usize>::new();
    let mut parent = BTreeMap::<NodeId, NodeId>::new();
    let mut points = BTreeSet::new();
    let mut time = 0usize;
    for &root in graph.undirected.keys() {
        check()?;
        if discovery.contains_key(&root) {
            continue;
        }
        let mut root_children = 0usize;
        let mut stack = vec![(root, 0usize)];
        time += 1;
        discovery.insert(root, time);
        low.insert(root, time);
        while let Some((node, next_index)) = stack.pop() {
            check()?;
            let neighbors = graph
                .undirected
                .get(&node)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if next_index < neighbors.len() {
                stack.push((node, next_index + 1));
                let neighbor = neighbors[next_index];
                match discovery.entry(neighbor) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        parent.insert(neighbor, node);
                        if node == root {
                            root_children += 1;
                        }
                        time += 1;
                        entry.insert(time);
                        low.insert(neighbor, time);
                        stack.push((neighbor, 0));
                    }
                    std::collections::btree_map::Entry::Occupied(entry)
                        if parent.get(&node).copied() != Some(neighbor) =>
                    {
                        let candidate = *entry.get();
                        low.entry(node)
                            .and_modify(|value| *value = (*value).min(candidate));
                    }
                    std::collections::btree_map::Entry::Occupied(_) => {}
                }
            } else if let Some(&parent_node) = parent.get(&node) {
                let Some(&node_low) = low.get(&node) else {
                    continue;
                };
                low.entry(parent_node)
                    .and_modify(|value| *value = (*value).min(node_low));
                if parent_node != root
                    && discovery
                        .get(&parent_node)
                        .is_some_and(|&parent_discovery| node_low >= parent_discovery)
                {
                    points.insert(parent_node);
                }
            }
        }
        if root_children > 1 {
            points.insert(root);
        }
    }
    Ok(points)
}

pub fn triangle_metrics(
    graph: &GraphWorkspace,
    max_work: usize,
) -> Result<BTreeMap<NodeId, (u64, f64)>> {
    let control = crate::request::control().unwrap_or_default();
    control.check()?;
    let mut counts = BTreeMap::from_iter(graph.undirected.keys().map(|&id| (id, 0u64)));
    let mut work = 0usize;
    for (&a, neighbors) in &graph.undirected {
        crate::graph::budget::check_control(&control)?;
        for &b in neighbors.iter().filter(|&&neighbor| neighbor > a) {
            crate::graph::budget::check_control(&control)?;
            let Some(b_neighbors) = graph.undirected.get(&b) else {
                continue;
            };
            for &c in b_neighbors.iter().filter(|&&neighbor| neighbor > b) {
                crate::graph::budget::check_control(&control)?;
                work = work.saturating_add(1);
                if work > max_work {
                    return Err(TriviumError::QueryExecution(
                        "三角计数超过交集工作预算 (Triangle count exceeds intersection work budget)".into(),
                    ));
                }
                if neighbors.contains(&c) {
                    *counts.entry(a).or_default() += 1;
                    *counts.entry(b).or_default() += 1;
                    *counts.entry(c).or_default() += 1;
                }
            }
        }
    }
    Ok(counts
        .into_iter()
        .map(|(id, count)| {
            let degree = graph.undirected.get(&id).map_or(0, Vec::len);
            let coefficient = if degree >= 2 {
                2.0 * count as f64 / (degree * (degree - 1)) as f64
            } else {
                0.0
            };
            (id, (count, coefficient))
        })
        .collect())
}

pub fn hits(
    graph: &GraphWorkspace,
    max_iterations: usize,
    tolerance: f64,
    max_work: usize,
) -> Result<BTreeMap<NodeId, (f64, f64)>> {
    hits_with_work(graph, max_iterations, tolerance, &mut 0, max_work)
}

pub(crate) fn hits_with_work(
    graph: &GraphWorkspace,
    max_iterations: usize,
    tolerance: f64,
    work: &mut usize,
    max_work: usize,
) -> Result<BTreeMap<NodeId, (f64, f64)>> {
    let control = crate::request::control().unwrap_or_default();
    control.check()?;
    if max_iterations == 0 || !tolerance.is_finite() || tolerance <= 0.0 {
        return Err(TriviumError::InvalidInput(
            "HITS 参数无效 (Invalid HITS parameters)".into(),
        ));
    }
    let ids = graph.directed.keys().copied().collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let per_iteration = graph
        .directed
        .values()
        .map(Vec::len)
        .sum::<usize>()
        .saturating_mul(2);
    let initial = 1.0 / (ids.len() as f64).sqrt();
    let mut hubs = BTreeMap::from_iter(ids.iter().map(|&id| (id, initial)));
    let mut authorities = hubs.clone();
    for _ in 0..max_iterations {
        crate::graph::budget::check_control(&control)?;
        *work = work
            .checked_add(per_iteration)
            .ok_or_else(|| TriviumError::QueryExecution("HITS 工作预算溢出".into()))?;
        if *work > max_work {
            return Err(TriviumError::QueryExecution(
                "HITS 超过迭代边预算 (HITS exceeds iterative edge budget)".into(),
            ));
        }
        let mut next_authorities = BTreeMap::new();
        for &id in &ids {
            crate::graph::budget::check_control(&control)?;
            let score = graph
                .reverse
                .get(&id)
                .into_iter()
                .flatten()
                .filter_map(|source| hubs.get(source))
                .sum::<f64>();
            next_authorities.insert(id, score);
        }
        normalize(&mut next_authorities);
        let mut next_hubs = BTreeMap::new();
        for &id in &ids {
            crate::graph::budget::check_control(&control)?;
            let score = graph
                .directed
                .get(&id)
                .into_iter()
                .flatten()
                .filter_map(|target| next_authorities.get(target))
                .sum::<f64>();
            next_hubs.insert(id, score);
        }
        normalize(&mut next_hubs);
        let delta = ids
            .iter()
            .map(|id| {
                let next_hub = next_hubs.get(id).copied().unwrap_or_default();
                let hub = hubs.get(id).copied().unwrap_or_default();
                let next_authority = next_authorities.get(id).copied().unwrap_or_default();
                let authority = authorities.get(id).copied().unwrap_or_default();
                (next_hub - hub).abs() + (next_authority - authority).abs()
            })
            .sum::<f64>();
        hubs = next_hubs;
        authorities = next_authorities;
        if delta <= tolerance {
            break;
        }
    }
    Ok(ids
        .into_iter()
        .map(|id| {
            (
                id,
                (
                    authorities.get(&id).copied().unwrap_or_default(),
                    hubs.get(&id).copied().unwrap_or_default(),
                ),
            )
        })
        .collect())
}

fn normalize(values: &mut BTreeMap<NodeId, f64>) {
    let norm = values
        .values()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if norm > 0.0 {
        for value in values.values_mut() {
            *value /= norm;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeightedPath {
    pub nodes: Vec<NodeId>,
    pub cost: f64,
}

use super::path_cost::PathCost;

type WeightedSearchState = (BTreeMap<NodeId, PathCost>, BTreeMap<NodeId, NodeId>);

fn weighted_path_to(parents: &BTreeMap<NodeId, NodeId>, mut node: NodeId) -> Vec<NodeId> {
    let mut path = vec![node];
    while let Some(&parent) = parents.get(&node) {
        path.push(parent);
        node = parent;
    }
    path.reverse();
    path
}

fn charge_edge(work: &mut usize, max_work: usize) -> Result<()> {
    if *work >= max_work {
        return Err(TriviumError::QueryExecution(
            "图算法超过边访问预算 (Graph algorithm exceeds edge examination budget)".into(),
        ));
    }
    *work += 1;
    Ok(())
}

// 每个未确定节点在堆中只有一个位置。松弛只会降低距离或改善同距离路径。
#[derive(Default)]
struct WeightedFrontier {
    nodes: Vec<NodeId>,
    positions: BTreeMap<NodeId, usize>,
}

impl WeightedFrontier {
    fn swap(&mut self, a: usize, b: usize) {
        self.nodes.swap(a, b);
        self.positions.insert(self.nodes[a], a);
        self.positions.insert(self.nodes[b], b);
    }

    fn improve(&mut self, id: NodeId, before: impl Fn(NodeId, NodeId) -> bool) {
        let mut pos = *self.positions.entry(id).or_insert_with(|| {
            self.nodes.push(id);
            self.nodes.len() - 1
        });
        while pos > 0 {
            let parent = (pos - 1) / 2;
            if !before(id, self.nodes[parent]) {
                break;
            }
            self.swap(parent, pos);
            pos = parent;
        }
    }

    fn pop(&mut self, before: impl Fn(NodeId, NodeId) -> bool) -> Option<NodeId> {
        let id = *self.nodes.first()?;
        self.nodes.swap_remove(0);
        self.positions.remove(&id);
        if let Some(&root) = self.nodes.first() {
            self.positions.insert(root, 0);
        }
        let mut pos = 0;
        while pos < self.nodes.len() / 2 {
            let left = 2 * pos + 1;
            let right = left + 1;
            let child = if right < self.nodes.len() && before(self.nodes[right], self.nodes[left]) {
                right
            } else {
                left
            };
            if !before(self.nodes[child], self.nodes[pos]) {
                break;
            }
            self.swap(child, pos);
            pos = child;
        }
        Some(id)
    }
}

// 父节点总是先确定。沿共享前缀树找首个分歧，无需给每个候选复制整条路径。
fn path_precedes(
    mut a: NodeId,
    mut b: NodeId,
    suffix: Option<NodeId>,
    parents: &BTreeMap<NodeId, NodeId>,
    depths: &BTreeMap<NodeId, usize>,
) -> bool {
    if a == b {
        return false;
    }
    let (da, db) = (depths[&a], depths[&b]);
    let mut child = 0;
    for _ in db..da {
        child = a;
        a = parents[&a];
    }
    for _ in da..db {
        child = b;
        b = parents[&b];
    }
    if a == b {
        return if da < db {
            suffix.is_none_or(|end| end < child)
        } else {
            suffix.is_some_and(|end| child < end)
        };
    }
    while parents[&a] != parents[&b] {
        a = parents[&a];
        b = parents[&b];
    }
    a < b
}

fn shortest_weighted_path(
    graph: &GraphWorkspace,
    source: NodeId,
    target: Option<NodeId>,
    banned_nodes: &BTreeSet<NodeId>,
    banned_edges: &BTreeSet<(NodeId, NodeId)>,
    work: &mut usize,
    max_work: usize,
) -> Result<WeightedSearchState> {
    let control = crate::request::control().unwrap_or_default();
    control.check()?;
    let mut distances = BTreeMap::new();
    let mut parents = BTreeMap::<NodeId, NodeId>::new();
    if banned_nodes.contains(&source) || !graph.weighted.contains_key(&source) {
        return Ok((distances, parents));
    }
    distances.insert(source, PathCost::Float(0.0));
    let mut depths = BTreeMap::from([(source, 0usize)]);
    let before = |a,
                  b,
                  distances: &BTreeMap<NodeId, PathCost>,
                  parents: &BTreeMap<NodeId, NodeId>,
                  depths: &BTreeMap<NodeId, usize>| {
        match distances[&a].compare(&distances[&b]) {
            std::cmp::Ordering::Equal if target.is_some() => {
                path_precedes(a, b, None, parents, depths)
            }
            std::cmp::Ordering::Equal => a < b,
            order => order.is_lt(),
        }
    };
    let mut frontier = WeightedFrontier::default();
    frontier.improve(source, |_, _| false);
    let mut settled = BTreeSet::new();
    while let Some(node) = frontier.pop(|a, b| before(a, b, &distances, &parents, &depths)) {
        crate::graph::budget::check_control(&control)?;
        let distance = distances[&node].clone();
        settled.insert(node);
        if target == Some(node) {
            break;
        }
        for &(neighbor, weight) in graph.weighted.get(&node).into_iter().flatten() {
            crate::graph::budget::check_control(&control)?;
            charge_edge(work, max_work)?;
            if banned_nodes.contains(&neighbor)
                || banned_edges.contains(&(node, neighbor))
                || settled.contains(&neighbor)
            {
                continue;
            }
            let candidate_distance = distance.plus(weight);
            let order = distances
                .get(&neighbor)
                .map_or(std::cmp::Ordering::Less, |cost| {
                    candidate_distance.compare(cost)
                });
            let replace = order.is_lt()
                || target.is_some()
                    && order.is_eq()
                    && path_precedes(node, parents[&neighbor], Some(neighbor), &parents, &depths);
            if replace {
                distances.insert(neighbor, candidate_distance);
                // 父节点已确定，前缀在后续松弛中保持稳定。
                if target.is_some() {
                    parents.insert(neighbor, node);
                    depths.insert(neighbor, depths[&node] + 1);
                }
                frontier.improve(neighbor, |a, b| before(a, b, &distances, &parents, &depths));
            }
        }
    }
    Ok((distances, parents))
}

pub(crate) fn validate_weighted_graph(graph: &GraphWorkspace) -> Result<()> {
    let control = crate::request::control().unwrap_or_default();
    control.check()?;
    for &(_, weight) in graph.weighted.values().flatten() {
        crate::graph::budget::check_control(&control)?;
        validate_weight(weight)?;
    }
    Ok(())
}

fn validate_weight(weight: f64) -> Result<()> {
    if !weight.is_finite() || weight < 0.0 {
        return Err(TriviumError::InvalidInput(
            "加权算法要求非负有限边权 (Weighted algorithms require finite non-negative edge weights)".into(),
        ));
    }
    Ok(())
}

pub fn weighted_dijkstra(
    graph: &GraphWorkspace,
    source: NodeId,
    target: NodeId,
    max_work: usize,
) -> Result<Option<WeightedPath>> {
    validate_weighted_graph(graph)?;
    weighted_dijkstra_with_work(graph, source, target, &mut 0, max_work)
}

pub(crate) fn weighted_dijkstra_with_work(
    graph: &GraphWorkspace,
    source: NodeId,
    target: NodeId,
    work: &mut usize,
    max_work: usize,
) -> Result<Option<WeightedPath>> {
    let (distances, parents) = shortest_weighted_path(
        graph,
        source,
        Some(target),
        &BTreeSet::new(),
        &BTreeSet::new(),
        work,
        max_work,
    )?;
    distances
        .get(&target)
        .map(|cost| {
            Ok(WeightedPath {
                nodes: weighted_path_to(&parents, target),
                cost: cost.finite_value()?,
            })
        })
        .transpose()
}

pub fn harmonic_centrality(
    graph: &GraphWorkspace,
    max_work: usize,
) -> Result<BTreeMap<NodeId, f64>> {
    validate_weighted_graph(graph)?;
    harmonic_centrality_with_work(graph, &mut 0, max_work)
}

pub(crate) fn harmonic_centrality_with_work(
    graph: &GraphWorkspace,
    work: &mut usize,
    max_work: usize,
) -> Result<BTreeMap<NodeId, f64>> {
    let control = crate::request::control().unwrap_or_default();
    control.check()?;
    let mut output = BTreeMap::new();
    for &source in graph.weighted.keys() {
        crate::graph::budget::check_control(&control)?;
        let (distances, _) = shortest_weighted_path(
            graph,
            source,
            None,
            &BTreeSet::new(),
            &BTreeSet::new(),
            work,
            max_work,
        )?;
        let score = distances
            .into_iter()
            .map(|(id, cost)| (id, cost.value()))
            .filter(|&(target, distance)| {
                target != source && distance.is_finite() && distance > 0.0
            })
            .map(|(_, distance)| 1.0 / distance)
            .sum();
        output.insert(source, score);
    }
    Ok(output)
}

pub fn yen_k_shortest_paths(
    graph: &GraphWorkspace,
    source: NodeId,
    target: NodeId,
    k: usize,
    max_work: usize,
    max_candidates: usize,
) -> Result<Vec<WeightedPath>> {
    validate_weighted_graph(graph)?;
    yen_k_shortest_paths_with_work(graph, source, target, k, &mut 0, max_work, max_candidates)
}

pub(crate) fn yen_k_shortest_paths_with_work(
    graph: &GraphWorkspace,
    source: NodeId,
    target: NodeId,
    k: usize,
    work: &mut usize,
    max_work: usize,
    max_candidates: usize,
) -> Result<Vec<WeightedPath>> {
    let control = crate::request::control().unwrap_or_default();
    control.check()?;
    if k == 0 || max_candidates == 0 {
        return Err(TriviumError::InvalidInput(
            "Yen 的 K 和候选上限必须大于 0 (Yen K and candidate limit must be positive)".into(),
        ));
    }
    let Some(first) = weighted_dijkstra_with_work(graph, source, target, work, max_work)? else {
        return Ok(Vec::new());
    };
    let mut accepted = vec![first];
    let mut candidates = BTreeMap::<Vec<NodeId>, PathCost>::new();
    while accepted.len() < k {
        crate::graph::budget::check_control(&control)?;
        let previous = accepted
            .last()
            .cloned()
            .ok_or_else(|| TriviumError::QueryExecution("Yen 已接受路径状态不一致".into()))?;
        for spur_index in 0..previous.nodes.len().saturating_sub(1) {
            crate::graph::budget::check_control(&control)?;
            let root = &previous.nodes[..=spur_index];
            let spur = previous.nodes[spur_index];
            let banned_nodes = root[..root.len().saturating_sub(1)]
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            let banned_edges = accepted
                .iter()
                .filter(|path| path.nodes.len() > spur_index && path.nodes[..=spur_index] == *root)
                .filter_map(|path| {
                    path.nodes
                        .get(spur_index + 1)
                        .copied()
                        .map(|next| (spur, next))
                })
                .collect::<BTreeSet<_>>();
            let (spur_distances, spur_parents) = shortest_weighted_path(
                graph,
                spur,
                Some(target),
                &banned_nodes,
                &banned_edges,
                work,
                max_work,
            )?;
            let Some(spur_cost) = spur_distances.get(&target) else {
                continue;
            };
            let mut nodes = root[..root.len().saturating_sub(1)].to_vec();
            nodes.extend(weighted_path_to(&spur_parents, target));
            // 只重新查找现有路径的根前缀，后缀直接使用搜索得到的代价。
            let mut cost = spur_cost.clone();
            for edge in root.windows(2) {
                crate::graph::budget::check_control(&control)?;
                for &(neighbor, weight) in graph.weighted.get(&edge[0]).into_iter().flatten() {
                    charge_edge(work, max_work)?;
                    if neighbor == edge[1] {
                        cost = cost.plus(weight);
                        break;
                    }
                }
            }
            if !accepted.iter().any(|path| path.nodes == nodes) {
                candidates.entry(nodes).or_insert(cost);
                if candidates.len() > max_candidates {
                    return Err(TriviumError::QueryExecution(
                        "Yen 候选路径超过预算 (Yen candidate paths exceed budget)".into(),
                    ));
                }
            }
        }
        let Some((nodes, cost)) = candidates
            .iter()
            .min_by(|left, right| left.1.compare(right.1).then_with(|| left.0.cmp(right.0)))
            .map(|(nodes, cost)| (nodes.clone(), cost.finite_value()))
        else {
            break;
        };
        candidates.remove(&nodes);
        accepted.push(WeightedPath { nodes, cost: cost? });
    }
    Ok(accepted)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeSimilarityPair {
    pub left: NodeId,
    pub right: NodeId,
    pub similarity: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PairSet {
    pairs: Vec<NodeSimilarityPair>,
}

impl PairSet {
    pub fn pairs(&self) -> &[NodeSimilarityPair] {
        &self.pairs
    }
}

pub fn node_similarity(
    graph: &GraphWorkspace,
    top_k: usize,
    cutoff: f64,
    max_comparisons: usize,
    max_bytes: usize,
) -> Result<PairSet> {
    let control = crate::request::control().unwrap_or_default();
    control.check()?;
    if top_k == 0 || !cutoff.is_finite() || !(0.0..=1.0).contains(&cutoff) {
        return Err(TriviumError::InvalidInput(
            "节点相似度参数无效 (Invalid node similarity parameters)".into(),
        ));
    }
    let node_count = graph.undirected.len();
    let comparisons = node_count
        .checked_mul(node_count.saturating_sub(1))
        .and_then(|value| value.checked_div(2))
        .ok_or_else(|| TriviumError::QueryExecution("节点对数量溢出".into()))?;
    if comparisons > max_comparisons {
        return Err(TriviumError::QueryExecution(
            "节点相似度超过比较预算 (Node similarity exceeds comparison budget)".into(),
        ));
    }
    let capacity = top_k.min(comparisons);
    let required_bytes = capacity
        .checked_mul(std::mem::size_of::<NodeSimilarityPair>())
        .ok_or_else(|| TriviumError::QueryExecution("PairSet 大小溢出".into()))?;
    if required_bytes > max_bytes {
        return Err(TriviumError::QueryExecution(
            "PairSet 超过临时内存预算 (PairSet exceeds temporary memory budget)".into(),
        ));
    }
    let ids = graph.undirected.keys().copied().collect::<Vec<_>>();
    let mut pairs = Vec::with_capacity(capacity);
    let mut work = 0usize;
    for (index, &left) in ids.iter().enumerate() {
        crate::graph::budget::check_control(&control)?;
        let left_neighbors = graph
            .undirected
            .get(&left)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for &right in &ids[index + 1..] {
            crate::graph::budget::check_control(&control)?;
            work = work
                .checked_add(1)
                .ok_or_else(|| TriviumError::QueryExecution("节点相似度工作预算溢出".into()))?;
            if work > max_comparisons {
                return Err(TriviumError::QueryExecution(
                    "节点相似度超过比较预算 (Node similarity exceeds comparison budget)".into(),
                ));
            }
            let right_neighbors = graph
                .undirected
                .get(&right)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut left_index = 0usize;
            let mut right_index = 0usize;
            let mut intersection = 0usize;
            while left_index < left_neighbors.len() && right_index < right_neighbors.len() {
                crate::graph::budget::check_control(&control)?;
                work = work
                    .checked_add(1)
                    .ok_or_else(|| TriviumError::QueryExecution("节点相似度工作预算溢出".into()))?;
                if work > max_comparisons {
                    return Err(TriviumError::QueryExecution(
                        "节点相似度超过邻居交集预算 (Node similarity exceeds neighbor intersection budget)".into(),
                    ));
                }
                match left_neighbors[left_index].cmp(&right_neighbors[right_index]) {
                    std::cmp::Ordering::Less => left_index += 1,
                    std::cmp::Ordering::Greater => right_index += 1,
                    std::cmp::Ordering::Equal => {
                        intersection += 1;
                        left_index += 1;
                        right_index += 1;
                    }
                }
            }
            let union = left_neighbors
                .len()
                .saturating_add(right_neighbors.len())
                .saturating_sub(intersection);
            let similarity = if union == 0 {
                0.0
            } else {
                intersection as f64 / union as f64
            };
            if similarity < cutoff {
                continue;
            }
            let pair = NodeSimilarityPair {
                left,
                right,
                similarity,
            };
            let slot = pairs.partition_point(|other: &NodeSimilarityPair| {
                other.similarity > similarity
                    || (other.similarity == similarity
                        && (other.left, other.right) <= (left, right))
            });
            if slot < capacity {
                if pairs.len() == capacity {
                    pairs.pop();
                }
                pairs.insert(slot, pair);
            }
        }
    }
    Ok(PairSet { pairs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 相似节点对替换首位时保持获准容量() {
        let mut mt = MemTable::<f32>::new(1);
        for id in 1..=4 {
            mt.insert_with_id(id, &[1.], Default::default()).unwrap();
        }
        for (from, to) in [(1, 2), (2, 3), (2, 4)] {
            mt.link(from, to, "e".into(), 1.).unwrap();
        }
        let graph = build_workspace(&mt, &BTreeSet::from([1, 2, 3, 4]), None, 3, 4096).unwrap();
        let result = node_similarity(
            &graph,
            1,
            0.,
            100,
            std::mem::size_of::<NodeSimilarityPair>(),
        )
        .unwrap();
        assert_eq!(
            result.pairs(),
            &[NodeSimilarityPair {
                left: 1,
                right: 3,
                similarity: 1.
            }]
        );
        assert_eq!(result.pairs.capacity(), 1);
    }
}
