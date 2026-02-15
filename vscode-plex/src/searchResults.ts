import * as vscode from 'vscode';
import { PlexBridge, SearchResult } from './bridge';

export class SearchResultsProvider implements vscode.TreeDataProvider<SearchResultItem> {
    private _onDidChangeTreeData = new vscode.EventEmitter<SearchResultItem | undefined | null>();
    readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

    private results: SearchResult[] = [];
    private query: string = '';

    constructor(private bridge: PlexBridge) {}

    setResults(results: SearchResult[], query: string): void {
        this.results = results;
        this.query = query;
        this._onDidChangeTreeData.fire(undefined);
    }

    getTreeItem(element: SearchResultItem): vscode.TreeItem {
        return element;
    }

    async getChildren(element?: SearchResultItem): Promise<SearchResultItem[]> {
        if (element) return [];

        if (this.results.length === 0) {
            if (this.query) {
                return [new SearchResultItem(`No results for '${this.query}'`, '', 0, 0)];
            }
            return [new SearchResultItem('Use Cmd+Shift+F5 to search', '', 0, 0)];
        }

        return this.results.map((r, i) => {
            const item = new SearchResultItem(
                `${i + 1}. ${r.qualifiedName}`,
                r.filePath,
                r.line,
                r.score
            );
            item.description = `${r.kind} — ${r.filePath}:${r.line}`;
            item.tooltip = [
                r.signature || r.qualifiedName,
                r.docComment ? `\n${r.docComment}` : '',
                `\nScore: ${r.score.toFixed(3)}`,
            ].join('');
            item.command = {
                command: 'plex.openFile',
                title: 'Open',
                arguments: [r.filePath, r.line],
            };
            return item;
        });
    }
}

class SearchResultItem extends vscode.TreeItem {
    constructor(label: string, filePath: string, line: number, score: number) {
        super(label, vscode.TreeItemCollapsibleState.None);
        if (score > 0) {
            this.iconPath = new vscode.ThemeIcon('search');
        }
    }
}
