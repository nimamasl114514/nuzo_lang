/**
 * 悬停提示包装：基于 completion.ts 的 getKeywordDocs 构建 Hover 响应。
 */

import { Hover, MarkupContent } from 'vscode-languageserver';
import { getKeywordDocs } from './completion';

export function buildHover(keyword: string): Hover | null {
  const docs = getKeywordDocs(keyword);
  if (!docs) return null;
  const content: MarkupContent = { kind: 'markdown', value: docs };
  return { contents: content };
}
