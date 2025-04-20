"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.activate = activate;
exports.deactivate = deactivate;
const vscode = __importStar(require("vscode"));
const node_1 = require("vscode-languageclient/node");
let client;
function activate(context) {
    // Get the configuration for the Django Language Server
    const config = vscode.workspace.getConfiguration('djangoLanguageServer');
    const command = config.get('command') || 'djls';
    const args = config.get('args') || ['serve'];
    // Create the server options
    const serverOptions = {
        command,
        args,
        transport: node_1.TransportKind.stdio
    };
    // Create the client options
    const clientOptions = {
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
    client = new node_1.LanguageClient('djangoLanguageServer', 'Django Language Server', serverOptions, clientOptions);
    // Start the client. This will also launch the server
    client.start();
    console.log('Django Language Server extension is now active!');
}
function deactivate() {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
//# sourceMappingURL=extension.js.map