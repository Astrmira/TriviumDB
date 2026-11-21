# 未发布变更

**中文** | [**English**](#english)

---

## 中文说明

- TQL 的 `GRAPH` 在读取默认图的邻接表前核对节点数、边数和图视图的基础内存需求。联合查询按节点 ID 去重，并保留各图独立的边。

- 离线迁移在创建目录前排查源库制品名冲突。预检和复用核对已有目标布局及交付报告，预检失败时回收临时副本。已确认发布的目标在重试失败时仍返回已提交状态。
- TQL 保留 MATCH 模式左端的节点身份、WITH 后的边谓词和各节点自己的图指标。排序函数可引用 RETURN 中的节点别名。向量与文本查询在执行前检查变量作用域，数值等值过滤直接查询索引成员身份。
- 精确余弦在严重抵消时采用补偿点积，加权最短路径和 Yen 使用完整边权和比较候选。Pelorus 过滤、精确结果物化和文档来源标识复制在分配前申请实例与共享运行时额度。
- Node 的 TQL 字符串入口拒绝孤立 UTF-16 代理项。Rust 底层 `VecPool::push`、`update`、`zero_out` 与全图 `pagerank` 返回 `Result`，直接调用这些接口时可用 `?` 传播错误。命名图使用已维护的默认图边数进行准入，Server 按唯一索引的提交结果推进全局 ETag。

- 外部迁移工具支持带 Pelorus 配置的当前格式库，在 Rom 或 Mmap 目标上重建双曲索引。目录重命名后同步父目录失败时返回 `publish/unknown`，重试可确认目标并完成交付。
- Console 将 JSON 数值直接舍入到目标浮点类型。CLI 以已有节点检索时读取原始 dtype 的向量，保留完整 u64 指纹。Pelorus 单条和批量查询在懒重建前校验维度、有限值及球内范围。
- Leiden 在权重统一缩放时保持社区划分，自环在每层收缩后保留度数，重新计算质心会替换上次结果。图工作区准入覆盖四张邻接目录及缓冲增长，K-core 按度取点，加权最短路按完整路径字典序处理等距候选。
- TQL 支持紧邻负数字面量的比较、比较两侧的括号标量及独立空值谓词。命名图写入在输入校验失败时报告 validation，文档分批发布先取得共享内存额度再复制上传状态。

- TQL 按语句中的赋值次序更新跨匹配行重叠的节点。无显式排序的 MATCH 分页从模式左端按节点 ID 枚举。FIND 校验带前缀的变量引用，控制台的行上限适用于显式 LIMIT。交互式 CLI 通过语句解析器识别 MATCH 写入。
- 旧代际回收记录通过校验的制品清单，文件占用解除后可继续清理。外部迁移工具通过主文件头核对已有目标的 Rom 或 Mmap 布局。
- BFS 按层限制前沿，双向最短路按不同节点计费。子图 PageRank 和度中心性逐条计算平行边，拒绝缺失的成员，度中心性按节点数减一归一化，单节点分母为一。具名边匹配与全路径枚举逐边读取邻接表。
- HTML 与 Markdown 正文过滤使用自闭合拼写的 script、style 和 template 元素。JSON 与 JSONL 文档保留大指数数值的原文。

- FIND 分页排序按实际投影列解析，直接查询与 Prepared 查询共用排序语义。普通投影将每行字符串字面量计入查询内存预算。空成员列表按常量条件规划，索引建议按解析后的字段名生成。
- Server 的节点与边 NDJSON 导入支持幂等重试和版本前置条件。Writer 按请求剩余期限决定是否等待合批。JSON 与 NDJSON 对混合类型列统一返回 mixed。边分页按稳定顺序计数，仅复制当前页的元数据。Server 启动时验证数据库路径可用 UTF-8 表示。
- 文档零宽选区返回空正文和空来源范围。Python 和 Node 局部更新边时可显式清空元数据。Hash 索引在映射与堆加载模式下均拒绝重复键。专用属性查询在确认结果超限时停止核验。
- f16 精确扫描在请求预算内扩宽一次查询向量，评分线程共享查询向量与范数，直接读取候选分量。全路径枚举使用显式栈和共享路径前缀，支持按调用方预算遍历深链。

- HTTP 事务在组 WAL 同步成功后确认幂等回执。同步之前发生的实体版本读取错误会使旧 ETag 整体失效，同键重试可以取回原先分配的节点 ID。HTTP 整节点、边、搜索结果和 TQL 投影中的嵌套大整数采用同一十进制字符串编码。
- 文件组回收先持久退役发布日志，再删除恢复备份。Windows 使用同卷写穿透移动发布控制文件。代际指针替换后的同步错误携带 `publish/unknown`，调用方可读取当前代际确认结果。
- TQL 简写向量查询在文档谓词匹配集内取 Top-K。`FIND` 默认按节点 ID 分页，显式字段排序可按有序索引游标取页。`SET VECTOR` 按唯一目标写入，`MATCH … CREATE` 拒绝在已绑定节点上提供属性，集合字面量检查成员之间的逗号。
- Python 搜索 Hook 在同一请求的各阶段共享 `custom_data` 字典。回调属性求值、返回值和上下文转换失败时，检索返回 Hook 错误。
- CLI 原子导入可设置输入字节和操作数上限。Console 边读取与路径遍历在复制和展开前执行会话预算。调和中心性只维护已发现的距离，可变长路径用显式栈遍历，N-gram 在生成前按字符数准入。
- CLI 和 Server 的节点导入在写入前识别受管理文档的保留字段，整批拒绝跨库归档指针。Payload 的 `$inc` 在写 WAL 前检查浮点加法的完整增量，拒绝会发生部分舍入的补丁。
- TQL 多列 `DISTINCT` 按完整投影去重。Rust `tql_nodes()` 遇到多个同名聚合时以表达式标识各列。减法支持紧邻操作数的写法，行注释支持 CR、LF 和 CRLF。只需行重数的 MATCH 计数跳过节点快照。
- 并行 PageRank 按节点顺序归并残差。图扩散以更宽的精度计算门控，门控归零的边仍计入归一化分母。双向扩散分别计入正反边，自环只计一次。图算法先准入边扫描预算，节点对相似度排序遵守 Top-K 容量。
- GraphFirst 将请求取消传入评分线程。Server 写分发把同一请求上下文传入事务、CAS 和批删，批删先去重并校验预算。QuIVer 构建任务负责发布最终状态。归档文档查询与上下文展开共享同一份解码工作区预留。
- 存储写入统计按实例累计 WAL、检查点和索引容器写入。Windows 与 Linux 均报告总缺页数，Linux 另提供主缺页与次缺页计数。Rust `ProcessMemorySnapshot` 的分类计数由 `u64` 改为 `Option<u64>`，供调用方识别平台可用性。

---

## English

- TQL `GRAPH` checks node and edge counts and the view's base memory requirement before reading default-graph adjacency lists. A union counts each node ID once and retains each graph's edges.

- Offline migration checks for source artifact name conflicts before creating a directory. Preflight and reuse validate the published layout and report. Failed preflight runs release their temporary copies. Retry errors preserve the committed outcome after publication is confirmed.
- TQL preserves the left endpoint of MATCH, edge predicates after WITH, and metrics attached to each node binding. Sort functions resolve projected node aliases. Vector and text sources validate variable scope before execution. Numeric equality filters probe index membership directly.
- Exact cosine uses a compensated dot product under severe cancellation. Weighted shortest paths and Yen compare full sums of edge weights. Pelorus filtering, exact result hydration, and document identity copies reserve instance and shared memory before allocation.
- Node TQL rejects unpaired UTF-16 surrogates. The low-level Rust methods `VecPool::push`, `update`, `zero_out` and full-graph `pagerank` return `Result`. Direct callers can propagate errors with `?`. Named graph admission uses the maintained default graph edge count. Server advances the global ETag when a unique index change commits.

- The migration tool reads current-format databases with Pelorus configuration and rebuilds the hyperbolic index in Rom or Mmap targets. A directory sync failure after rename reports `publish/unknown`. Retrying confirms the destination and completes publication.
- The console rounds JSON numbers directly to the target floating-point type. CLI searches by node use the stored vector dtype and preserve all u64 fingerprint bits. Single and batch Pelorus queries validate dimensions, finite values and ball membership before a lazy rebuild.
- Leiden preserves partitions under uniform weight scaling, retains self-loop degree during contraction and replaces centroids on recomputation. Graph workspace admission covers four adjacency directories and buffer growth. K-core selects by degree and weighted shortest paths break distance ties in full-path lexicographic order.
- TQL supports comparisons adjacent to negative literals, parenthesized scalars on either side of a comparison and standalone null predicates. Named graph input errors report the validation phase. Document publication reserves shared memory before copying upload state.

- TQL applies assignments in statement order when a node matches several roles. Unordered MATCH pagination enumerates anchors by ID from the left of the pattern. FIND validates qualified references. Explicit LIMIT clauses respect console session caps. The interactive CLI uses the statement parser to dispatch MATCH mutations.
- Generation reclamation records the validated artifact list and resumes after file handles are released. The external migration tool checks the published file header before reusing a Rom or Mmap destination.
- BFS limits each frontier layer. Bidirectional shortest paths count distinct visited nodes. Induced PageRank and degree centrality count parallel edges and reject missing members. Degree centrality divides by the node count minus one, using one for a singleton. Named-edge matching and all-paths traversal stream adjacency lists.
- HTML and Markdown extraction skips the contents of script, style and template elements written with a self-closing slash. JSON and JSONL document ingestion preserves numeric literals with large exponents.

- FIND pagination resolves ordering against the actual projection for both direct and prepared queries. Node projections account for string literals per row. Empty membership predicates produce constant decisions, and index advice uses parsed field names.
- Node and edge NDJSON imports honor idempotency keys and generation preconditions. The writer uses the remaining request deadline when deciding whether to wait for a batch. JSON and NDJSON label heterogeneous columns as mixed. Edge pagination counts in stable order and copies metadata only for the requested page. The server validates that its database path can be represented in UTF-8 before opening it.
- Empty document selections return empty text and source spans. Python and Node edge updates can explicitly clear metadata. Both mapped and heap-backed hash indexes reject duplicate keys. Indexed property lookups stop verification once they establish that the result limit has been exceeded.
- Exact f16 scans reserve and widen the query once, share its components and norm across workers, and read candidate components directly. Both all-paths traversal APIs use an explicit stack and a shared path prefix for deep graphs.

- HTTP transactions confirm idempotent receipts after the group WAL sync succeeds. If entity-version reads fail before that point, the server invalidates prior ETags. A retry with the same key can retrieve the original allocated IDs. Nested large integers in node, edge, search and TQL responses use the same decimal string encoding.
- Checkpoint cleanup durably retires the publication journal before removing recovery backups. On Windows, control files use same-volume write-through moves. A sync error after a generation pointer switch carries `publish/unknown`; callers can read the current generation to reconcile the outcome.
- Shorthand TQL vector search ranks the nodes matching its document predicate before taking Top-K. By default, `FIND` pages by node ID. Explicit field ordering can page through an ordered index cursor. `SET VECTOR` writes each target once. `MATCH … CREATE` rejects properties on previously bound nodes, and collection literals require separators between members.
- Python search hooks share a request-local `custom_data` dictionary across stages. Errors resolving callbacks, decoding return values or converting the context propagate as hook errors.
- Atomic CLI imports support input-byte and operation limits. Console edge reads and path traversal apply session budgets before copying or expanding the graph. Harmonic centrality tracks discovered distances, variable-length paths use an explicit stack, and N-gram generation admits bounded character input before allocating grams.
- The CLI and Server node importers reject an entire batch containing reserved managed-document payloads before transferring archive references across databases. Payload `$inc` checks that the full increment survives floating-point addition before writing to the WAL.
- TQL deduplicates the full projection in multi-column `DISTINCT` queries. Rust `tql_nodes()` uses expression names for colliding aggregate columns. Subtraction accepts adjacent operands, and line comments end at CR, LF or CRLF. MATCH counts that depend only on row multiplicity skip node snapshots.
- Parallel PageRank sums convergence residuals in node order. Graph diffusion evaluates attention in higher precision and keeps fully suppressed edges in the normalization denominator. Bidirectional traversal counts reciprocal edges separately and self-loops once. Graph algorithms admit edge scans before building adjacency lists, and node similarity stays within the Top-K pair capacity.
- GraphFirst propagates request cancellation to scoring workers. Server transactions, compare-and-set and batch deletion inherit the writer request context. Batch deletion deduplicates and admits IDs before gathering edges. The QuIVer worker owns the final build status. Archived document queries and context expansion share a single reservation for decoding.
- Each database instance accumulates WAL, checkpoint and index-container write bytes. Windows and Linux report total page faults. Linux also reports minor and major faults separately. In Rust, the minor and major counters in `ProcessMemorySnapshot` change from `u64` to `Option<u64>` so callers can handle platform availability.
