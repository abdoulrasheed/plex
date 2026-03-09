import * as vscode from 'vscode';
import * as fs from 'fs';
import * as path from 'path';
import { PlexBridge } from './bridge';
import { SymbolTreeProvider } from './symbolTree';
import { SearchResultsProvider } from './searchResults';
import { CallGraphProvider } from './callGraph';
import { spawn, ChildProcess } from 'child_process';

let bridge: PlexBridge;
let symbolTree: SymbolTreeProvider;
let searchResults: SearchResultsProvider;
let callGraphProvider: CallGraphProvider;
let plexServeProcess: ChildProcess | null = null;
let dashboardPanel: vscode.WebviewPanel | null = null;
let statusBarItem: vscode.StatusBarItem;
const DASHBOARD_PORT = 7778;

export function activate(context: vscode.ExtensionContext) {
    bridge = new PlexBridge();
    bridge.setExtensionPath(context.extensionPath);

    symbolTree = new SymbolTreeProvider(bridge);
    searchResults = new SearchResultsProvider(bridge);
    callGraphProvider = new CallGraphProvider(bridge);

    vscode.window.registerTreeDataProvider('plex.symbolTree', symbolTree);
    vscode.window.registerTreeDataProvider('plex.searchResults', searchResults);
    vscode.window.registerTreeDataProvider('plex.callGraphView', callGraphProvider);

    // --- Status Bar ---
    statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
    statusBarItem.command = 'plex.stats';
    statusBarItem.text = '$(zap) Plex';
    statusBarItem.tooltip = 'Plex Code Intelligence — Click for stats';
    statusBarItem.show();
    context.subscriptions.push(statusBarItem);

    // Try to load initial stats
    updateStatusBar();

    // --- Commands ---

    context.subscriptions.push(
        vscode.commands.registerCommand('plex.index', async () => {
            const config = vscode.workspace.getConfiguration('plex');
            const useEmbeddings = config.get<boolean>('embeddings', true);
            
            await vscode.window.withProgress(
                {
                    location: vscode.ProgressLocation.Notification,
                    title: 'Plex: Indexing project...',
                    cancellable: false,
                },
                async () => {
                    try {
                        const result = await bridge.index(useEmbeddings);
                        vscode.window.showInformationMessage(`Plex: ${result}`);
                        symbolTree.refresh();
                        updateStatusBar();
                    } catch (e: any) {
                        vscode.window.showErrorMessage(`Plex index failed: ${e.message}`);
                    }
                }
            );
        }),

        vscode.commands.registerCommand('plex.search', async () => {
            const query = await vscode.window.showInputBox({
                prompt: 'Search your codebase (semantic + text)',
                placeHolder: 'e.g. authentication middleware, payment handler...',
            });
            if (!query) return;

            try {
                const results = await bridge.search(query);
                searchResults.setResults(results, query);
                vscode.commands.executeCommand('plex.searchResults.focus');
            } catch (e: any) {
                vscode.window.showErrorMessage(`Plex search failed: ${e.message}`);
            }
        }),

        vscode.commands.registerCommand('plex.callGraph', async () => {
            const editor = vscode.window.activeTextEditor;
            let symbolName: string | undefined;

            if (editor && !editor.selection.isEmpty) {
                symbolName = editor.document.getText(editor.selection);
            }

            if (!symbolName) {
                symbolName = await vscode.window.showInputBox({
                    prompt: 'Enter function/method name to trace',
                    placeHolder: 'e.g. handleRequest, pay_vendor...',
                });
            }
            if (!symbolName) return;

            try {
                const graph = await bridge.callGraph(symbolName);
                callGraphProvider.setGraph(graph, symbolName);
                vscode.commands.executeCommand('plex.callGraphView.focus');
            } catch (e: any) {
                vscode.window.showErrorMessage(`Plex call graph failed: ${e.message}`);
            }
        }),

        vscode.commands.registerCommand('plex.stats', async () => {
            try {
                const stats = await bridge.stats();
                vscode.window.showInformationMessage(`Plex: ${stats}`);
            } catch (e: any) {
                vscode.window.showErrorMessage(`Plex stats failed: ${e.message}`);
            }
        }),

        vscode.commands.registerCommand('plex.symbolInfo', async () => {
            const editor = vscode.window.activeTextEditor;
            let symbolName: string | undefined;

            if (editor && !editor.selection.isEmpty) {
                symbolName = editor.document.getText(editor.selection);
            }

            if (!symbolName) {
                symbolName = await vscode.window.showInputBox({
                    prompt: 'Enter symbol name',
                });
            }
            if (!symbolName) return;

            try {
                const info = await bridge.symbolInfo(symbolName);
                const doc = await vscode.workspace.openTextDocument({
                    content: info,
                    language: 'markdown',
                });
                vscode.window.showTextDocument(doc, { preview: true });
            } catch (e: any) {
                vscode.window.showErrorMessage(`Plex symbol info failed: ${e.message}`);
            }
        }),

        vscode.commands.registerCommand('plex.dashboard', async () => {
            if (dashboardPanel) {
                dashboardPanel.reveal(vscode.ViewColumn.One);
                return;
            }

            // Start plex serve if not running
            if (!plexServeProcess) {
                const plexPath = bridge.getPlexPathPublic();
                const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || process.cwd();
                plexServeProcess = spawn(plexPath, ['serve', '.', '--port', String(DASHBOARD_PORT)], { cwd, stdio: 'ignore' });
                plexServeProcess.on('exit', () => { plexServeProcess = null; });
                // Give the server a moment to start
                await new Promise(resolve => setTimeout(resolve, 500));
            }

            dashboardPanel = vscode.window.createWebviewPanel(
                'plex.dashboard',
                'Plex Dashboard',
                vscode.ViewColumn.One,
                {
                    enableScripts: true,
                    retainContextWhenHidden: true,
                }
            );

            dashboardPanel.webview.html = getDashboardHtml(DASHBOARD_PORT);

            dashboardPanel.onDidDispose(() => {
                dashboardPanel = null;
            });
        }),

        vscode.commands.registerCommand('plex.openFile', async (filePath: string, line: number) => {
            const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
            if (!workspaceRoot) return;

            const fullPath = vscode.Uri.file(`${workspaceRoot}/${filePath}`);
            const doc = await vscode.workspace.openTextDocument(fullPath);
            const editor = await vscode.window.showTextDocument(doc);
            const pos = new vscode.Position(Math.max(0, line - 1), 0);
            editor.selection = new vscode.Selection(pos, pos);
            editor.revealRange(new vscode.Range(pos, pos), vscode.TextEditorRevealType.InCenter);
        }),

        vscode.commands.registerCommand('plex.setupMcp', async () => {
            const result = await ensureMcpConfig(true);
            if (result) {
                vscode.window.showInformationMessage('Plex: MCP config created! AI agents can now use plex tools.');
            }
        })
    );

    // --- Auto-index on save ---
    const config = vscode.workspace.getConfiguration('plex');
    if (config.get<boolean>('autoIndex', true)) {
        context.subscriptions.push(
            vscode.workspace.onDidSaveTextDocument(async (doc) => {
                const ext = doc.fileName.split('.').pop() || '';
                const supported = ['py', 'ts', 'js', 'tsx', 'jsx', 'rs', 'go', 'java', 'c', 'h', 'cpp', 'cc', 'cxx', 'hpp'];
                if (supported.includes(ext)) {
                    try {
                        await bridge.index(false); // Quick re-index without embeddings on save
                        symbolTree.refresh();
                    } catch {
                        // Silent fail on auto-index
                    }
                }
            })
        );
    }

    // Initial load
    symbolTree.refresh();

    // Auto-setup MCP config for AI agents (silent, won't overwrite)
    ensureMcpConfig(false);

    // Auto-index on first activation if no index exists
    autoIndexIfNeeded();
}

async function autoIndexIfNeeded(): Promise<void> {
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!workspaceRoot) return;

    const indexPath = path.join(workspaceRoot, 'index.db');
    if (fs.existsSync(indexPath)) {
        // Index already exists — just update status bar
        return;
    }

    // No index found — prompt and auto-index
    const config = vscode.workspace.getConfiguration('plex');
    const useEmbeddings = config.get<boolean>('embeddings', true);

    const choice = await vscode.window.showInformationMessage(
        'Plex: No index found. Index this project now?',
        'Index with Embeddings',
        'Index (fast, no embeddings)',
        'Skip'
    );

    if (choice === 'Skip' || !choice) return;

    const withEmbed = choice === 'Index with Embeddings';

    await vscode.window.withProgress(
        {
            location: vscode.ProgressLocation.Notification,
            title: withEmbed
                ? 'Plex: Indexing project with embeddings (first run may download model)...'
                : 'Plex: Indexing project...',
            cancellable: false,
        },
        async () => {
            try {
                const result = await bridge.index(withEmbed);
                vscode.window.showInformationMessage(`Plex: ${result}`);
                symbolTree.refresh();
                updateStatusBar();
            } catch (e: any) {
                vscode.window.showErrorMessage(`Plex index failed: ${e.message}`);
            }
        }
    );
}

export function deactivate() {
    if (plexServeProcess) {
        plexServeProcess.kill();
        plexServeProcess = null;
    }
}

async function updateStatusBar(): Promise<void> {
    try {
        const stats = await bridge.stats();
        // Parse "Files: N" from output
        const filesMatch = stats.match(/Files:\s+(\d+)/);
        const symbolsMatch = stats.match(/Symbols:\s+(\d+)/);
        if (filesMatch && symbolsMatch) {
            const files = parseInt(filesMatch[1], 10);
            const symbols = parseInt(symbolsMatch[1], 10);
            statusBarItem.text = `$(zap) Plex: ${files} files, ${symbols} symbols`;
            statusBarItem.tooltip = stats;
        }
    } catch {
        statusBarItem.text = '$(zap) Plex: Not indexed';
        statusBarItem.tooltip = 'Click to view stats, or run "Plex: Index Project"';
    }
}

/**
 * Ensure MCP configuration files exist so AI agents (Cursor, VS Code Copilot, Claude Desktop)
 * can discover and use plex as an MCP tool server.
 */
async function ensureMcpConfig(force: boolean): Promise<boolean> {
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!workspaceRoot) return false;

    const plexPath = bridge.getPlexPathPublic();
    let created = false;

    // --- Cursor: .cursor/mcp.json ---
    const cursorDir = path.join(workspaceRoot, '.cursor');
    const cursorMcpPath = path.join(cursorDir, 'mcp.json');
    if (force || !fs.existsSync(cursorMcpPath)) {
        const cursorConfig = {
            mcpServers: {
                plex: {
                    command: plexPath,
                    args: ["mcp", "."],
                    cwd: workspaceRoot
                }
            }
        };

        // If file exists, merge rather than overwrite
        if (fs.existsSync(cursorMcpPath)) {
            try {
                const existing = JSON.parse(fs.readFileSync(cursorMcpPath, 'utf-8'));
                if (!existing.mcpServers?.plex) {
                    existing.mcpServers = existing.mcpServers || {};
                    existing.mcpServers.plex = cursorConfig.mcpServers.plex;
                    fs.writeFileSync(cursorMcpPath, JSON.stringify(existing, null, 2) + '\n');
                    created = true;
                }
            } catch {
                // If parse fails, don't touch it
            }
        } else {
            fs.mkdirSync(cursorDir, { recursive: true });
            fs.writeFileSync(cursorMcpPath, JSON.stringify(cursorConfig, null, 2) + '\n');
            created = true;
        }
    }

    // --- VS Code Copilot: .vscode/mcp.json ---
    const vscodeDir = path.join(workspaceRoot, '.vscode');
    const vscodeMcpPath = path.join(vscodeDir, 'mcp.json');
    if (force || !fs.existsSync(vscodeMcpPath)) {
        const vscodeConfig = {
            servers: {
                plex: {
                    type: "stdio",
                    command: plexPath,
                    args: ["mcp", "."]
                }
            }
        };

        if (fs.existsSync(vscodeMcpPath)) {
            try {
                const existing = JSON.parse(fs.readFileSync(vscodeMcpPath, 'utf-8'));
                if (!existing.servers?.plex) {
                    existing.servers = existing.servers || {};
                    existing.servers.plex = vscodeConfig.servers.plex;
                    fs.writeFileSync(vscodeMcpPath, JSON.stringify(existing, null, 2) + '\n');
                    created = true;
                }
            } catch {
                // If parse fails, don't touch it
            }
        } else {
            fs.mkdirSync(vscodeDir, { recursive: true });
            fs.writeFileSync(vscodeMcpPath, JSON.stringify(vscodeConfig, null, 2) + '\n');
            created = true;
        }
    }

    return created;
}

function getDashboardHtml(port: number): string {
    return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; connect-src http://localhost:${port}; script-src https://d3js.org 'unsafe-inline'; style-src 'unsafe-inline';">
<style>body{margin:0;padding:0;overflow:hidden;height:100vh;background:#0d1117;font-family:sans-serif;color:#e6edf3;display:flex;align-items:center;justify-content:center;}</style>
</head>
<body>
<div id="loading" style="text-align:center">
  <div style="font-size:24px;margin-bottom:8px">⚡</div>
  <div style="color:#8b949e">Loading Plex Dashboard...</div>
</div>
<script>
  // Fetch the full dashboard HTML from the running server and replace document
  fetch('http://localhost:${port}/')
    .then(r => r.text())
    .then(html => {
      // Inject the API base URL and rewrite the HTML
      const modified = html.replace("const API = '';", "const API = 'http://localhost:${port}';");
      document.open();
      document.write(modified);
      document.close();
    })
    .catch(err => {
      document.getElementById('loading').innerHTML = '<div style="color:#f85149">Failed to connect to Plex server</div><div style="color:#8b949e;margin-top:8px;font-size:13px">Run "Plex: Index Project" first, then try again.</div>';
    });
</script>
</body>
</html>`;
}
