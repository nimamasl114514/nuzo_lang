import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';

function findExecutableFromFileDir(filePath: string): string | null {
  let dir = path.dirname(filePath);
  for (let i = 0; i < 10; i++) {
    const debugPath = path.join(dir, 'target', 'debug', 'nuzo_run.exe');
    const releasePath = path.join(dir, 'target', 'release', 'nuzo_run.exe');
    if (fs.existsSync(debugPath)) return debugPath;
    if (fs.existsSync(releasePath)) return releasePath;
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

export function getExecutablePath(filePath: string): string | null {
  const config = vscode.workspace.getConfiguration('nuzo');
  const configured = config.get<string>('executablePath', '');
  if (configured && fs.existsSync(configured)) return configured;
  const fromFile = findExecutableFromFileDir(filePath);
  if (fromFile) return fromFile;
  const workspaceFolders = vscode.workspace.workspaceFolders;
  if (workspaceFolders && workspaceFolders.length > 0) {
    for (const folder of workspaceFolders) {
      const root = folder.uri.fsPath;
      const dbg = path.join(root, 'target', 'debug', 'nuzo_run.exe');
      const rel = path.join(root, 'target', 'release', 'nuzo_run.exe');
      if (fs.existsSync(dbg)) return dbg;
      if (fs.existsSync(rel)) return rel;
    }
  }
  // 尝试在 PATH 中查找
  const executableName = process.platform === 'win32' ? 'nuzo_run.exe' : 'nuzo_run';
  try {
    const which = process.platform === 'win32' ? 'where' : 'which';
    const result = require('child_process').execSync(`${which} ${executableName}`, { encoding: 'utf-8' });
    const foundPath = result.split('\n')[0].trim();
    if (foundPath && fs.existsSync(foundPath)) return foundPath;
  } catch { /* 不在 PATH 中 */ }
  return null;
}
