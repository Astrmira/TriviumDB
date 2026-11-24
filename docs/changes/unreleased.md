# 未发布变更

**中文** | [**English**](#english)

---

## 中文说明

本页记录下一批变更。最近的预发布版本见 [0.9.4-beta.1 双语说明](0.9.4-beta.1.md)，最近的正式版本见 [0.9.3 双语说明](0.9.3.md)。

- 图作用域、文档检索与上下文的工作区同时计入实例和共享运行时。文本召回按所选倒排索引准入，并在扫描命中时检查取消。具名边 MATCH 对孤立起点计步，按投影读取端点属性，零行分页也校验排名向量。规划器从有界复合索引键统计字段相关性。
- 同一文档修订的证据窗口批量定位原文行列。命名图编辑直接读取预期修订，不受读接口默认返回大小的限制。不可变代际打开与发布每次验证只计算一遍文件摘要。Node `searchHybrid` 新增可选的 `annRerankLimit`。
- 多起点加权最短路按精确成本选路。批量路径查询共享遍历额度，`path_truncated()` 标明全路径聚合是否受枚举上限影响。最短路工作区按实例及共享额度准入。管线最终投影计入 Payload 预算与 PROFILE。
- 浮点认知阶段检查 dtype、取消和工作区，残差召回只保留 Top-K。DPP 按实际向量计算对角质量。Rust 的 `nmf_multiplicative_update` 返回 `Result<(Vec<f32>, Vec<f32>)>`，调用方可用 `?` 传播矩阵形状错误。
- Node 的普通与 Prepared TQL 均声明返回 `QueryRow[]`，节点投影的 ID 为字符串。Python 传播行构造错误，成功提交后的回执转换错误标记为已提交。代际租约保留 I/O 错误，CLI 按检查点状态标记 WAL。
- 普通 TQL 绑定按稳定身份选择 DISTINCT 代表行；普通 MATCH 完成语义校验后跳过空页扫描。控制台 MATCH CREATE 按新增节点与边收取写操作额度。
- 条件更新及属性索引复核在解码旧 Payload 前预留空间。并发图快照、全量 ID 枚举和 GraphFirst 去重计入实例与共享额度。Node 两个批量检索入口通过 Promise 返回资源准入和取消错误。
- 精确余弦 Top-K 对舍入为零的分数按 ID 决胜。检索诊断分别记录 QuIVer、精确扫描和过滤回退。维度迁移保留 Pelorus 的曲率与构建配置，回填向量沿用源库的球内约束。

- Rom 与 Mmap 按检查点内的字节边界读取属性索引定义，空库可保留完整目录。Rust `MemTable::rebuild_property_indexes_from_definitions` 返回 `Result<()>`，重建保留唯一约束，构建出错或取消时保持原目录。`index_memory_stats().persisted_bytes` 报告已发布的完整 `.idx` 文件长度，包含对齐填充。
- TQL 聚合同分时按有类型的分组值分页，跨类型比较返回 false。Node Prepared 支持完整 64 位整数域的 `bigint`。复合索引规划仅对实际使用的字段对收集统计，同次规划共享一次扫描。
- Payload 读取按 JSON 原始长度扣除请求预算，PROFILE 保留实际解码量。节点、Payload、出边、归档文档、Pelorus 查询和关键词自动机的临时内存均接受实例与共享额度检查。Rust `MemTable::build_text_index` 返回 `Result<()>`；额度不足时可调整限额后重试。图管线同时计入路径和来源占用的内存。
- WAL 组提交保留已写入帧的最高同步强度。离线迁移原子发布暂存归属，支持原命令从空暂存目录重试。CLI 导出遇到受管理文档时提示使用快照并保留目标文件。
- MATCH 聚合与同分分页按实体身份确定顺序。TQL 字符串按 JSON 规则解码转义。显式具名边分页收齐即停，FIND 返回节点属性时跳过向量和出边的复制。
- 图边分页、可达性、GraphFirst 锚点和 Python／Node.js 批量向量输入纳入实例与共享内存额度。HTTP 最短路径在找到目标后结束，已有节点到自身返回零跳路径。
- 同一向量代际的 QuIVer 构建共用一次快照和工作区。手动等待者在目标配置发布后收到成功回执。事务删除造成索引退化时，提交节点变更并失效索引，下一次构建单独申请额度。
- 过滤解码、认知重排矩阵、标量 TQL 与邻域遍历按实例剩余额度准入。排队的 Node.js 批量检索在开始执行时读取实例限额。疲劳状态在整次检索成功后更新，失败请求可按原状态重试。PROFILE 仅对实际解码计入解析字节。
- FIND 满足显式 LIMIT 后结束扫描。DPP 保持向量缩放与候选排序前缀一致，Rust NMF 查询分析使用双精度投影。只读属性索引统计按实际条目计数，Mmap 存储诊断报告当前代际的 Payload 文件。
- Server 查询根据 Accept 权重选择 JSON 或 NDJSON，节点 HTTP ETag 同时覆盖节点版本与全库代际。Windows 迁移工具按目录项原名提取制品后缀，CLI 交互命令报告底层操作错误。
- HTTP 二进制向量搜索按原始向量分数应用阈值，向量与批量节点 JSON 响应均限制为 16 MiB。Python 和 Node.js 诊断检索默认关闭图扩散。单条与批量检索统一拒绝零 Top-K。
- 检索在解码命中 Payload 前检查实例与共享额度，批量响应持续占用结果额度。WAL 恢复分别核算帧缓冲和待提交事务。WAL 回放及 Rom、Mmap 检查点加载均按 JSON 结构估算节点 Payload 的解码开销。代际清单在同一检查点持锁生成。
- 命名图遍历按返回子图计费，文档上下文按修订一次定位全部窗口。FISTA 使用双精度中间值，空候选返回空集，二次扫描按需计费。图扩散跳过零重启能量的种子循环。
- 修正通用搜索入口的 QuIVer 候选预算。默认导航宽度根据最终 Top-K 计算，显式召回池控制导航宽度。单条 ANN 查询直接在请求线程执行。纯向量查询按最终 Top-K 和内核精排预算执行，只为返回结果读取 Payload。
- 通用向量检索支持可选的 `ann_rerank_limit`（Node.js 使用 `annRerankLimit`），可独立指定 QuIVer 内核的精排候选数。导航宽度至少覆盖有效精排预算。
- 新增单条和批量 ID 检索入口。纯向量查询可跳过结果 Payload 的读取及语言对象构造。Python 单向量检索支持匹配 dtype 的连续一维缓冲区，批量 ID 检索支持二维缓冲区。
- 压缩 QuIVer 查询使用常数时间的活跃节点计数。增量删除更新该计数，已有索引在加载时从墓碑恢复计数。启用 `use_for_ann` 后，普通向量和 ID 查询可直接使用配置的压缩精排。

---

## English

This page tracks upcoming changes. See the [0.9.4-beta.1 prerelease notes](0.9.4-beta.1.md) and the [0.9.3 stable release notes](0.9.3.md).

- Scoped graph, document retrieval and context workspaces count against both the instance and shared runtime. Text retrieval admits the selected posting indexes and checks cancellation while scanning hits. Named-edge MATCH charges isolated anchors, reads endpoint properties on demand and validates ranking vectors for empty pages. Planning derives field correlations from bounded composite index keys.
- Evidence windows resolve original source coordinates together within each document revision. Named graph edits read the expected revision directly. Opening and publishing immutable generations hash each manifest file once per verification. Node `searchHybrid` accepts an optional trailing `annRerankLimit`.
- Multi-source weighted paths retain exact costs through route selection. Batched paths share traversal limits, and `path_truncated()` reports when the enumeration cap affects an aggregate. Shortest paths reserve instance and shared workspace. Final pipeline projection charges the query payload budget and appears in PROFILE.
- Floating-point cognitive stages check dtype, cancellation and workspace. Residual recall retains the top K candidates. DPP computes its quality diagonal from the actual vectors. Rust `nmf_multiplicative_update` returns `Result<(Vec<f32>, Vec<f32>)>`. Callers can propagate invalid matrix shapes with `?`.
- Node declares `QueryRow[]` for ordinary and prepared TQL, with string IDs in projected nodes. Python propagates row conversion errors and marks post-commit receipt conversion failures as committed. Generation leases preserve I/O errors. CLI WAL status follows checkpoint state.
- TQL selects DISTINCT representatives by stable binding identity for ordinary unranked matches and finds. Plain MATCH skips empty pages after semantic validation. Console MATCH CREATE charges the operation budget for new nodes and edges.
- Conditional updates and property index verification reserve memory before decoding stored payloads. Concurrent graph snapshots, full ID enumeration, and GraphFirst deduplication account for both instance and shared memory. Both Node batch search methods deliver resource admission failures and cancellation through their returned promises.
- Exact cosine Top-K breaks ties by ID when scores round to zero. Search diagnostics distinguish QuIVer, exact scans, and filtered fallback. Dimension migration preserves the Pelorus curvature and build settings so vector backfills follow the source database’s Poincaré ball constraint.

- Rom and Mmap validate property index definitions against the checkpoint length, including catalogs on empty databases. Rust `MemTable::rebuild_property_indexes_from_definitions` returns `Result<()>`, preserves uniqueness, and retains the previous catalog on failure or cancellation. `index_memory_stats().persisted_bytes` measures the published `.idx` file including alignment padding.
- TQL breaks aggregate ties using typed group values before pagination. Cross-type comparisons return false. Node Prepared accepts `bigint` across the full 64-bit integer domain. Composite-index planning collects statistics for the field pairs the query uses in a single pass.
- Payload reads charge the original JSON length on cache hits and misses. PROFILE reports bytes actually decoded. Instance and shared limits cover point reads, outgoing edges, archived documents, Pelorus scratch space, and keyword compilation. Rust `MemTable::build_text_index` returns `Result<()>` and supports retry after increasing the budget. Graph pipeline budgets and profiles include path and provenance storage.
- Group commit honors the strongest sync mode used by writes in the group. Offline migration publishes staging ownership atomically and can retry an empty legacy stage. CLI export directs managed documents to snapshots and preserves the destination on error.
- MATCH aggregation and tie-breaking use entity identities. TQL strings decode JSON escapes. Explicit named-edge pages stop at the requested row count, and FIND property projections skip vector and edge snapshots.
- Edge pages, reachability, GraphFirst anchors and Python/Node.js batch query inputs reserve both instance and shared memory. HTTP shortest-path searches stop upon finding the target and return a zero-hop path for an existing node queried against itself.
- QuIVer builds for the same vector generation share one snapshot and workspace. A manual waiter confirms publication of the requested configuration. Transactions invalidate degraded indexes after deletes, and the next build reserves its own workspace.
- Filter decoding, cognitive reranking matrices, scalar TQL and neighborhood traversal use the remaining instance budget. Queued Node.js batch searches read the instance limit when execution begins. Fatigue updates take effect after successful search completion. PROFILE counts bytes decoded on cache misses.
- FIND stops scanning once it satisfies an explicit LIMIT. DPP preserves direction under rescaling and produces consistent ranking prefixes. Rust NMF query analysis uses double-precision projection. Read-only index statistics count each posting once, and Mmap diagnostics recognize the active generation's payload file.
- Server query responses negotiate JSON and NDJSON using Accept weights. Node HTTP ETags cover both node version and database generation. The Windows migration tool extracts artifact suffixes from the actual directory entry, and the CLI reports errors from interactive commands.
- Corrected QuIVer candidate budgeting in the general search entry points. The default navigation width follows the final Top-K, and an explicit recall pool controls navigation width. Single ANN queries run directly on the request thread. Pure vector queries use the final Top-K and the kernel rerank budget, then load payloads for the returned results.
- General vector search accepts an optional `ann_rerank_limit` (`annRerankLimit` in Node.js) to set the QuIVer reranking candidate budget independently. Navigation width covers at least the effective rerank budget.
- Added single and batch ID-only search. Pure vector queries skip result payload reads and object construction. Python single-vector search accepts contiguous one-dimensional buffers of the matching dtype, and batch ID search accepts two-dimensional buffers.
- Compressed QuIVer queries read the active node count in constant time. Incremental deletions update the count, and loading an existing index reconstructs it from tombstones. With `use_for_ann` enabled, regular vector and ID searches use the configured compressed reranker directly.
- HTTP binary vector search applies the threshold to vector scores. Both vector and batch node JSON responses enforce the 16 MiB limit. Python and Node.js diagnostic search default to zero graph expansion. Single and batch vector search reject a zero Top-K.
- Search checks instance and shared memory limits before decoding hit payloads and accounts for retained batch results. WAL recovery budgets frame buffers and pending operations separately. WAL replay and checkpoint loading in both Rom and Mmap modes estimate node payload decoding costs from the JSON structure. Generation manifests are published under the checkpoint lock.
- Named graph traversal charges the returned subgraph. Document context locates all windows in one pass per revision. FISTA uses double-precision intermediates, propagates empty input and charges a secondary scan only when needed. Graph diffusion skips seed redistribution for zero restart energy.
