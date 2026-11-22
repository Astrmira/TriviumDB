'use strict'

const assert = require('node:assert/strict')
const path = require('node:path')

module.exports = async function vectorBuffers(root, TriviumDB) {
  const file = path.join(root, 'matrix.tdb')
  const matrix = new Float32Array([99, 1, 0, 0, 1]).subarray(1)
  const db = new TriviumDB(file, { dim: 2 })
  const ids = db.batchInsert(matrix, [{ key: 'x' }, { key: 'y' }])
  const pending = db.searchBatch(matrix, 1, 1, 0, true)
  matrix.fill(0)
  assert.deepEqual((await pending).map(row => row.map(h => [h.id, h.payload])), [
    [[ids[0], { key: 'x' }]], [[ids[1], { key: 'y' }]],
  ])
  const detached = new Float32Array(2)
  structuredClone(detached.buffer, { transfer: [detached.buffer] })
  for (const invalid of [new Float32Array(3), new Float64Array(4),
    new Float32Array(new SharedArrayBuffer(16)), detached]) {
    assert.throws(() => db.batchInsertWithIds([80, 81], invalid, [{}, {}]),
      e => e.code === 'TDB_INVALID_INPUT' && e.target.field === 'vectors')
  }
  assert.throws(() => db.batchInsertWithIds([80, 81], new Float32Array([1, 0, NaN, 1]), [{}, {}]),
    e => e.code === 'TDB_INVALID_VECTOR' && e.write.outcome === 'not_committed')
  assert.deepEqual(await db.searchBatch(new Float32Array(0)), [])
  db.close()
  const reopened = new TriviumDB(file, { dim: 2 })
  assert.deepEqual(reopened.allNodeIds(), ids)
  assert.deepEqual(reopened.get(ids[1]).vector, [0, 1])
  reopened.close()

  const half = new TriviumDB(path.join(root, 'half-matrix.tdb'), { dim: 2, dtype: 'f16' })
  const halves = new Float32Array([0.25, 0, 0, 0.5])
  half.batchInsertWithIds([7, 3], halves, [{}, {}])
  assert.deepEqual(half.get(7).vector, [0.25, 0])
  assert.deepEqual((await half.searchBatch(halves, 1, 1, 0, true)).map(row => row.map(h => h.id)), [[7], [3]])
  half.close()

  const integers = new BigUint64Array([(1n << 63n) + 1n, 0n, 1n << 63n, 0n])
  const bits = new TriviumDB(path.join(root, 'bits.tdb'), { dim: 2, dtype: 'u64' })
  bits.batchInsertWithIds([9, 5], integers, [{}, {}])
  assert.deepEqual((await bits.searchBatch(integers, 1, 1, 0, true)).map(row => row.map(h => h.id)), [[9], [5]])
  bits.close()
}
