import * as path from 'path';
import * as vscode from 'vscode';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind
} from 'vscode-languageclient/node';

let client: LanguageClient;

export function activate(context: vscode.ExtensionContext) {
  // Get the configuration for the Django Language Server
  const config = vscode.workspace.getConfiguration('djangoLanguageServer');
  const command = config.get<string>('command') || 'djls';
  const args = config.get<string[]>('args') || ['serve'];

  // Create the server options
  const serverOptions: ServerOptions = {
    command,
    args,
    transport: TransportKind.stdio
  };

  // Create the client options
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'django-html' },
      { scheme: 'file', language: 'htmldjango' },
      { scheme: 'file', language: 'python' }
    ],
    synchronize: {
      // Notify the server about file changes to Django files contained in the workspace
      fileEvents: vscode.workspace.createFileSystemWatcher('**/{*.py,*.html,*.djhtml}')
    }
  };

  // Create and start the client
  client = new LanguageClient(
    'djangoLanguageServer',
    'Django Language Server',
    serverOptions,
    clientOptions
  );

  // Start the client. This will also launch the server
  client.start();

  console.log('Django Language Server extension is now active!');
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  return client.stop();
}