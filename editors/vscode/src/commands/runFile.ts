import * as vscode from 'vscode';
import * as path from 'path';
import { getExecutablePath } from '../executableFinder';

// 终端复用：避免每次运行都创建新终端
let nuzoTerminal: vscode.Terminal | undefined;
let nuzoTerminalCwd: string | undefined;

// 一次性注册终端关闭监听器，清理引用以便下次重建
vscode.window.onDidCloseTerminal((closedTerminal) => {
  if (closedTerminal === nuzoTerminal) {
    nuzoTerminal = undefined;
    nuzoTerminalCwd = undefined;
  }
});

export async function runFileCommand(uri?: vscode.Uri): Promise<void> {
  const fileUri = uri ?? vscode.window.activeTextEditor?.document.uri;
  if (!fileUri) {
    vscode.window.showWarningMessage('No active Nuzo file to run.');
    return;
  }
  if (!fileUri.fsPath.endsWith('.nuzo') && !fileUri.fsPath.endsWith('.nz')) {
    vscode.window.showWarningMessage('Active file is not a Nuzo file.');
    return;
  }
  const filePath = fileUri.fsPath;
  const executable = getExecutablePath(filePath);
  if (!executable) {
    const workspaceFolders = vscode.workspace.workspaceFolders;
    const hasWorkspace = workspaceFolders && workspaceFolders.length > 0;
    const message = hasWorkspace
      ? 'Cannot find nuzo_run.exe. Please set "nuzo.executablePath" in settings.'
      : 'Please open nuzo_lang folder as workspace, or set "nuzo.executablePath" in settings.';
    const action = await vscode.window.showErrorMessage(message, 'Open Settings');
    if (action === 'Open Settings') {
      vscode.commands.executeCommand('workbench.action.openSettings', 'nuzo.executablePath');
    }
    return;
  }
  const fileDir = path.dirname(filePath);
  // 复用终端：终端不存在或工作目录变了则新建
  if (!nuzoTerminal || nuzoTerminalCwd !== fileDir) {
    nuzoTerminal = vscode.window.createTerminal({
      name: 'Nuzo',
      cwd: fileDir,
    });
    nuzoTerminalCwd = fileDir;
  }
  nuzoTerminal.show();
  // PowerShell 需要用 & 调用带引号的路径
  const callOperator = process.platform === 'win32' ? '& ' : '';
  nuzoTerminal.sendText(`${callOperator}"${executable}" run "${filePath}"`, true);
}
