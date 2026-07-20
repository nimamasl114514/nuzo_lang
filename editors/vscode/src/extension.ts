import * as vscode from 'vscode';
import * as path from 'path';
import { LanguageClient, LanguageClientOptions, ServerOptions, TransportKind } from 'vscode-languageclient/node';
import { runFileCommand } from './commands/runFile';
import { compileFileCommand } from './commands/compileFile';
import { showDisassemblyCommand } from './commands/showDisassembly';
import { getExecutablePath } from './executableFinder';

let languageClient: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext): void {
  // 注册命令（优先确保命令可用，即使 LSP 启动失败也不影响）
  context.subscriptions.push(
    vscode.commands.registerCommand('nuzo.runFile', (uri?: vscode.Uri) => runFileCommand(uri)),
    vscode.commands.registerCommand('nuzo.compileFile', (uri?: vscode.Uri) => compileFileCommand(uri)),
    vscode.commands.registerCommand('nuzo.showDisassembly', (uri?: vscode.Uri) => showDisassemblyCommand(uri)),
  );

  // 状态栏指示器：显示可执行文件查找状态
  const statusBarItem = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Right,
    100,
  );
  context.subscriptions.push(statusBarItem);
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(() => updateStatusBar(statusBarItem)),
  );
  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor(() => updateStatusBar(statusBarItem)),
  );
  updateStatusBar(statusBarItem);

  // 启动 LSP（失败不应阻塞命令注册）
  try {
    startLanguageServer(context);
  } catch (err) {
    console.error('[nuzo] Failed to start language server:', err);
  }
}

function updateStatusBar(item: vscode.StatusBarItem): void {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== 'nuzo') {
    item.hide();
    return;
  }
  const config = vscode.workspace.getConfiguration('nuzo');
  const lspEnabled = config.get<boolean>('enableLanguageServer', true);
  const executable = getExecutablePath(editor.document.uri.fsPath);
  if (executable) {
    item.text = `$(check) Nuzo: ${path.basename(executable)}`;
    item.tooltip = `Executable: ${executable}\nLSP: ${lspEnabled ? 'Active' : 'Disabled'}`;
    item.command = undefined;
  } else {
    item.text = '$(warning) Nuzo: Not Found';
    item.tooltip = 'nuzo_run not found. Click to open settings.';
    item.command = 'workbench.action.openSettings';
  }
  item.show();
}

function startLanguageServer(context: vscode.ExtensionContext): void {
  // LSP server 是同一包内的 server.js（由 src/lsp/server.ts 编译）
  const serverModule = context.asAbsolutePath(path.join('out', 'src', 'lsp', 'server.js'));
  
  const serverOptions: ServerOptions = {
    run: { module: serverModule, transport: TransportKind.ipc },
    debug: {
      module: serverModule,
      transport: TransportKind.ipc,
      options: { execArgv: ['--nolazy', '--inspect=6009'] },
    },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: 'file', language: 'nuzo' }],
    synchronize: {
      configurationSection: 'nuzo',
      fileEvents: vscode.workspace.createFileSystemWatcher('**/*.nuzo'),
    },
  };

  languageClient = new LanguageClient(
    'nuzoLanguageServer',
    'Nuzo Language Server',
    serverOptions,
    clientOptions,
  );

  languageClient.start();
}

export function deactivate(): Thenable<void> | undefined {
  if (languageClient) {
    return languageClient.stop();
  }
  return undefined;
}
