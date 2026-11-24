'use strict'

const assert = require('node:assert/strict')

function invalid(field, write) {
  return error => {
    assert.equal(error.schema_version, 1)
    assert.equal(error.code, 'TDB_INVALID_INPUT')
    assert.deepEqual(error.target, { field })
    assert.equal(error.stage, undefined)
    assert.deepEqual(error.write, write)
    assert.deepEqual(Object.keys(error.details), ['reason'])
    assert.equal(typeof error.details.reason, 'string')
    assert.ok(error.details.reason.length > 0)
    return true
  }
}

module.exports = { invalid }
