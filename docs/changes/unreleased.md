# 未发布变更

**中文** | [**English**](#english)

---

## 中文说明

Python 绑定采用 PyO3 0.30，普通 CPython 支持范围扩展到 3.9–3.15。批量检索与插入新增按数据库 dtype 读取的二维连续 `memoryview`，TQL 执行及批量写入的提交阶段释放 GIL。Node.js 22、24、26 复用 Node-API 8 模块，批量读取与写入可以使用 `Float32Array` 和 `BigUint64Array`。两端保留嵌套数组输入、结构化参数错误及原事务语义。用法见[批量检索](../api-reference.md)。

混合检索在准备 QuIVer 索引前检查查询配置和向量，BM25 统一校验 `k1 > 0` 和 `0 ≤ b ≤ 1`。批量检索对整批保留的命中及 Payload 计费。QuIVer 建图和导航模式安装的预算错误包含字节级详情。Python 批量接口为已释放的视图提供结构化错误，空矩阵也按数据库维度校验列数。

MemTable 的直接插入、替换与补丁在修改节点和索引前校验 Payload 和唯一约束。代际清单回执区分重命名失败与重命名后的同步失败。离线迁移重试核对完整 Pelorus 配置。Markdown 行内 HTML 块保留跨标签的正文及行证据，文档来源身份错误指向具体字段。Server 在打开数据库前校验请求超时能否表示为截止时间。

最近正式版本见 [0.9.3 双语发布说明](0.9.3.md)。

---

## English

The Python binding uses PyO3 0.30 and supports GIL-enabled CPython 3.9–3.15. Batch searches and inserts accept contiguous two-dimensional memoryviews of the database dtype. TQL execution and batch write commits release the GIL. Node.js 22, 24 and 26 share the Node-API 8 addon. Batch searches and inserts accept packed Float32Array or BigUint64Array inputs. Both bindings retain nested array inputs, structured argument errors and transaction semantics.

Hybrid search validates the input configuration and vector before preparing the QuIVer index. BM25 requires `k1 > 0` and `0 ≤ b ≤ 1`. Batch search accounts for all retained hits and payloads. QuIVer build and navigation-mode admission errors report memory in bytes. Python batch methods return structured errors for released views and check the column count of empty matrices.

Direct MemTable inserts, replacements and patches validate payloads and uniqueness before changing nodes or indexes. Generation manifest receipts distinguish a failed rename from a directory sync failure after the rename. Offline migration retries verify the full Pelorus configuration. Inline HTML in Markdown retains block and line evidence across tags. Invalid document source identities identify the offending field. The server validates that the request timeout yields a representable deadline before opening the database.

See the [0.9.3 bilingual release notes](0.9.3.md) for the latest stable release.
