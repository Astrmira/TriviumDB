# 未发布变更

**中文** | [**English**](#english)

---

## 中文说明

- CLI 和 Server 的节点导入在写入前识别受管理文档的保留字段，整批拒绝跨库归档指针。Payload 的 `$inc` 在写 WAL 前检查浮点加法的完整增量，拒绝会发生部分舍入的补丁。
- TQL 多列 `DISTINCT` 按完整投影去重。Rust `tql_nodes()` 遇到多个同名聚合时以表达式标识各列。减法支持紧邻操作数的写法，行注释支持 CR、LF 和 CRLF。只需行重数的 MATCH 计数跳过节点快照。
- 并行 PageRank 按节点顺序归并残差。图扩散以更宽的精度计算门控，门控归零的边仍计入归一化分母。双向扩散分别计入正反边，自环只计一次。图算法先准入边扫描预算，节点对相似度排序遵守 Top-K 容量。
- GraphFirst 将请求取消传入评分线程。Server 写分发把同一请求上下文传入事务、CAS 和批删，批删先去重并校验预算。QuIVer 构建任务负责发布最终状态。归档文档查询与上下文展开共享同一份解码工作区预留。
- 存储写入统计按实例累计 WAL、检查点和索引容器写入。Windows 与 Linux 均报告总缺页数，Linux 另提供主缺页与次缺页计数。Rust `ProcessMemorySnapshot` 的分类计数由 `u64` 改为 `Option<u64>`，供调用方识别平台可用性。

---

## English

- The CLI and Server node importers reject an entire batch containing reserved managed-document payloads before transferring archive references across databases. Payload `$inc` checks that the full increment survives floating-point addition before writing to the WAL.
- TQL deduplicates the full projection in multi-column `DISTINCT` queries. Rust `tql_nodes()` uses expression names for colliding aggregate columns. Subtraction accepts adjacent operands, and line comments end at CR, LF or CRLF. MATCH counts that depend only on row multiplicity skip node snapshots.
- Parallel PageRank sums convergence residuals in node order. Graph diffusion evaluates attention in higher precision and keeps fully suppressed edges in the normalization denominator. Bidirectional traversal counts reciprocal edges separately and self-loops once. Graph algorithms admit edge scans before building adjacency lists, and node similarity stays within the Top-K pair capacity.
- GraphFirst propagates request cancellation to scoring workers. Server transactions, compare-and-set and batch deletion inherit the writer request context. Batch deletion deduplicates and admits IDs before gathering edges. The QuIVer worker owns the final build status. Archived document queries and context expansion share a single reservation for decoding.
- Each database instance accumulates WAL, checkpoint and index-container write bytes. Windows and Linux report total page faults. Linux also reports minor and major faults separately. In Rust, the minor and major counters in `ProcessMemorySnapshot` change from `u64` to `Option<u64>` so callers can handle platform availability.
