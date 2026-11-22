# 未发布变更

**中文** | [**English**](#english)

---

## 中文说明

本页记录下一批变更。最近的预发布版本见 [0.9.4-beta.1 双语说明](0.9.4-beta.1.md)，最近的正式版本见 [0.9.3 双语说明](0.9.3.md)。

- 修正通用搜索入口的 QuIVer 候选预算。默认导航宽度根据最终 Top-K 计算，显式召回池控制导航宽度。单条 ANN 查询直接在请求线程执行。纯向量查询按最终 Top-K 和内核精排预算执行，只为返回结果读取 Payload。
- 通用向量检索支持可选的 `ann_rerank_limit`（Node.js 使用 `annRerankLimit`），可独立指定 QuIVer 内核的精排候选数。导航宽度至少覆盖有效精排预算。
- 新增单条和批量 ID 检索入口。纯向量查询可跳过结果 Payload 的读取及语言对象构造。Python 单向量检索支持匹配 dtype 的连续一维缓冲区，批量 ID 检索支持二维缓冲区。

---

## English

This page tracks upcoming changes. See the [0.9.4-beta.1 prerelease notes](0.9.4-beta.1.md) and the [0.9.3 stable release notes](0.9.3.md).

- Corrected QuIVer candidate budgeting in the general search entry points. The default navigation width follows the final Top-K, and an explicit recall pool controls navigation width. Single ANN queries run directly on the request thread. Pure vector queries use the final Top-K and the kernel rerank budget, then load payloads for the returned results.
- General vector search accepts an optional `ann_rerank_limit` (`annRerankLimit` in Node.js) to set the QuIVer reranking candidate budget independently. Navigation width covers at least the effective rerank budget.
- Added single and batch ID-only search. Pure vector queries skip result payload reads and object construction. Python single-vector search accepts contiguous one-dimensional buffers of the matching dtype, and batch ID search accepts two-dimensional buffers.
