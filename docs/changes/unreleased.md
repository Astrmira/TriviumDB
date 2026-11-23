# 未发布变更

**中文** | [**English**](#english)

---

## 中文说明

本页记录下一批变更。最近的预发布版本见 [0.9.4-beta.1 双语说明](0.9.4-beta.1.md)，最近的正式版本见 [0.9.3 双语说明](0.9.3.md)。

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

- Corrected QuIVer candidate budgeting in the general search entry points. The default navigation width follows the final Top-K, and an explicit recall pool controls navigation width. Single ANN queries run directly on the request thread. Pure vector queries use the final Top-K and the kernel rerank budget, then load payloads for the returned results.
- General vector search accepts an optional `ann_rerank_limit` (`annRerankLimit` in Node.js) to set the QuIVer reranking candidate budget independently. Navigation width covers at least the effective rerank budget.
- Added single and batch ID-only search. Pure vector queries skip result payload reads and object construction. Python single-vector search accepts contiguous one-dimensional buffers of the matching dtype, and batch ID search accepts two-dimensional buffers.
- Compressed QuIVer queries read the active node count in constant time. Incremental deletions update the count, and loading an existing index reconstructs it from tombstones. With `use_for_ann` enabled, regular vector and ID searches use the configured compressed reranker directly.
- HTTP binary vector search applies the threshold to vector scores. Both vector and batch node JSON responses enforce the 16 MiB limit. Python and Node.js diagnostic search default to zero graph expansion. Single and batch vector search reject a zero Top-K.
- Search checks instance and shared memory limits before decoding hit payloads and accounts for retained batch results. WAL recovery budgets frame buffers and pending operations separately. WAL replay and checkpoint loading in both Rom and Mmap modes estimate node payload decoding costs from the JSON structure. Generation manifests are published under the checkpoint lock.
- Named graph traversal charges the returned subgraph. Document context locates all windows in one pass per revision. FISTA uses double-precision intermediates, propagates empty input and charges a secondary scan only when needed. Graph diffusion skips seed redistribution for zero restart energy.
