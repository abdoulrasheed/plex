import * as vscode from 'vscode';
import { PlexBridge, SearchResult } from './bridge';

interface SymbolGroup {
    label: string;
    filePath: string;
    symbols: SearchResult[];
}

export class SymbolTreeProvider implements vscode.TreeDataProvider<SymbolTreeItem> {
    private _onDidChangeTreeData = new vscode.EventEmitter<SymbolTreeItem | undefined | null>();
    readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

    private groups: SymbolGroup[] = [];

    constructor(private bridge: PlexBridge) {}

    refresh(): void {
        this._onDidChangeTreeData.fire(undefined);
    }

    getTreeItem(element: SymbolTreeItem): vscode.TreeItem {
        return element;
    }

    async getChildren(element?: SymbolTreeItem): Promise<SymbolTreeItem[]> {
        if (!element) {
            // Top-level: try to load symbols for the current file
            const editor = vscode.window.activeTextEditor;
            if (!editor) {
                return [new SymbolTreeItem('Open a file to see symbols', vscode.TreeItemCollapsibleState.None)];
            }

            const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
            if (!workspaceRoot) return [];

            const relPath = editor.document.uri.fsPath.replace(workspaceRoot + '/', '');

            try {
                const symbols = await this.bridge.getFileSymbols(relPath);
                if (symbols.length === 0) {
                    return [new SymbolTreeItem('No symbols indexed for this file', vscode.TreeItemCollapsibleState.None)];
                }

                return symbols.map((s) => {
                    const item = new SymbolTreeItem(
                        `${kindIcon(s.kind)} ${s.name}`,
                        vscode.TreeItemCollapsibleState.None
                    );
                    item.description = `${s.kind} — line ${s.line}`;
                    item.tooltip = s.signature || s.qualifiedName;
                    item.command = {
                        command: 'plex.openFile',
                        title: 'Go to symbol',
                        arguments: [s.filePath, s.line],
                    };
                    return item;
                });
            } catch {
                return [new SymbolTreeItem('Index not found. Run "Plex: Index Project"', vscode.TreeItemCollapsibleState.None)];
            }
        }

        return [];
    }
}

function kindIcon(kind: string): string {
    const icons: Record<string, string> = {
        function: '$(symbol-method)',
        method: '$(symbol-method)',
        class: '$(symbol-class)',
        struct: '$(symbol-structure)',
        trait: '$(symbol-interface)',
        interface: '$(symbol-interface)',
        enum: '$(symbol-enum)',
        import: '$(package)',
        variable: '$(symbol-variable)',
        constant: '$(symbol-constant)',
    };
    return icons[kind.toLowerCase()] || '$(symbol-misc)';
}

class SymbolTreeItem extends vscode.TreeItem {
    constructor(
        public readonly label: string,
        public readonly collapsibleState: vscode.TreeItemCollapsibleState
    ) {
        super(label, collapsibleState);
    }
}
