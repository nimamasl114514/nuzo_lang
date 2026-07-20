/**
 * 关键字补全与关键字文档。
 *
 * KEYWORD_INFOS 是关键字元数据的唯一数据源，completion 与 hover 共享。
 */

import { CompletionItem, CompletionItemKind } from 'vscode-languageserver';

interface KeywordInfo { keyword: string; detail: string; docs: string; }

const KEYWORD_INFOS: KeywordInfo[] = [
  // 声明
  { keyword: 'fn', detail: '函数声明', docs: '声明函数: `fn name(params) { body }`' },
  { keyword: 'return', detail: '返回语句', docs: '从函数返回: `return value;`' },
  { keyword: 'import', detail: '导入模块', docs: '导入模块: `import "module" as alias;`' },
  { keyword: 'as', detail: '别名', docs: '给导入的模块起别名: `import "module" as m;`' },
  { keyword: 'lazy', detail: '懒求值', docs: '延迟求值: `lazy expr`' },
  // 控制流
  { keyword: 'if', detail: '条件分支', docs: '条件判断: `if cond { ... } else { ... }`' },
  { keyword: 'else', detail: '否则分支', docs: 'if 的否则分支: `else { ... }`' },
  { keyword: 'while', detail: '当型循环', docs: '条件循环: `while cond { ... }`' },
  { keyword: 'for', detail: '遍历循环', docs: '遍历集合: `for item in iterable { ... }`' },
  { keyword: 'in', detail: '遍历介词', docs: 'for 循环中的介词: `for x in list { ... }`' },
  { keyword: 'loop', detail: '无限循环', docs: '无限循环: `loop { ... }`' },
  { keyword: 'break', detail: '跳出循环', docs: '跳出当前循环: `break;`' },
  { keyword: 'continue', detail: '继续循环', docs: '跳到下一次循环: `continue;`' },
  { keyword: 'match', detail: '模式匹配', docs: '模式匹配: `match value { pat => expr, ... }`' },
  // 异常处理
  { keyword: 'try', detail: '异常捕获', docs: '捕获异常: `try { ... } catch (e) { ... }`' },
  { keyword: 'catch', detail: '异常处理', docs: '处理异常: `catch (e) { ... }`' },
  { keyword: 'out', detail: '抛出异常', docs: '抛出异常: `out Error("msg");`' },
  { keyword: 'keep', detail: '始终执行', docs: '无论是否异常都执行: `keep { ... }`' },
  // 逻辑运算
  { keyword: 'and', detail: '逻辑与', docs: '逻辑与: `a and b`' },
  { keyword: 'or', detail: '逻辑或', docs: '逻辑或: `a or b`' },
  { keyword: 'not', detail: '逻辑非', docs: '逻辑非: `not x`' },
  // 字面量
  { keyword: 'true', detail: '布尔真值', docs: '布尔真值字面量' },
  { keyword: 'false', detail: '布尔假值', docs: '布尔假值字面量' },
  { keyword: 'nil', detail: '空值', docs: '空值字面量（null）' },
];

export function getCompletions(prefix: string): CompletionItem[] {
  const items: CompletionItem[] = [];
  for (const info of KEYWORD_INFOS) {
    if (prefix === '' || info.keyword.startsWith(prefix)) {
      items.push({
        label: info.keyword,
        kind: CompletionItemKind.Keyword,
        detail: info.detail,
        documentation: { kind: 'markdown', value: info.docs },
        insertText: info.keyword,
      });
    }
  }
  return items;
}

export function getKeywordDocs(keyword: string): string | null {
  const info = KEYWORD_INFOS.find((i) => i.keyword === keyword);
  return info ? `**${info.keyword}** — ${info.detail}\n\n${info.docs}` : null;
}
