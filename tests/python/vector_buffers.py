"""实际 wheel 的批量向量、事务拒绝与宽整数精度契约。"""

from array import array
from pathlib import Path
import sys
from tempfile import TemporaryDirectory

from triviumdb import TriviumDB, TriviumDBError


def run_vector_buffers(root):
    path = str(Path(root) / "matrix.tdb")
    backing = array("f", [99, 1, 0, 0, 1])
    matrix = memoryview(backing)[1:].cast("B").cast("f", (2, 2))
    with TriviumDB(path, dim=2) as db:
        ids = db.batch_insert(matrix.toreadonly(), [{"key": "x"}, {"key": "y"}])
        hits = db.search_batch(matrix, top_k=1, exact=True)
        assert [[(h.id, h.payload) for h in row] for row in hits] == [
            [(ids[0], {"key": "x"})], [(ids[1], {"key": "y"})]
        ]
        for bad_input in (memoryview(array("d", [1, 0])).cast("B").cast("d", (1, 2)),
                          memoryview(array("f", [1, 0, 0, 1, 1, 0])).cast("B").cast("f", (3, 2))[::2]):
            try:
                db.batch_insert_with_ids([80, 81], bad_input, [{}, {}])
            except ValueError as exc:
                assert exc.code == "TDB_INVALID_INPUT" and exc.target["field"] == "vectors"
            else:
                raise AssertionError("应拒绝不兼容的矩阵")
        invalid_row = memoryview(array("f", [1, 0, float("nan"), 1])).cast("B").cast("f", (2, 2))
        try:
            db.batch_insert_with_ids([80, 81], invalid_row, [{}, {}])
        except TriviumDBError as exc:
            assert exc.code == "TDB_INVALID_VECTOR" and exc.write["outcome"] == "not_committed"
        else:
            raise AssertionError("第二行校验失败时必须拒绝整批写入")
    with TriviumDB(path, dim=2) as db:
        assert db.all_node_ids() == ids
        assert db.get(ids[1]).vector == [0, 1]

    bits = [2**63 + 1, 0, 2**63, 0]
    fingerprints = memoryview(array("Q", bits)).cast("B").cast("Q", (2, 2))
    with TriviumDB(str(Path(root) / "bits.tdb"), dim=2, dtype="u64") as db:
        db.batch_insert_with_ids([9, 5], fingerprints, [{}, {}])
        assert db.get(9).vector == bits[:2]
        assert [[h.id for h in row] for row in db.search_batch(fingerprints, top_k=1, exact=True)] == [[9], [5]]


def run_numpy_buffers(root):
    import numpy as np

    with TriviumDB(str(Path(root) / "half-matrix.tdb"), dim=2, dtype="f16") as db:
        matrix = np.array([[0.25, 0], [0, 0.5]], dtype=np.float16)
        db.batch_insert_with_ids([7, 3], memoryview(matrix), [{}, {}])
        assert db.get(7).vector == [0.25, 0]
        hits = db.search_batch(memoryview(matrix), top_k=1, exact=True)
        assert [[h.id for h in row] for row in hits] == [[7], [3]]
        opposite_endian = matrix.astype(matrix.dtype.newbyteorder("S"))
        try:
            db.batch_insert(memoryview(opposite_endian), [{}, {}])
        except ValueError as exc:
            assert exc.code == "TDB_INVALID_INPUT" and exc.target["field"] == "vectors"
        else:
            raise AssertionError("应在写入前拒绝异字节序矩阵")
        assert db.all_node_ids() == [3, 7]


if __name__ == "__main__":
    with TemporaryDirectory() as directory:
        if sys.argv[1:] == ["--numpy"]:
            run_numpy_buffers(directory)
        else:
            run_vector_buffers(directory)
