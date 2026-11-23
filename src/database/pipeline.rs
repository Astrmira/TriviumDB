//! 混合检索管线 (Hybrid Search Pipeline)
//!
//! 从 database.rs 独立拆分的核心检索逻辑，包含：
//! - L0 安全防御（NaN/Inf/维度检查 + 参数钳位）
//! - L1 文本稀疏召回（AC 自动机 + BM25）
//! - L2 向量稠密召回（BruteForce / BQ 三级火箭）
//! - L3 Payload 预过滤（Parallel Bit-Tag Array 布隆拦截）
//! - L4 FISTA 残差搜索
//! - L5 影子查询
//! - L6 SA-PPR 有限深度图谱扩散
//! - L7 不应期/侧向抑制
//! - L9 DPP 多样性采样
//!
//! 以及 6 个 Hook 调用点的集成。

use crate::VectorType;
use crate::database::config::SearchConfig;
use crate::error::Result;
use crate::hook::{HookContext, NoopHook, SearchHook};
use crate::index::brute_force;
use crate::index::quiver::QuIVerSearchConfig;
use crate::node::{NodeId, SearchHit};
use crate::storage::memtable::MemTable;
use std::sync::{Arc, RwLock};

pub(crate) struct PipelineOutput {
    pub combined_hits: Vec<SearchHit>,
    pub result_workspace: Option<crate::resources::RequestWorkspace>,
    // 纯向量查询的语义结果就是 combined_hits，两套结果共用同一份所有权。
    pub semantic_hits: Option<Vec<SearchHit>>,
    pub graph_hits: Vec<SearchHit>,
}

impl PipelineOutput {
    fn empty() -> Self {
        Self {
            combined_hits: Vec::new(),
            result_workspace: None,
            semantic_hits: None,
            graph_hits: Vec::new(),
        }
    }
}

use super::{HookReadScope, read_or_recover, try_start_quiver_build, write_or_recover};

fn call_hook<T: VectorType, R>(
    memtable: &Arc<RwLock<MemTable<T>>>,
    call: impl FnOnce() -> R,
) -> Result<R> {
    let _scope = HookReadScope::enter(memtable)?;
    Ok(call())
}

pub(super) fn validate_config<T: VectorType>(config: &SearchConfig, dim: usize) -> Result<()> {
    if config.top_k == 0 {
        return Err(crate::TriviumError::InvalidInput(
            "top_k 必须大于零 (top_k must be positive)".into(),
        )
        .at_field("top_k"));
    }
    if config.ann_rerank_limit == Some(0) {
        return Err(crate::error::TriviumError::InvalidInput(
            "ann_rerank_limit 必须大于 0 (ann_rerank_limit must be positive)".into(),
        )
        .at_field("ann_rerank_limit"));
    }
    for (name, value) in [
        ("min_score", config.min_score),
        ("fista_lambda", config.fista_lambda),
        ("teleport_alpha", config.teleport_alpha),
        ("dpp_quality_weight", config.dpp_quality_weight),
        ("fista_threshold", config.fista_threshold),
        ("text_boost", config.text_boost),
        ("min_edge_weight", config.min_edge_weight),
    ] {
        if !value.is_finite() {
            return Err(crate::error::TriviumError::InvalidInput(format!(
                "检索配置 {name} 必须是有限数值 (Search config {name} must be finite)"
            ))
            .at_field(name));
        }
    }

    if crate::vector::is_binary_vector::<T>()
        && config.enable_advanced_pipeline
        && (config.enable_sparse_residual || config.enable_dpp)
    {
        return Err(crate::TriviumError::InvalidInput("u64 支持汉明检索与图扩散，浮点认知重排配置不适用于二进制指纹 (u64 supports Hamming retrieval and graph expansion; floating-point cognitive reranking is unavailable for binary fingerprints)".into()));
    }
    crate::index::text::validate_bm25(config.bm25_k1, config.bm25_b)?;
    if let Some(bias) = config.diffusion_bias.as_deref() {
        if bias.len() != dim {
            return Err(crate::error::TriviumError::DimensionMismatch {
                expected: dim,
                got: bias.len(),
            }
            .at_field("diffusion_bias"));
        }
        if bias.iter().any(|value| !value.is_finite()) {
            return Err(crate::error::TriviumError::InvalidVector {
                reason:
                    "扩散偏置向量包含 NaN 或 Infinity (Diffusion bias contains NaN or Infinity)"
                        .to_string(),
            }
            .at_field("diffusion_bias"));
        }
    }

    if config.min_edge_weight < 0.0 {
        return Err(crate::error::TriviumError::InvalidInput(
            "min_edge_weight 不得为负数 (min_edge_weight must be non-negative)".into(),
        )
        .at_field("min_edge_weight"));
    }
    Ok(())
}

fn sanitize_config<T: VectorType>(config: &mut SearchConfig, dim: usize) -> Result<()> {
    validate_config::<T>(config, dim)?;
    config.recall_k = if config.recall_k == 0 {
        config.top_k.saturating_mul(8).max(64)
    } else {
        config.recall_k.max(config.top_k)
    };
    config.rerank_k = if config.rerank_k == 0 {
        config.top_k.saturating_mul(4).max(32)
    } else {
        config.rerank_k.max(config.top_k)
    };
    config.rerank_k = config.rerank_k.min(config.recall_k);
    config.fista_lambda = config.fista_lambda.clamp(1e-5, 100.0);
    config.teleport_alpha = config.teleport_alpha.clamp(0.0, 1.0);
    config.dpp_quality_weight = config.dpp_quality_weight.clamp(0.0, 10.0);
    config.fista_threshold = config.fista_threshold.clamp(0.0, f32::MAX);
    Ok(())
}

fn validate_hooked_query(query: &[f32], dim: usize) -> Result<()> {
    if query.len() != dim {
        return Err(crate::error::TriviumError::DimensionMismatch {
            expected: dim,
            got: query.len(),
        });
    }
    if query.iter().any(|value| !value.is_finite()) {
        return Err(crate::error::TriviumError::InvalidVector {
            reason: "Hook 修改后的查询向量包含 NaN 或 Infinity (Hook-modified query vector contains NaN or Infinity)"
                .to_string(),
        });
    }
    Ok(())
}

fn check_hook_error(ctx: &mut HookContext) -> Result<()> {
    if let Some(error) = ctx.error.take() {
        return Err(crate::error::TriviumError::HookExecutionError(error));
    }
    crate::request::check_current()
}

/// 执行完整的混合检索管线
///
/// 这是从 `Database::search_hybrid_internal` 中提取出的核心管线逻辑。
/// 将 ~500 行的检索实现独立为专门文件，便于维护和测试。
pub(crate) fn execute_pipeline_with_limit<T: VectorType>(
    memtable: &Arc<RwLock<MemTable<T>>>,
    quiver_builds: &super::QuiverBuilds,
    memory_limit: usize,
    hook: &Arc<dyn SearchHook>,
    query_text: Option<&str>,
    query_vector: Option<&[T]>,
    config: &SearchConfig,
    context: Option<&mut HookContext>,
    alpha: Option<f64>,
    include_payload: bool,
) -> Result<PipelineOutput> {
    crate::request::check_current()?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeSearchReadLock);
    let runtime = read_or_recover(memtable).resource_runtime().clone();
    let quiver_config = crate::index::quiver::QuIVerConfig::default();
    let mut safe_cfg = config.clone();
    let prepared = {
        let mt = read_or_recover(memtable);
        mt.ensure_readable()?;
        sanitize_config::<T>(&mut safe_cfg, mt.dim())?;
        if let Some(query) = query_vector {
            crate::vector::validate_query_vector(query, mt.dim())?;
        }
        // 精确请求直接扫描存储向量，自动建图仅服务当前需要 ANN 的向量请求。
        if alpha != Some(0.0)
            && query_vector.is_some()
            && !config.force_brute_force
            && mt.auto_quiver_build_needed()
            && let Ok(guard) =
                try_start_quiver_build(quiver_builds, mt.vector_generation(), &quiver_config)
        {
            let workspace = mt.reserve_request_workspace(
                memory_limit,
                mt.quiver_build_peak_bytes(&quiver_config),
            )?;
            mt.quiver_build_snapshot()
                .map(|snapshot| (snapshot, guard, workspace))
        } else {
            None
        }
    };
    if let Some((snapshot, _guard, _workspace)) = prepared {
        let source_generation = snapshot.generation;
        let index = runtime
            .pool(runtime.parallelism())?
            .install(|| MemTable::<T>::build_quiver_snapshot(snapshot, &quiver_config));
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeQuiverPublish);
        let published =
            write_or_recover(memtable).publish_quiver_if_current(source_generation, index);
        _guard
            .flight
            .published
            .store(published, std::sync::atomic::Ordering::Release);
    }

    let mt = read_or_recover(memtable);
    mt.ensure_readable()?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::SearchLockAcquired);

    let resident = || {
        if memory_limit == 0 {
            0
        } else {
            mt.resident_memory_bytes()
        }
    };
    let noop_hook = (hook.as_ref() as &dyn std::any::Any).is::<NoopHook>();
    let pure_vector = noop_hook
        && config.expand_depth == 0
        && !(config.enable_text_hybrid_search && query_text.is_some())
        && !(config.enable_advanced_pipeline
            && (config.enable_sparse_residual || config.enable_dpp));
    if pure_vector
        && context.is_none()
        && let Some(query) = query_vector.filter(|_| alpha != Some(0.0))
    {
        let mut combined_hits = vector_recall_hits(&mt, &safe_cfg, query, safe_cfg.top_k, true)?;
        crate::request::check_current()?;
        let bytes = if include_payload {
            hit_payload_bytes(
                &mt,
                combined_hits.iter().map(|hit| hit.id),
                combined_hits.capacity(),
            )?
        } else {
            combined_hits.len().saturating_mul(size_of::<NodeId>())
        };
        let mut result_workspace = mt.reserve_request_workspace(memory_limit, bytes)?;
        if include_payload {
            let mut retained = combined_hits
                .capacity()
                .saturating_mul(size_of::<SearchHit>());
            for hit in &mut combined_hits {
                crate::request::check_current()?;
                let payload = mt
                    .try_get_payload(hit.id)?
                    .ok_or(crate::TriviumError::NodeNotFound(hit.id))?;
                hit.payload = (*payload).clone();
                retained = retained.saturating_add(
                    crate::storage::payload_store::query_clone_peak(&hit.payload),
                );
            }
            result_workspace.resize_result(retained, resident(), memory_limit)?;
        }
        return Ok(PipelineOutput {
            combined_hits,
            result_workspace: Some(result_workspace),
            semantic_hits: None,
            graph_hits: Vec::new(),
        });
    }

    let collect_observations = context.is_some() || !noop_hook;
    let mut local_context = HookContext::new();
    let ctx = context.unwrap_or(&mut local_context);

    // ═══════════════════════════════════════════════════════
    //  L0: 容错与防御式编程 (Sanity Checks)
    // ═══════════════════════════════════════════════════════
    let dim = mt.dim();

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #1: on_pre_search — 查询预处理
    // ═══════════════════════════════════════════════════════
    let mut query_vec_f32: Vec<f32> = query_vector
        .map(|qv| qv.iter().map(|x| x.to_f32()).collect())
        .unwrap_or_default();
    let original_hook_query = crate::vector::is_binary_vector::<T>().then(|| query_vec_f32.clone());
    {
        let t0 = std::time::Instant::now();
        call_hook(memtable, || {
            hook.on_pre_search(&mut query_vec_f32, &mut safe_cfg, ctx)
        })?;
        ctx.record_timing("hook_pre_search", t0.elapsed());
        check_hook_error(ctx)?;
    }

    crate::request::check_current()?;

    // 如果 Hook 请求提前终止管线，直接返回空结果
    if ctx.abort {
        return Ok(PipelineOutput::empty());
    }

    if safe_cfg.enable_refractory_fatigue && !config.enable_refractory_fatigue {
        return Err(crate::TriviumError::InvalidInput(
            "Hook 启用疲劳需要调用方显式授权 (Enabling fatigue in a hook requires caller opt-in)"
                .into(),
        )
        .at_field("enable_refractory_fatigue"));
    }
    sanitize_config::<T>(&mut safe_cfg, dim)?;
    let _query_workspace = (!pure_vector)
        .then(|| reserve_candidate_workspace(&mt, None))
        .transpose()?;
    if query_vector.is_some() {
        validate_hooked_query(&query_vec_f32, dim)?;
    }

    // 如果 Hook 修改了查询向量，需要转回泛型 T
    if original_hook_query
        .as_ref()
        .is_some_and(|original| original != &query_vec_f32)
    {
        return Err(crate::TriviumError::InvalidInput("u64 查询保持原始位模式，浮点 Hook 查询改写不适用于二进制指纹 (u64 queries preserve their original bits; floating-point hook query rewriting is unavailable for binary fingerprints)".into()));
    }
    let hooked_query: Vec<T> =
        if let Some(original) = query_vector.filter(|_| crate::vector::is_binary_vector::<T>()) {
            original.to_vec()
        } else {
            query_vec_f32.iter().map(|&x| T::from_f32(x)).collect()
        };
    let query_vector: Option<&[T]> = if query_vector.is_some() {
        crate::vector::validate_query_vector(&hooked_query, dim)?;
        Some(&hooked_query)
    } else {
        None
    };

    let query_vector = query_vector.filter(|_| alpha != Some(0.0));
    let config = &safe_cfg;
    if collect_observations {
        ctx.record_observation("estimated_heap_bytes", mt.estimated_memory_bytes() as u64);
        ctx.record_observation("mmap_vector_bytes", mt.mmap_vector_bytes() as u64);
        ctx.record_observation("node_count", mt.node_count() as u64);
    }
    if let Some(metrics) = collect_observations
        .then(crate::observability::process_memory_snapshot)
        .flatten()
    {
        ctx.record_observation("process_rss_bytes", metrics.rss_bytes);
        for (name, faults) in [
            ("process_total_faults", metrics.total_faults),
            ("process_major_faults", metrics.major_faults),
            ("process_minor_faults", metrics.minor_faults),
        ] {
            if let Some(count) = faults {
                ctx.record_observation(name, count);
            }
        }
    }

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #2: on_custom_recall — 自定义召回
    // ═══════════════════════════════════════════════════════
    let custom_recall_result = {
        let t0 = std::time::Instant::now();
        let result = call_hook(memtable, || {
            hook.on_custom_recall(&query_vec_f32, config, ctx)
        })?;
        ctx.record_timing("hook_custom_recall", t0.elapsed());
        check_hook_error(ctx)?;
        result
    };

    // ═══════════════════════════════════════════════════════
    //  L1 + L2 + L3: 混合召回（文本 + 向量 + 布隆拦截）
    // ═══════════════════════════════════════════════════════
    let mut anchor_hits: Vec<SearchHit> = Vec::new();
    let mut seed_map: std::collections::HashMap<NodeId, f32> = std::collections::HashMap::new();
    let mut dense_ranking = Vec::new();
    let mut sparse_ranking = Vec::new();

    if let Some(custom_hits) = custom_recall_result {
        // 使用自定义召回结果，跳过内置管线
        for hit in custom_hits {
            *seed_map.entry(hit.id).or_insert(0.0) += hit.score;
        }
    } else {
        // QuIVer 构建已完成；精确回退直接读取基础层和增量层。
        recall_text_ranked(&mt, config, query_text, &mut sparse_ranking)?;
        let vector_recall_started = std::time::Instant::now();
        let vector_limit = if pure_vector {
            config.top_k
        } else if noop_hook
            && sparse_ranking.is_empty()
            && !(config.enable_advanced_pipeline && config.enable_sparse_residual)
        {
            effective_rerank_k(config)
        } else {
            effective_recall_k(config)
        };
        if let Some(query) = query_vector {
            dense_ranking = vector_recall_hits(&mt, config, query, vector_limit, pure_vector)?
                .into_iter()
                .map(|hit| (hit.id, hit.score))
                .collect();
        }
        ctx.record_timing("vector_recall", vector_recall_started.elapsed());
        let ann = query_vector.and_then(|_| ann_search_config(&mt, config, vector_limit));
        ctx.record_observation("vector_route_quiver", u64::from(ann.is_some()));
        ctx.record_observation("vector_recall_candidates", dense_ranking.len() as u64);
        if let Some(ann) = ann {
            ctx.record_observation("quiver_ef_search", ann.ef_search as u64);
            ctx.record_observation("quiver_top_k", ann.top_k as u64);
            ctx.record_observation("quiver_rerank_limit", ann.rerank_limit.unwrap_or(0) as u64);
        }
        // FISTA 基于稠密召回候选，融合后再追加影子排名。
        if let Some(alpha) = alpha.filter(|alpha| *alpha < 1.0 && !sparse_ranking.is_empty()) {
            add_rrf(&mut seed_map, &dense_ranking, alpha as f32);
            add_rrf(&mut seed_map, &sparse_ranking, (1.0 - alpha) as f32);
        } else {
            fuse_rankings_rrf(
                &mut seed_map,
                &dense_ranking,
                &sparse_ranking,
                config.text_boost,
            );
        }
        let _residual_workspace =
            if config.enable_advanced_pipeline && config.enable_sparse_residual {
                let n = seed_map.len();
                let cells = n
                    .checked_mul(n)
                    .and_then(|gram| {
                        n.checked_mul(dim + 8)
                            .and_then(|vectors| gram.checked_add(vectors))
                    })
                    .and_then(|cells| cells.checked_add(dim.saturating_mul(4)))
                    .and_then(|cells| cells.checked_mul(std::mem::size_of::<f64>()))
                    .ok_or_else(|| crate::error::TriviumError::CapacityAllocationFailed {
                        reason: "残差查询工作区估算溢出".into(),
                    })?;
                Some(mt.reserve_instance_workspace(cells)?)
            } else {
                None
            };
        if config.enable_advanced_pipeline && config.enable_sparse_residual {
            let request = crate::request::current();
            runtime.pool(runtime.parallelism())?.install(|| {
                let _scope = request
                    .as_ref()
                    .map(crate::request::RequestContext::enter)
                    .transpose()?;
                recall_residual(&mt, config, query_vector, &mut seed_map)
            })?;
        }
    }

    // 内置召回已在分支内完成 RRF；自定义召回保留调用方给出的原始分数。

    crate::request::check_current()?;

    // 将 seed_map 聚合为 anchor_hits
    let mut result_workspace = aggregate_seeds(&mt, config, &seed_map, &mut anchor_hits)?;
    ctx.record_count("dense_recall", dense_ranking.len());
    ctx.record_count("sparse_recall", sparse_ranking.len());
    ctx.record_count("fused_recall", anchor_hits.len());

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #3: on_post_recall — 召回后处理
    // ═══════════════════════════════════════════════════════
    {
        let t0 = std::time::Instant::now();
        call_hook(memtable, || hook.on_post_recall(&mut anchor_hits, ctx))?;
        ctx.record_timing("hook_post_recall", t0.elapsed());
        check_hook_error(ctx)?;
    }

    if anchor_hits.is_empty() {
        return Ok(PipelineOutput::empty());
    }

    // 补充 Payload 并构建种子集
    let mut seeds = anchor_hits;
    if !noop_hook {
        // Hook 可以替换节点；先释放旧副本，再按实际节点准入并读取。
        seeds.retain_mut(|hit| {
            hit.payload = serde_json::Value::Null;
            mt.contains(hit.id)
        });
        let bytes = hit_payload_bytes(&mt, seeds.iter().map(|hit| hit.id), seeds.capacity())?;
        result_workspace.resize_result(bytes, resident(), memory_limit)?;
        for hit in &mut seeds {
            crate::request::check_current()?;
            hit.payload = (*mt
                .try_get_payload(hit.id)?
                .ok_or(crate::TriviumError::NodeNotFound(hit.id))?)
            .clone();
        }
    }

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #4: on_pre_graph_expand — 图扩散前拦截
    // ═══════════════════════════════════════════════════════
    {
        let t0 = std::time::Instant::now();
        call_hook(memtable, || hook.on_pre_graph_expand(&mut seeds, ctx))?;
        ctx.record_timing("hook_pre_graph_expand", t0.elapsed());
        check_hook_error(ctx)?;
    }

    let semantic = seeds.iter().take(config.top_k);
    let _semantic_workspace =
        mt.reserve_request_workspace(memory_limit, cloned_hit_bytes(semantic.clone()))?;
    let semantic_hits = semantic.cloned().collect();
    let semantic_ids: std::collections::HashSet<NodeId> = seeds.iter().map(|hit| hit.id).collect();

    // ═══════════════════════════════════════════════════════
    //  L6 + L7: SA-PPR 有限深度图谱扩散 + 不应期/侧向抑制
    // ═══════════════════════════════════════════════════════
    let t_graph = std::time::Instant::now();
    let mut fatigue = crate::graph::traversal::FatigueDelta::default();
    let mut expanded = crate::graph::traversal::expand_graph_with_labels_checked(
        &*mt,
        seeds,
        config.expand_depth,
        config.teleport_alpha,
        config.enable_inverse_inhibition,
        config.lateral_inhibition_threshold,
        config.enable_refractory_fatigue,
        config.diffusion_bias.as_deref(), // CCSA: 传递扩散偏置向量
        config.expand_labels.as_deref(),
        config.max_edges_per_node,
        config.min_edge_weight,
        config.edge_direction,
        &mut fatigue,
        crate::request::check_current,
        |scores| {
            let bytes = hit_payload_bytes(
                &mt,
                scores
                    .iter()
                    .map(|&(id, _)| id)
                    .filter(|&id| mt.contains(id)),
                scores.len(),
            )?;
            result_workspace.resize_result(bytes, resident(), memory_limit)
        },
        |id| mt.try_get_payload(id),
    )?;
    ctx.record_timing("graph_expand", t_graph.elapsed());

    // L8 (时间衰减与多维重排) 已被设计哲学剥离：交由上层 Hook 或 Agent 侧处理。

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #5: on_rerank — 自定义重排序
    // ═══════════════════════════════════════════════════════
    {
        let t0 = std::time::Instant::now();
        if let Some(reranked) = call_hook(memtable, || hook.on_rerank(&mut expanded, ctx))? {
            expanded = reranked;
        }
        check_hook_error(ctx)?;
        ctx.record_timing("hook_rerank", t0.elapsed());
        ctx.record_count("rerank", expanded.len());
    }

    if config.payload_filter.is_some() {
        with_filter_callback(&mt, config.payload_filter.as_ref(), |matches| {
            expanded.retain(|hit| matches(hit.id));
            Ok(())
        })?;
    }

    let graph = expanded
        .iter()
        .filter(|hit| !semantic_ids.contains(&hit.id))
        .take(config.top_k);
    let _graph_workspace =
        mt.reserve_request_workspace(memory_limit, cloned_hit_bytes(graph.clone()))?;
    let graph_hits = graph.cloned().collect();

    // ═══════════════════════════════════════════════════════
    //  L9: DPP 多样性采样
    // ═══════════════════════════════════════════════════════
    let _dpp_workspace =
        if config.enable_advanced_pipeline && config.enable_dpp && expanded.len() > config.top_k {
            let n = expanded.len().min(config.top_k.saturating_mul(3));
            let cells = n
                .checked_mul(dim.saturating_add(config.top_k).saturating_add(8))
                .and_then(|cells| cells.checked_mul(std::mem::size_of::<f64>()))
                .ok_or_else(|| crate::error::TriviumError::CapacityAllocationFailed {
                    reason: "DPP 查询工作区估算溢出".into(),
                })?;
            Some(mt.reserve_instance_workspace(cells)?)
        } else {
            None
        };
    if config.enable_advanced_pipeline
        && config.enable_dpp
        && expanded.len() > config.top_k
        && let Some(selected) = apply_dpp(&mt, config, &mut expanded)
    {
        expanded = selected;
    } else {
        expanded.truncate(config.top_k);
    }

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #6: on_post_search — 最终后处理
    // ═══════════════════════════════════════════════════════
    {
        let t0 = std::time::Instant::now();
        call_hook(memtable, || hook.on_post_search(&mut expanded, ctx))?;
        ctx.record_timing("hook_post_search", t0.elapsed());
        check_hook_error(ctx)?;
    }

    let retained = if include_payload {
        cloned_hit_bytes(expanded.iter()).saturating_add(
            (expanded.capacity() - expanded.len()).saturating_mul(size_of::<SearchHit>()),
        )
    } else {
        expanded.len().saturating_mul(size_of::<NodeId>())
    };
    result_workspace.resize_result(retained, resident(), memory_limit)?;
    crate::request::check_current()?;
    fatigue.apply(&mt);
    Ok(PipelineOutput {
        combined_hits: expanded,
        result_workspace: Some(result_workspace),
        semantic_hits: Some(semantic_hits),
        graph_hits,
    })
}

// ═══════════════════════════════════════════════════════════
//  子管线函数：将各阶段拆为独立函数，提高可读性与可测试性
// ═══════════════════════════════════════════════════════════

fn effective_recall_k(config: &SearchConfig) -> usize {
    if config.recall_k == 0 {
        config.top_k.saturating_mul(8).max(64)
    } else {
        config.recall_k.max(config.top_k)
    }
}

fn effective_rerank_k(config: &SearchConfig) -> usize {
    let recall_k = effective_recall_k(config);
    let rerank_k = if config.rerank_k == 0 {
        config.top_k.saturating_mul(4).max(32)
    } else {
        config.rerank_k.max(config.top_k)
    };
    rerank_k.min(recall_k)
}

fn reserve_candidate_workspace<T: VectorType>(
    mt: &MemTable<T>,
    ann: Option<&QuIVerSearchConfig>,
) -> Result<crate::resources::RequestWorkspace> {
    let candidates = ann.map_or(mt.internal_slot_count(), |search| search.ef_search);
    let bytes = candidates
        .checked_mul(std::mem::size_of::<SearchHit>() + 32)
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or_else(|| crate::error::TriviumError::CapacityAllocationFailed {
            reason: "混合查询工作区估算溢出".into(),
        })?;
    // 导航位图按槽位计位；候选队列和精排列表按实际搜索宽度计项。
    let navigation = ann.map_or(0, |_| {
        mt.quiver()
            .map_or(0, |index| index.total_count())
            .div_ceil(64)
            .saturating_mul(16)
            .saturating_add(mt.dim().saturating_mul(size_of::<f32>() * 2))
    });
    mt.reserve_instance_workspace(bytes.saturating_add(navigation))
}

/// L1: 文本稀疏召回（AC 自动机精准锚点 + BM25 兜底打分）
fn recall_text_ranked<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_text: Option<&str>,
    ranking: &mut Vec<(NodeId, f32)>,
) -> Result<()> {
    if !config.enable_text_hybrid_search {
        return Ok(());
    }
    if let Some(txt) = query_text {
        let text_engine = mt.text_engine();
        let mut combined = std::collections::HashMap::<NodeId, f32>::new();
        for (id, score) in mt.search_keywords(txt)? {
            *combined.entry(id).or_insert(0.0) += score;
        }
        for (id, score) in text_engine.search_bm25(txt, config.bm25_k1, config.bm25_b) {
            *combined.entry(id).or_insert(0.0) += score;
        }
        for (id, score) in combined {
            if matches_payload_filter(mt, config.payload_filter.as_ref(), id)? {
                ranking.push((id, score));
            }
        }
        ranking.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranking.truncate(effective_recall_k(config));
    }
    Ok(())
}

#[cfg(test)]
fn recall_text<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_text: Option<&str>,
    seed_map: &mut std::collections::HashMap<NodeId, f32>,
) {
    let mut ranking = Vec::new();
    recall_text_ranked(mt, config, query_text, &mut ranking).unwrap();
    seed_map.extend(ranking);
}

#[cfg(test)]
fn recall_vector<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_vector: Option<&[T]>,
    seed_map: &mut std::collections::HashMap<NodeId, f32>,
) {
    let mut ranking = Vec::new();
    recall_vector_ranked(mt, config, query_vector, &mut ranking).unwrap();
    seed_map.extend(ranking);
}

pub(super) fn fuse_rankings_rrf(
    output: &mut std::collections::HashMap<NodeId, f32>,
    dense: &[(NodeId, f32)],
    sparse: &[(NodeId, f32)],
    sparse_weight: f32,
) {
    output.clear();
    if sparse.is_empty() {
        output.extend(dense.iter().copied());
        return;
    }
    add_rrf(output, dense, 1.0);
    add_rrf(output, sparse, sparse_weight.max(0.0));
}

fn add_rrf(
    output: &mut std::collections::HashMap<NodeId, f32>,
    ranking: &[(NodeId, f32)],
    weight: f32,
) {
    const RRF_K: f32 = 60.0;
    for (rank, &(id, _)) in ranking.iter().enumerate() {
        *output.entry(id).or_insert(0.0) += weight * (RRF_K / (RRF_K + rank as f32 + 1.0));
    }
}

/// L2 + L3: 向量稠密召回（自适应路由 + 布隆预过滤）
fn recall_vector_ranked<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_vector: Option<&[T]>,
    ranking: &mut Vec<(NodeId, f32)>,
) -> Result<()> {
    if let Some(query) = query_vector {
        let hits = vector_recall_hits(mt, config, query, effective_recall_k(config), true)?;
        ranking.extend(hits.into_iter().map(|hit| (hit.id, hit.score)));
    }
    Ok(())
}

fn vector_recall_hits<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_vector: &[T],
    limit: usize,
    admit_route: bool,
) -> Result<Vec<SearchHit>> {
    if let Some(search_cfg) = ann_search_config(mt, config, limit) {
        let _workspace = admit_route
            .then(|| reserve_candidate_workspace(mt, Some(&search_cfg)))
            .transpose()?;
        // 单条 ANN 查询在当前请求线程执行，保留线程局部搜索工作区。
        let approximate_hits = quiver_pipeline(mt, config, query_vector, &search_cfg)?;
        if config.payload_filter.is_none() || approximate_hits.len() >= config.top_k.min(limit) {
            return Ok(approximate_hits);
        }
        tracing::debug!(
            returned = approximate_hits.len(),
            requested = config.top_k,
            "QuIVer 过滤结果不足，回退精确扫描 (Insufficient filtered QuIVer results, falling back to exact scan)"
        );
    }
    // 分段精确扫描继续使用受限线程池并保持全局 slot 顺序。
    let _workspace = admit_route
        .then(|| reserve_candidate_workspace(mt, None))
        .transpose()?;
    let runtime = mt.resource_runtime();
    let request = crate::request::current();
    runtime.pool(runtime.parallelism())?.install(|| {
        let _scope = request
            .as_ref()
            .map(crate::request::RequestContext::enter)
            .transpose()?;
        brute_force_pipeline(mt, config, query_vector, mt.dim(), limit)
    })
}

fn ann_search_config<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    limit: usize,
) -> Option<QuIVerSearchConfig> {
    if crate::vector::is_binary_vector::<T>() || config.force_brute_force {
        return None;
    }
    let rows = mt.quiver()?.total_count();
    let top_k = limit.min(rows);
    let rerank_limit = config.ann_rerank_limit.unwrap_or(64).max(top_k).min(rows);
    let ef_search = effective_recall_k(config)
        .max(64)
        .max(rerank_limit)
        .min(rows);
    Some(QuIVerSearchConfig {
        top_k,
        ef_search,
        rerank_limit: Some(rerank_limit),
    })
}

/// 暴力全扫管线，供索引尚未就绪或 force_brute_force 时使用。
fn brute_force_pipeline<T: VectorType + Sync>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_vector: &[T],
    dim: usize,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let bloom_mask = config
        .payload_filter
        .as_ref()
        .map(|f| f.extract_must_have_mask())
        .unwrap_or(0);
    let (base, delta) = mt.vec_pool().vector_segments();
    with_filter_callback(mt, config.payload_filter.as_ref(), |matches| {
        let id_filter_map = |idx| eligible_node_id(mt, bloom_mask, idx).filter(|&id| matches(id));
        let control = crate::request::control();
        brute_force::search_segments_controlled(
            query_vector,
            base,
            delta,
            dim,
            limit,
            config.min_score,
            id_filter_map,
            control.as_ref(),
        )
    })
}

/// QuIVer 管线：BQ-native Vamana 图搜索（替代三级火箭）
///
/// 冷热分离架构：
/// - Hot 路径（O(log N)）：2-bit BQ 签名 beam search 遍历 Vamana 图
/// - Cold 路径（O(ef)）：仅对候选集做 f32 cosine 精排
/// - 相比三级火箭的 O(N) 全扫，大规模数据下速度提升显著
fn quiver_pipeline<T: VectorType + Sync>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_vector: &[T],
    search_cfg: &QuIVerSearchConfig,
) -> Result<Vec<SearchHit>> {
    let quiver = mt.quiver().unwrap();
    let q_f32: std::borrow::Cow<'_, [f32]> =
        if std::any::TypeId::of::<T>() == std::any::TypeId::of::<f32>() {
            std::borrow::Cow::Borrowed(bytemuck::cast_slice(query_vector))
        } else {
            std::borrow::Cow::Owned(query_vector.iter().map(|x| x.to_f32()).collect())
        };

    let filter_ref = config.payload_filter.as_ref();
    let bloom_mask = filter_ref
        .map(|filter| filter.extract_must_have_mask())
        .unwrap_or(0);

    // 冷热分离精排回调：按 MemTable slot 索引**按需**从 mmap 零拷贝取回单条向量，
    // 转为 f32 写入复用缓冲区。整个查询只触达 ~ef 个候选，
    // 不再物化全量 f32 数组（不触发 merged 缓存），冷数据始终留在 OS PageCache。
    let vec_pool = mt.vec_pool();
    let raw_results = with_filter_callback(mt, filter_ref, |matches| {
        let configured =
            mt.configured_quiver_search(&q_f32, search_cfg, &|id| matches(id), None)?;
        let raw_results = if let Some(hits) = configured {
            hits
        } else {
            quiver.search(
                &q_f32,
                |slot, buf| {
                    if !eligible_node_id(mt, bloom_mask, slot).is_some_and(matches) {
                        return false;
                    }
                    match vec_pool.get(slot) {
                        Some(v) => {
                            buf.clear();
                            buf.extend(v.iter().map(|x| x.to_f32()));
                            true
                        }
                        None => false,
                    }
                },
                search_cfg,
            )
        };

        Ok(raw_results)
    })?;
    let mut hits = Vec::new();
    for (id, score) in raw_results {
        if score >= config.min_score && matches_payload_filter(mt, filter_ref, id)? {
            hits.push(SearchHit {
                id,
                score,
                payload: serde_json::Value::Null,
            });
        }
    }
    hits.sort_unstable_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    Ok(hits)
}

/// L4 + L5: FISTA 残差搜索 + 影子查询
fn recall_residual<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_vector: Option<&[T]>,
    seed_map: &mut std::collections::HashMap<NodeId, f32>,
) -> Result<()> {
    if !config.enable_advanced_pipeline || !config.enable_sparse_residual || seed_map.is_empty() {
        return Ok(());
    }
    let query_vector = match query_vector {
        Some(qv) => qv,
        None => return Ok(()),
    };

    let filter_ref = config.payload_filter.as_ref();

    let mut entity_vecs = Vec::new();
    for &id in seed_map.keys() {
        if matches_payload_filter(mt, filter_ref, id)?
            && let Some(v) = mt.get_vector(id)
        {
            entity_vecs.push(v.iter().map(|x| x.to_f32()).collect());
        }
    }
    if entity_vecs.is_empty() {
        return Ok(());
    }
    let q_f32: Vec<f32> = query_vector.iter().map(|&x| x.to_f32()).collect();

    if let Some(residual) = crate::cognitive::fista_recall_direction(
        &q_f32,
        &entity_vecs,
        config.fista_lambda,
        80,
        config.fista_threshold,
    ) {
        tracing::debug!(
            threshold = config.fista_threshold,
            "残差超过阈值，执行二次召回 (Residual exceeds threshold, running secondary recall)"
        );
        let r_orig: Vec<T> = residual.iter().map(|&x| T::from_f32(x)).collect();
        let mut shadow_config = config.clone();
        shadow_config.recall_k = config.rerank_k;
        shadow_config.top_k = config.rerank_k;
        shadow_config.min_score = -1.0;
        let mut shadow_ranking = Vec::new();
        recall_vector_ranked(mt, &shadow_config, Some(&r_orig), &mut shadow_ranking)?;
        for (rank, (id, _)) in shadow_ranking.into_iter().enumerate() {
            // 影子分支使用排名融合，避免残差余弦与主召回分数跨量纲相加。
            let score = 0.5 * 60.0 / (61.0 + rank as f32);
            *seed_map.entry(id).or_insert(0.0) += score;
        }
    }
    Ok(())
}

fn cloned_hit_bytes<'a>(hits: impl Iterator<Item = &'a SearchHit>) -> usize {
    hits.fold(0usize, |sum, hit| {
        sum.saturating_add(size_of::<SearchHit>()).saturating_add(
            crate::storage::payload_store::query_clone_peak(&hit.payload),
        )
    })
}

fn hit_payload_bytes<T: VectorType>(
    mt: &MemTable<T>,
    ids: impl Iterator<Item = NodeId>,
    capacity: usize,
) -> Result<usize> {
    ids.into_iter().try_fold(
        capacity.saturating_mul(size_of::<SearchHit>()),
        |sum, id| {
            crate::request::check_current()?;
            Ok(sum.saturating_add(mt.payload_query_peak(id)?))
        },
    )
}

/// 将 seed_map 聚合为排序后的 anchor_hits
fn aggregate_seeds<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    seed_map: &std::collections::HashMap<NodeId, f32>,
    anchor_hits: &mut Vec<SearchHit>,
) -> Result<crate::resources::RequestWorkspace> {
    let filter_ref = config.payload_filter.as_ref();
    let mut candidates = Vec::new();
    for (&id, &score) in seed_map {
        if score >= config.min_score && matches_payload_filter(mt, filter_ref, id)? {
            candidates.push((id, score));
        }
    }
    candidates.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    candidates.truncate(effective_rerank_k(config));
    let bytes = hit_payload_bytes(mt, candidates.iter().map(|&(id, _)| id), candidates.len())?;
    let workspace = mt.reserve_instance_workspace(bytes)?;
    for (id, score) in candidates {
        if let Some(payload) = mt.try_get_payload(id)? {
            anchor_hits.push(SearchHit {
                id,
                score,
                payload: (*payload).clone(),
            });
        }
    }
    Ok(workspace)
}

#[inline]
fn matches_payload_filter<T: VectorType>(
    mt: &MemTable<T>,
    filter: Option<&crate::filter::Filter>,
    id: NodeId,
) -> Result<bool> {
    match filter {
        Some(filter) => mt.matches_payload_filter(id, filter),
        None => Ok(mt.contains(id)),
    }
}

// ANN 和并行精确扫描共用布尔回调；将首次读取错误传回请求线程。
fn with_filter_callback<T: VectorType, R>(
    mt: &MemTable<T>,
    filter: Option<&crate::filter::Filter>,
    run: impl FnOnce(&(dyn Fn(NodeId) -> bool + Sync)) -> Result<R>,
) -> Result<R> {
    if filter.is_none() {
        return run(&|id| mt.contains(id));
    }
    let error = std::sync::OnceLock::new();
    let result = run(&|id| {
        if error.get().is_some() {
            return false;
        }
        match matches_payload_filter(mt, filter, id) {
            Ok(matched) => matched,
            Err(cause) => {
                let _ = error.set(cause);
                false
            }
        }
    });
    match error.into_inner() {
        Some(cause) => Err(cause),
        None => result,
    }
}

#[inline]
fn eligible_node_id<T: VectorType>(
    mt: &MemTable<T>,
    bloom_mask: u64,
    idx: usize,
) -> Option<NodeId> {
    let id = *mt.internal_indices().get(idx)?;
    if id == 0 {
        return None;
    }
    if bloom_mask != 0
        && mt
            .fast_tags_slice()
            .get(idx)
            .is_some_and(|tag| (*tag & bloom_mask) != bloom_mask)
    {
        return None;
    }
    Some(id)
}

/// L9: DPP 多样性采样
fn apply_dpp<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    expanded: &mut [SearchHit],
) -> Option<Vec<SearchHit>> {
    let limit = config.top_k;
    let dpp_pool_size = std::cmp::min(expanded.len(), limit.saturating_mul(3));
    let mut pool_vecs = Vec::with_capacity(dpp_pool_size);
    let mut pool_scores = Vec::with_capacity(dpp_pool_size);
    let mut pool_valid = Vec::with_capacity(dpp_pool_size);

    for (i, hit) in expanded.iter().take(dpp_pool_size).enumerate() {
        if let Some(v) = mt.get_vector(hit.id) {
            pool_vecs.push(v.iter().map(|&x| x.to_f32()).collect());
            pool_scores.push(hit.score);
            pool_valid.push(i);
        }
    }

    if pool_valid.len() <= limit {
        return None;
    }

    let selected_idx =
        crate::cognitive::dpp_greedy(&pool_vecs, &pool_scores, limit, config.dpp_quality_weight);

    let mut final_results = Vec::with_capacity(limit);
    for &idx in &selected_idx {
        let hit = &mut expanded[pool_valid[idx]];
        final_results.push(SearchHit {
            id: hit.id,
            score: hit.score,
            payload: hit.payload.take(),
        });
    }
    final_results.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    Some(final_results)
}

#[cfg(test)]
fn execute_pipeline<T: VectorType>(
    memtable: &Arc<RwLock<MemTable<T>>>,
    quiver_builds: &super::QuiverBuilds,
    hook: &Arc<dyn SearchHook>,
    query_text: Option<&str>,
    query_vector: Option<&[T]>,
    config: &SearchConfig,
    ctx: &mut HookContext,
) -> Result<Vec<SearchHit>> {
    execute_pipeline_with_limit(
        memtable,
        quiver_builds,
        0,
        hook,
        query_text,
        query_vector,
        config,
        Some(ctx),
        None,
        true,
    )
    .map(|output| output.combined_hits)
}

// ═══════════════════════════════════════════════════════════
//  单元测试
// ═══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::config::SearchConfig;
    use crate::filter::Filter;
    use crate::hook::{HookContext, NoopHook, SearchHook};
    use crate::node::SearchHit;
    use crate::storage::memtable::MemTable;
    use std::sync::Arc;

    /// 构建一个包含若干 f32 节点的内存 MemTable（无磁盘 IO）
    fn make_memtable(dim: usize, nodes: &[(u64, Vec<f32>, serde_json::Value)]) -> MemTable<f32> {
        let mut mt = MemTable::new(dim);
        for (id, vec, payload) in nodes {
            mt.insert_with_id(*id, vec, payload.clone()).unwrap();
        }
        mt
    }

    fn wrap(mt: MemTable<f32>) -> Arc<RwLock<MemTable<f32>>> {
        Arc::new(RwLock::new(mt))
    }

    fn builds() -> super::super::QuiverBuilds {
        Arc::new((
            std::sync::Mutex::new(std::collections::HashMap::new()),
            std::sync::Condvar::new(),
        ))
    }

    fn default_config() -> SearchConfig {
        SearchConfig {
            top_k: 5,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        }
    }

    #[test]
    fn test_rrf融合不依赖跨量纲原始分数() {
        let dense = vec![(1, 0.9), (2, 0.8)];
        let sparse = vec![(2, 1000.0), (1, 0.001)];
        let mut fused = std::collections::HashMap::new();
        fuse_rankings_rrf(&mut fused, &dense, &sparse, 1.0);
        assert!((fused[&1] - fused[&2]).abs() < 1e-6);
    }

    #[test]
    fn test_rrf单路保持稠密原始分数() {
        let dense = vec![(1, 0.91), (2, 0.42)];
        let mut fused = std::collections::HashMap::new();
        fuse_rankings_rrf(&mut fused, &dense, &[], 1.0);
        assert_eq!(fused[&1], 0.91);
        assert_eq!(fused[&2], 0.42);
    }

    #[test]
    fn test_sanitize_config解耦候选池() {
        let mut config = SearchConfig {
            top_k: 10,
            recall_k: 0,
            rerank_k: 0,
            ..Default::default()
        };
        sanitize_config::<f32>(&mut config, 128).unwrap();
        assert_eq!(config.recall_k, 80);
        assert_eq!(config.rerank_k, 40);
    }

    // ════════ aggregate_seeds ════════

    #[test]
    fn test_aggregate_seeds_sorts_descending_and_truncates() {
        let mt = make_memtable(
            2,
            &[
                (1, vec![1.0, 0.0], serde_json::json!({"a": 1})),
                (2, vec![0.0, 1.0], serde_json::json!({"a": 2})),
                (3, vec![0.5, 0.5], serde_json::json!({"a": 3})),
            ],
        );
        let cfg = SearchConfig {
            top_k: 2,
            min_score: 0.0,
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        seed_map.insert(1u64, 0.9f32);
        seed_map.insert(2, 0.5);
        seed_map.insert(3, 0.7);

        let mut hits = Vec::new();
        aggregate_seeds(&mt, &cfg, &seed_map, &mut hits).unwrap();

        assert_eq!(hits.len(), 3);
        // 排序检查：降序
        for w in hits.windows(2) {
            assert!(w[0].score >= w[1].score, "应按分数降序");
        }
    }

    #[test]
    fn aggregate_seeds_only_hydrates_rerank_pool() {
        let mut nodes = Vec::new();
        let mut seed_map = std::collections::HashMap::new();
        for id in 1..=100 {
            nodes.push((id, vec![1.0, 0.0], serde_json::json!({"id": id})));
            seed_map.insert(id, id as f32);
        }
        let mut mt = make_memtable(2, &nodes);
        mt.configure_payload_cache(0, 0);
        let before = mt.payload_memory_stats();
        let mut hits = Vec::new();
        aggregate_seeds(
            &mt,
            &SearchConfig {
                top_k: 2,
                rerank_k: 5,
                min_score: 0.0,
                ..Default::default()
            },
            &seed_map,
            &mut hits,
        )
        .unwrap();
        let after = mt.payload_memory_stats();
        assert_eq!(hits.len(), 5);
        assert_eq!(after.payload_lookups - before.payload_lookups, 5);
        assert!(after.payload_parsed_bytes > before.payload_parsed_bytes);
    }

    #[test]
    fn test_aggregate_seeds_filters_by_min_score() {
        let mt = make_memtable(
            2,
            &[
                (1, vec![1.0, 0.0], serde_json::json!({})),
                (2, vec![0.0, 1.0], serde_json::json!({})),
            ],
        );
        let cfg = SearchConfig {
            top_k: 10,
            min_score: 0.8,
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        seed_map.insert(1u64, 0.9f32);
        seed_map.insert(2, 0.3); // 低于 min_score

        let mut hits = Vec::new();
        aggregate_seeds(&mt, &cfg, &seed_map, &mut hits).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, 1);
    }

    #[test]
    fn test_aggregate_seeds_with_payload_filter() {
        let mt = make_memtable(
            2,
            &[
                (1, vec![1.0, 0.0], serde_json::json!({"role": "admin"})),
                (2, vec![0.0, 1.0], serde_json::json!({"role": "user"})),
            ],
        );
        let cfg = SearchConfig {
            top_k: 10,
            min_score: 0.0,
            payload_filter: Some(Filter::eq("role", serde_json::json!("admin"))),
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        seed_map.insert(1u64, 0.9f32);
        seed_map.insert(2, 0.8);

        let mut hits = Vec::new();
        aggregate_seeds(&mt, &cfg, &seed_map, &mut hits).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, 1);
    }

    #[test]
    fn test_aggregate_seeds_empty_map() {
        let mt = make_memtable(2, &[(1, vec![1.0, 0.0], serde_json::json!({}))]);
        let cfg = default_config();
        let seed_map = std::collections::HashMap::new();
        let mut hits = Vec::new();
        aggregate_seeds(&mt, &cfg, &seed_map, &mut hits).unwrap();
        assert!(hits.is_empty());
    }

    // ════════ recall_vector (brute-force 路径) ════════

    #[test]
    fn test_recall_vector_basic() {
        let mut mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (2, vec![0.0, 1.0, 0.0], serde_json::json!({})),
                (3, vec![0.0, 0.0, 1.0], serde_json::json!({})),
            ],
        );
        mt.ensure_vectors_cache(true);

        let cfg = SearchConfig {
            top_k: 2,
            min_score: 0.0,
            ..Default::default()
        };
        let query: Vec<f32> = vec![1.0, 0.0, 0.0];
        let mut seed_map = std::collections::HashMap::new();

        recall_vector(&mt, &cfg, Some(&query), &mut seed_map);

        assert!(!seed_map.is_empty(), "应召回至少一个节点");
        // 节点 1 与 query 完全对齐，得分最高
        let best_id = seed_map
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert_eq!(*best_id, 1);
    }

    #[test]
    fn pure_vector_recall_does_not_touch_payload() {
        let mut mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({"text": "一"})),
                (2, vec![0.0, 1.0, 0.0], serde_json::json!({"text": "二"})),
            ],
        );
        mt.configure_payload_cache(0, 0);
        mt.ensure_vectors_cache(true);
        let before = mt.payload_memory_stats();
        let mut ranking = Vec::new();
        recall_vector_ranked(
            &mt,
            &SearchConfig {
                top_k: 2,
                min_score: -1.0,
                force_brute_force: true,
                ..Default::default()
            },
            Some(&[1.0, 0.0, 0.0]),
            &mut ranking,
        )
        .unwrap();
        let after = mt.payload_memory_stats();
        assert_eq!(ranking.len(), 2);
        assert_eq!(after.payload_lookups, before.payload_lookups);
        assert_eq!(after.payload_parsed_bytes, before.payload_parsed_bytes);
    }

    #[test]
    fn test_recall_vector_none_query_is_noop() {
        let mut mt = make_memtable(3, &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))]);
        mt.ensure_vectors_cache(true);
        let cfg = default_config();
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mt, &cfg, None, &mut seed_map);
        assert!(seed_map.is_empty());
    }

    #[test]
    fn test_recall_vector_with_payload_filter() {
        let mut mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({"tag": "yes"})),
                (2, vec![0.9, 0.1, 0.0], serde_json::json!({"tag": "no"})),
            ],
        );
        mt.ensure_vectors_cache(true);

        let cfg = SearchConfig {
            top_k: 5,
            min_score: 0.0,
            payload_filter: Some(Filter::eq("tag", serde_json::json!("yes"))),
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mt, &cfg, Some(&query), &mut seed_map);

        assert!(seed_map.contains_key(&1));
        assert!(
            !seed_map.contains_key(&2),
            "node 2 应被 payload_filter 过滤"
        );
    }

    #[test]
    fn test_recall_vector_filters_before_top_k() {
        let mut mt = make_memtable(
            2,
            &[
                (1, vec![1.0, 0.0], serde_json::json!({"group": "drop"})),
                (2, vec![0.8, 0.6], serde_json::json!({"group": "drop"})),
                (3, vec![0.6, 0.8], serde_json::json!({"group": "keep"})),
                (4, vec![0.0, 1.0], serde_json::json!({"group": "keep"})),
                (5, vec![-0.6, 0.8], serde_json::json!({"group": "keep"})),
            ],
        );
        mt.ensure_vectors_cache(true);

        let cfg = SearchConfig {
            top_k: 2,
            recall_k: 2,
            min_score: -1.0,
            force_brute_force: true,
            payload_filter: Some(Filter::eq("group", serde_json::json!("keep"))),
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mt, &cfg, Some(&[1.0, 0.0]), &mut seed_map);

        assert_eq!(seed_map.len(), 2);
        assert!(seed_map.contains_key(&3));
        assert!(seed_map.contains_key(&4));
        assert!(!seed_map.contains_key(&0));
    }

    #[test]
    fn test_recall_vector_quiver_filter_fills_top_k() {
        let mut nodes: Vec<(u64, Vec<f32>, serde_json::Value)> = (1..=40)
            .map(|id| (id, vec![1.0, 0.0], serde_json::json!({"group": "drop"})))
            .collect();
        nodes.extend([
            (100, vec![0.0, 1.0], serde_json::json!({"group": "keep"})),
            (101, vec![-0.8, 0.6], serde_json::json!({"group": "keep"})),
            (102, vec![-1.0, 0.0], serde_json::json!({"group": "keep"})),
        ]);
        let mut mt = make_memtable(2, &nodes);
        mt.build_quiver(&crate::index::quiver::QuIVerConfig::default());

        let cfg = SearchConfig {
            top_k: 2,
            recall_k: 2,
            min_score: -1.0,
            payload_filter: Some(Filter::eq("group", serde_json::json!("keep"))),
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mt, &cfg, Some(&[1.0, 0.0]), &mut seed_map);

        assert_eq!(seed_map.len(), 2);
        assert!(seed_map.contains_key(&100));
        assert!(seed_map.contains_key(&101));
    }

    // ════════ recall_text ════════

    #[test]
    fn test_recall_text_disabled_is_noop() {
        let mt = make_memtable(
            2,
            &[(1, vec![1.0, 0.0], serde_json::json!({"text": "hello"}))],
        );
        let cfg = SearchConfig {
            enable_text_hybrid_search: false,
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_text(&mt, &cfg, Some("hello"), &mut seed_map);
        assert!(seed_map.is_empty());
    }

    #[test]
    fn test_recall_text_none_query_is_noop() {
        let mt = make_memtable(
            2,
            &[(1, vec![1.0, 0.0], serde_json::json!({"text": "hello"}))],
        );
        let cfg = SearchConfig {
            enable_text_hybrid_search: true,
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_text(&mt, &cfg, None, &mut seed_map);
        assert!(seed_map.is_empty());
    }

    // ════════ recall_residual ════════

    #[test]
    fn test_recall_residual_disabled_is_noop() {
        let mut mt = make_memtable(3, &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))]);
        mt.ensure_vectors_cache(true);
        let cfg = SearchConfig {
            enable_advanced_pipeline: false,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut seed_map = std::collections::HashMap::new();
        seed_map.insert(1u64, 0.9f32);
        let before = seed_map.clone();
        recall_residual(&mt, &cfg, Some(&query), &mut seed_map).unwrap();
        assert_eq!(seed_map, before, "disabled 时 seed_map 不应变化");
    }

    #[test]
    fn test_recall_residual_empty_seeds_is_noop() {
        let mut mt = make_memtable(3, &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))]);
        mt.ensure_vectors_cache(true);
        let cfg = SearchConfig {
            enable_advanced_pipeline: true,
            enable_sparse_residual: true,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut seed_map = std::collections::HashMap::new();
        recall_residual(&mt, &cfg, Some(&query), &mut seed_map).unwrap();
        assert!(seed_map.is_empty());
    }

    // ════════ apply_dpp ════════

    #[test]
    fn test_apply_dpp_returns_none_when_pool_too_small() {
        let mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (2, vec![0.0, 1.0, 0.0], serde_json::json!({})),
            ],
        );
        let cfg = SearchConfig {
            top_k: 5,
            enable_dpp: true,
            dpp_quality_weight: 1.0,
            ..Default::default()
        };
        let mut expanded = vec![
            SearchHit {
                id: 1,
                score: 0.9,
                payload: serde_json::json!({}),
            },
            SearchHit {
                id: 2,
                score: 0.5,
                payload: serde_json::json!({}),
            },
        ];
        // pool_valid.len() <= top_k → 返回 None
        assert!(apply_dpp(&mt, &cfg, &mut expanded).is_none());
    }

    #[test]
    fn test_apply_dpp_selects_diverse_subset() {
        let mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (2, vec![0.99, 0.01, 0.0], serde_json::json!({})),
                (3, vec![0.0, 1.0, 0.0], serde_json::json!({})),
                (4, vec![0.0, 0.0, 1.0], serde_json::json!({})),
            ],
        );
        let cfg = SearchConfig {
            top_k: 2,
            enable_dpp: true,
            dpp_quality_weight: 1.0,
            ..Default::default()
        };
        let mut expanded = vec![
            SearchHit {
                id: 1,
                score: 1.0,
                payload: serde_json::json!({}),
            },
            SearchHit {
                id: 2,
                score: 0.95,
                payload: serde_json::json!({}),
            },
            SearchHit {
                id: 3,
                score: 0.8,
                payload: serde_json::json!({}),
            },
            SearchHit {
                id: 4,
                score: 0.7,
                payload: serde_json::json!({}),
            },
        ];

        let result = apply_dpp(&mt, &cfg, &mut expanded);
        assert!(result.is_some());
        let selected = result.unwrap();
        assert_eq!(selected.len(), 2);
        // DPP 应该选择多样化的组合，而不是得分最高但相似的 1 和 2
        let ids: Vec<u64> = selected.iter().map(|h| h.id).collect();
        assert!(ids.contains(&1), "最高分节点应被选中");
        // 节点 2 与 1 高度相似，DPP 倾向选择 3 或 4 而非 2
        assert!(!ids.contains(&2), "DPP 应优先选择多样化的节点而非相似节点");
    }

    // ════════ execute_pipeline 集成 ════════

    #[test]
    fn test_execute_pipeline_dimension_mismatch() {
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = default_config();
        let bad_query = vec![1.0, 0.0]; // dim=2, 期望 dim=3
        let mut ctx = HookContext::new();

        let result = execute_pipeline(
            &mt,
            &builds(),
            &hook,
            None,
            Some(&bad_query),
            &cfg,
            &mut ctx,
        );
        assert!(result.is_err(), "维度不匹配应返回错误");
    }

    #[test]
    fn test_execute_pipeline_nan_query_rejected() {
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = default_config();
        let nan_query = vec![f32::NAN, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let result = execute_pipeline(
            &mt,
            &builds(),
            &hook,
            None,
            Some(&nan_query),
            &cfg,
            &mut ctx,
        );
        assert!(result.is_err(), "NaN 查询向量应被拒绝");
    }

    #[test]
    fn test_execute_pipeline_inf_query_rejected() {
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = default_config();
        let inf_query = vec![f32::INFINITY, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let result = execute_pipeline(
            &mt,
            &builds(),
            &hook,
            None,
            Some(&inf_query),
            &cfg,
            &mut ctx,
        );
        assert!(result.is_err(), "Infinity 查询向量应被拒绝");
    }

    #[test]
    fn test_execute_pipeline_empty_db() {
        let mt = wrap(MemTable::<f32>::new(3));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = default_config();
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results =
            execute_pipeline(&mt, &builds(), &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(results.is_empty(), "空库应返回空结果");
    }

    #[test]
    fn test_execute_pipeline_basic_vector_search() {
        let mt = wrap(make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({"name": "a"})),
                (2, vec![0.0, 1.0, 0.0], serde_json::json!({"name": "b"})),
                (3, vec![0.0, 0.0, 1.0], serde_json::json!({"name": "c"})),
            ],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = SearchConfig {
            top_k: 2,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results =
            execute_pipeline(&mt, &builds(), &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].id, 1, "最相似节点应排第一");
    }

    #[test]
    fn test_execute_pipeline_respects_top_k() {
        let nodes: Vec<(u64, Vec<f32>, serde_json::Value)> = (1..=10)
            .map(|i| {
                (
                    i as u64,
                    vec![1.0, i as f32 * 0.01, 0.0],
                    serde_json::json!({}),
                )
            })
            .collect();
        let mt = wrap(make_memtable(3, &nodes));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = SearchConfig {
            top_k: 3,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results =
            execute_pipeline(&mt, &builds(), &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(results.len() <= 3, "结果数不应超过 top_k");
    }

    #[test]
    fn test_execute_pipeline_records_timings() {
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = default_config();
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let _ =
            execute_pipeline(&mt, &builds(), &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(!ctx.stage_timings.is_empty(), "管线应记录阶段计时");
        let stage_names: Vec<&str> = ctx.stage_timings.iter().map(|(n, _)| n.as_str()).collect();
        assert!(stage_names.contains(&"hook_pre_search"));
        assert!(stage_names.contains(&"hook_post_search"));
        let count_names: Vec<&str> = ctx
            .stage_counts
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert!(count_names.contains(&"dense_recall"));
        assert!(count_names.contains(&"fused_recall"));
    }

    // ════════ Hook 集成 ════════

    #[test]
    fn test_hook回调期间写重入保护已激活() {
        struct ProbeHook(
            std::sync::Arc<std::sync::atomic::AtomicBool>,
            Arc<RwLock<MemTable<f32>>>,
        );
        impl SearchHook for ProbeHook {
            fn on_pre_search(&self, _: &mut Vec<f32>, _: &mut SearchConfig, _: &mut HookContext) {
                self.0.store(
                    matches!(
                        super::super::reject_hook_reentrant_access(&self.1),
                        Err(crate::TriviumError::ReentrantAccess {
                            operation: "hook_callback"
                        })
                    ),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }
        }

        let blocked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> =
            Arc::new(ProbeHook(std::sync::Arc::clone(&blocked), mt.clone()));
        let mut ctx = HookContext::new();
        execute_pipeline(
            &mt,
            &builds(),
            &hook,
            None,
            Some(&[1.0, 0.0, 0.0]),
            &default_config(),
            &mut ctx,
        )
        .unwrap();
        assert!(blocked.load(std::sync::atomic::Ordering::SeqCst));
        assert!(super::super::reject_hook_reentrant_access(&mt).is_ok());
    }

    #[test]
    fn test_hook_abort_returns_empty() {
        struct AbortHook;
        impl SearchHook for AbortHook {
            fn on_pre_search(&self, _: &mut Vec<f32>, _: &mut SearchConfig, ctx: &mut HookContext) {
                ctx.abort = true;
            }
        }

        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(AbortHook);
        let cfg = default_config();
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results =
            execute_pipeline(&mt, &builds(), &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(results.is_empty(), "abort=true 时应返回空结果");
    }

    #[test]
    fn test_hook_custom_recall_overrides_builtin() {
        struct FixedRecallHook;
        impl SearchHook for FixedRecallHook {
            fn on_custom_recall(
                &self,
                _: &[f32],
                _: &SearchConfig,
                _: &mut HookContext,
            ) -> Option<Vec<SearchHit>> {
                Some(vec![SearchHit {
                    id: 999,
                    score: 1.0,
                    payload: serde_json::Value::Null,
                }])
            }
        }

        let mt = wrap(make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (
                    999,
                    vec![0.0, 0.0, 1.0],
                    serde_json::json!({"custom": true}),
                ),
            ],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(FixedRecallHook);
        let cfg = SearchConfig {
            top_k: 5,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results =
            execute_pipeline(&mt, &builds(), &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, 999, "自定义召回应覆盖内置召回");
    }

    #[test]
    fn test_hook_post_recall_filters() {
        struct FilterLowScoreHook;
        impl SearchHook for FilterLowScoreHook {
            fn on_post_recall(&self, hits: &mut Vec<SearchHit>, _: &mut HookContext) {
                hits.retain(|h| h.score > 0.5);
            }
        }

        let mt = wrap(make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (2, vec![0.0, 1.0, 0.0], serde_json::json!({})),
                (3, vec![0.0, 0.0, 1.0], serde_json::json!({})),
            ],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(FilterLowScoreHook);
        let cfg = SearchConfig {
            top_k: 10,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results =
            execute_pipeline(&mt, &builds(), &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        for r in &results {
            assert!(
                r.score > 0.5,
                "Hook 过滤后不应有低分结果: score={}",
                r.score
            );
        }
    }

    #[test]
    fn test_hook_rerank_reverses_order() {
        struct ReverseRerankHook;
        impl SearchHook for ReverseRerankHook {
            fn on_rerank(
                &self,
                hits: &mut Vec<SearchHit>,
                _: &mut HookContext,
            ) -> Option<Vec<SearchHit>> {
                let mut reversed = hits.clone();
                reversed.reverse();
                Some(reversed)
            }
        }

        let mt = wrap(make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (2, vec![0.7, 0.7, 0.0], serde_json::json!({})),
            ],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(ReverseRerankHook);
        let cfg = SearchConfig {
            top_k: 5,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results =
            execute_pipeline(&mt, &builds(), &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(results.len() >= 2);
        // rerank hook 反转了顺序，原本分低的现在排前面
        assert_eq!(results[0].id, 2, "rerank 反转后 node 2 应排第一");
    }

    // ════════ 参数钳位 (L0 安全防御) ════════

    #[test]
    fn 检索管线拒绝零命中数量并指向参数() {
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = SearchConfig {
            top_k: 0,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results = execute_pipeline(&mt, &builds(), &hook, None, Some(&query), &cfg, &mut ctx);
        let error = results.unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
        assert_eq!(
            serde_json::to_value(error.info().target).unwrap(),
            serde_json::json!({"field":"top_k"})
        );
    }

    // ════════ 图扩散集成 ════════

    #[test]
    fn test_pipeline_with_graph_expansion() {
        let mut mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({"name": "seed"})),
                (
                    2,
                    vec![0.0, 1.0, 0.0],
                    serde_json::json!({"name": "neighbor"}),
                ),
            ],
        );
        mt.link(1, 2, "related".to_string(), 0.8).unwrap();

        let mt = wrap(mt);
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = SearchConfig {
            top_k: 5,
            min_score: 0.0,
            expand_depth: 1,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results =
            execute_pipeline(&mt, &builds(), &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        let ids: Vec<u64> = results.iter().map(|h| h.id).collect();
        assert!(ids.contains(&2), "图扩散应将邻居节点 2 纳入结果");
    }
}
