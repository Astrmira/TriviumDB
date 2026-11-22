# 未发布变更

**中文** | [**English**](#english)

---

## 中文说明

Python 绑定采用 PyO3 0.30，普通 CPython 支持范围扩展到 3.9–3.15。批量检索与插入新增按数据库 dtype 读取的二维连续 `memoryview`，TQL 执行及批量写入的提交阶段释放 GIL。Node.js 22、24、26 复用 Node-API 8 模块，批量读取与写入可以使用 `Float32Array` 和 `BigUint64Array`。两端保留嵌套数组输入、结构化参数错误及原事务语义。用法见[批量检索](../api-reference.md)。

最近正式版本见 [0.9.3 双语发布说明](0.9.3.md)。

---

## English

The Python binding uses PyO3 0.30 and supports GIL-enabled CPython 3.9–3.15. Batch searches and inserts accept contiguous two-dimensional memoryviews of the database dtype. TQL execution and batch write commits release the GIL. Node.js 22, 24 and 26 share the Node-API 8 addon. Batch searches and inserts accept packed Float32Array or BigUint64Array inputs. Both bindings retain nested array inputs, structured argument errors and transaction semantics.

See the [0.9.3 bilingual release notes](0.9.3.md) for the latest stable release.
