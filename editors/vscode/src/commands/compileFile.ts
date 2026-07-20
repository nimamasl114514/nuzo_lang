import * as vscode from 'vscode';
import * as path from 'path';
import { execFile } from 'child_process';
import { promisify } from 'util';
import { getExecutablePath } from '../executableFinder';

const execFileAsync = promisify(execFile);

export async function compileFileCommand(uri?: vscode.Uri): Promise<void> {
  const fileUri = uri ?? vscode.window.activeTextEditor?.document.uri;
  if (!fileUri) {
    vscode.window.showWarningMessage('No active Nuzo file to compile.');
    return;
  }
  if (!fileUri.fsPath.endsWith('.nuzo') && !fileUri.fsPath.endsWith('.nz')) {
    vscode.window.showWarningMessage('Active file is not a Nuzo file.');
    return;
  }
  const filePath = fileUri.fsPath;
  const executable = getExecutablePath(filePath);
  if (!executable) {
    vscode.window.showErrorMessage('Cannot find nuzo_run.exe. Please set "nuzo.executablePath" in settings.');
    return;
  }
  await vscode.window.withProgress({
    location: vscode.ProgressLocation.Notification,
    title: 'Compiling Nuzo file...',
    cancellable: false,
  }, async () => {
    try {
      const { stdout, stderr } = await execFileAsync(executable, ['check', filePath], {
        cwd: path.dirname(filePath),
        maxBuffer: 10 * 1024 * 1024,
        timeout: 30_000,
        windowsHide: true,
      });
      if (stderr.length === 0) {
        vscode.window.showInformationMessage('Compilation successful.');
      } else {
        const output = vscode.window.createOutputChannel('Nuzo Compile');
        output.clear();
        output.appendLine(`Compilation failed for ${path.basename(filePath)}`);
        output.appendLine('---');
        output.append(stderr || stdout || 'Unknown error');
        output.show(true);
      }
    } catch (err: unknown) {
      const e = err as { stderr?: string; stdout?: string };
      const output = vscode.window.createOutputChannel('Nuzo Compile');
      output.clear();
      output.appendLine(`Compilation failed for ${path.basename(filePath)}`);
      output.appendLine('---');
      output.append(e.stderr || e.stdout || String(err));
      output.show(true);
    }
  });
}