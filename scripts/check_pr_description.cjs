// 只读取 PR 元数据。工作流从目标分支加载本文件，正文和补丁均不作为代码执行。
'use strict';

const basic = ['Why / 为什么改', 'Changes / 改了什么', 'Verification / 如何验证'];
const additional = ['Invariants / 不变量', 'Compatibility / 兼容性', 'Failure behavior / 失败行为'];
const headings = new Map();
for (const label of [...basic, ...additional]) {
  for (const alias of [label, ...label.split(' / ')]) {
    headings.set(alias.toLowerCase(), label);
  }
}

function sensitivePath(path) {
  if (!path || /(?:^|\/)(?:tests?|benches|examples|docs)\/|\.md$/i.test(path)) return false;
  return /^(?:src\/(?:storage|database|query|sdk|bindings|error)|python\/triviumdb|crates\/triviumdb-migrate\/(?:src|source))\//.test(path)
    || /^src\/index\/(?:bq|quiver|pelorus)[./_]/.test(path)
    || /^src\/(?:lib|resources|request|error|node|vector|hook|filter|document|named_graph)\.rs$/.test(path)
    || path === 'triviumdb.d.ts' || path === 'index.js';
}

function missingSections(body, sensitive) {
  const filled = new Set();
  let section;
  let depth = 0;
  let fence;
  // 跳过模板提示。代码块中的标题是用户提供的样例，不能开启新的说明栏。
  for (const line of (body || '').replace(/<!--[\s\S]*?(?:-->|$)/g, '').split(/\r?\n/)) {
    const marker = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (marker && !fence) {
      fence = marker[1];
      continue;
    }
    if (marker && fence && marker[1][0] === fence[0] && marker[1].length >= fence.length && !marker[2].trim()) {
      fence = undefined;
      continue;
    }
    const heading = !fence && line.match(/^ {0,3}(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (heading) {
      const label = headings.get(heading[2].trim().toLowerCase());
      if (label || heading[1].length <= depth) {
        section = label;
        depth = heading[1].length;
      }
      continue;
    }
    const value = line.trim().replace(/^[-*+]\s+/, '').replace(/^[*_`]+|[*_`]+$/g, '');
    if (section && /[\p{L}\p{N}]/u.test(value) && !/^(?:n\/?a|none|tbd|todo|待填|待补充|不适用|无)[.!。！]?$/i.test(value)) {
      filled.add(section);
    }
  }
  return (sensitive ? [...basic, ...additional] : basic).filter(label => !filled.has(label));
}

async function check({github, context, core}) {
  const params = {...context.repo, pull_number: context.payload.pull_request.number};
  // 重新运行旧任务时也检查最新正文。文件列表包含重命名前的路径。
  const {data: pull} = await github.rest.pulls.get(params);
  let sensitive = false;
  let count = 0;
  for await (const {data: files} of github.paginate.iterator(github.rest.pulls.listFiles, {...params, per_page: 100})) {
    count += files.length;
    sensitive = files.some(file => sensitivePath(file.filename) || sensitivePath(file.previous_filename));
    if (sensitive) break;
  }
  // GitHub 文件列表有上限，无法核对完整差异时按高风险变更检查。
  const missing = missingSections(pull.body, sensitive || count !== pull.changed_files);
  if (missing.length) {
    core.setFailed('Please fill in these sections in English or Chinese / 请用英文或中文补充：' + missing.join(', '));
  }
}

module.exports = check;
