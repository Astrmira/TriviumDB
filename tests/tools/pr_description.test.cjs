'use strict';

const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const check = require('../../scripts/check_pr_description.cjs');

const template = readFileSync(path.join(__dirname, '../../.github/PULL_REQUEST_TEMPLATE.md'), 'utf8');
const basic = '## Why\nUsers lost the input on retry.\n## Changes\nKeep the input.\n## Verification\n### Steps\nRetried with the same input.';
const risk = '\n### 不变量\n保留原始数据。\n### 兼容性\n沿用已有错误码。\n### 失败行为\n返回错误并允许重试。';

async function evaluate(body, pages, changedFiles = pages.flat().length) {
  const messages = [];
  const params = {owner: 'Astrmira', repo: 'TriviumDB', pull_number: 42};
  const listFiles = Symbol('listFiles');
  const github = {
    rest: {pulls: {
      listFiles,
      get: async args => {
        assert.deepEqual(args, params);
        return {data: {body, changed_files: changedFiles}};
      },
    }},
    paginate: {async *iterator(method, args) {
      assert.equal(method, listFiles);
      assert.deepEqual(args, {...params, per_page: 100});
      for (const data of pages) yield {data};
    }},
  };
  const context = {repo: {owner: 'Astrmira', repo: 'TriviumDB'}, payload: {pull_request: {number: 42, body: '旧正文'}}};
  await check({github, context, core: {setFailed: message => messages.push(message)}});
  return messages;
}

test('空模板和注释不能充当回答，普通文档 PR 可任选一种语言', async () => {
  const docs = [[{filename: 'docs/testing.md'}]];
  for (const empty of [null, template]) {
    const [message] = await evaluate(empty, docs);
    assert.match(message, /Why .*Changes .*Verification/);
    assert.doesNotMatch(message, /Invariants/);
  }
  const chinese = '## 为什么改\n补充用法。\n## 改了什么\n加入查询示例。\n## 如何验证\n执行了两个查询。';
  assert.deepEqual(await evaluate(chinese, docs), []);
  assert.deepEqual(await evaluate(basic, docs), []);
});

test('代码块里的标题属于当前栏，待填提示不算验证', async () => {
  const body = '## Why\nA user hit a mismatch.\n## Changes\n```markdown\n## Verification\nexample output\n```\n## Verification\n<!-- 本地核对 -->\n- TODO';
  const [message] = await evaluate(body, [[{filename: 'README.md'}]]);
  assert.match(message, /：Verification \/ 如何验证$/);
});

test('跨页重命名按原路径识别高风险，验证证据只需填写一次', async () => {
  const pages = [[{filename: 'README.md'}], [{filename: 'archive.rs', previous_filename: 'src/storage/wal.rs'}]];
  const [message] = await evaluate(basic, pages);
  assert.match(message, /Invariants .*Compatibility .*Failure behavior/);
  assert.doesNotMatch(message, /Verification/);
  assert.deepEqual(await evaluate(basic + risk, pages), []);
});

test('公共声明或清单不完整时要求兼容性说明，普通回归测试使用基本栏目', async () => {
  for (const filename of ['python/triviumdb/__init__.pyi', 'triviumdb.d.ts', 'src/index/pelorus/search.rs']) {
    const [message] = await evaluate(basic, [[{filename}]]);
    assert.match(message, /Compatibility/);
  }
  const tests = [[{filename: 'tests/core/pelorus.rs'}]];
  assert.deepEqual(await evaluate(basic, tests), []);
  const [message] = await evaluate(basic, tests, 3001);
  assert.match(message, /Compatibility/);
});
