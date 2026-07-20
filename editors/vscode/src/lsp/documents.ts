/**
 * 文档管理：TextDocuments 增量同步。
 *
 * 注意：TextDocument 类（构造器）必须从 vscode-languageserver-textdocument 导入，
 * 它是传给 new TextDocuments(TextDocument) 的工厂；vscode-languageserver 仅导出
 * TextDocument 接口类型，无法作为值使用。
 */

import { TextDocuments } from 'vscode-languageserver';
import { TextDocument } from 'vscode-languageserver-textdocument';

export function createTextDocuments(): TextDocuments<TextDocument> {
  const documents = new TextDocuments(TextDocument);
  documents.onDidChangeContent((change) => {
    // 触发诊断（由 server.ts 注册回调）
    onDocumentChange(change.document);
  });
  return documents;
}

// 文档变更回调（由 server.ts 设置）
let onDocumentChange: (doc: TextDocument) => void = () => {};
export function setOnDocumentChange(cb: (doc: TextDocument) => void): void {
  onDocumentChange = cb;
}
