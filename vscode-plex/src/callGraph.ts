import * as vscode from 'vscode';
import { PlexBridge, CallGraphNode } from './bridge';

export class CallGraphProvider implements vscode.TreeDataProvider<CallGraphItem> {
    private _onDidChangeTreeData = new vscode.EventEmitter<CallGraphItem | undefined | null>();
    readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

    private roots: CallGraphNode[] = [];
    private symbolName: string = '';

    constructor(private bridge: PlexBridge) {}

    setGraph(nodes: CallGraphNode[], symbolName: string): void {
        this.roots = nodes;
        this.symbolName = symbolName;
        this._onDidChangeTreeData.fire(undefined);
    }

    getTreeItem(element: CallGraphItem): vscode.TreeItem {
        return element;
    }

    async getChildren(element?: CallGraphItem): Promise<CallGraphItem[]> {
        if (!element) {
            if (this.roots.length === 0) {
                return [new CallGraphItem('Select a function to see its call graph', [], '', 0)];
            }
            return this.roots.map((node) => this.nodeToItem(node));
        }

        return element.childNodes.map((node) => this.nodeToItem(node));
    }

    private nodeToItem(node: CallGraphNode): CallGraphItem {
        const hasChildren = node.children.length > 0;
        const item = new CallGraphItem(
            `${kindEmoji(node.kind)} ${node.name}`,
            node.children,
            node.filePath,
            node.line
        );
        item.collapsibleState = hasChildren
            ? vscode.TreeItemCollapsibleState.Expanded
            : vscode.TreeItemCollapsibleState.None;
        item.description = node.filePath ? `${node.filePath}:${node.line}` : '';
        item.tooltip = `${node.kind} — ${node.filePath}:${node.line}`;

        if (node.filePath) {
            item.command = {
                command: 'plex.openFile',
                title: 'Open',
                arguments: [node.filePath, node.line],
            };
        }

        return item;
    }
}

function kindEmoji(kind: string): string {
    const map: Record<string, string> = {
        function: '$(symbol-method)',
        method: '$(symbol-method)',
        class: '$(symbol-class)',
        root: '$(home)',
    };
    return map[kind.toLowerCase()] || '$(circle-outline)';
}

class CallGraphItem extends vscode.TreeItem {
    constructor(
        label: string,
        public readonly childNodes: CallGraphNode[],
        public readonly filePath: string,
        public readonly line: number
    ) {
        super(label, vscode.TreeItemCollapsibleState.None);
    }
}
