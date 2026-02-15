import * as vscode from 'vscode';
import { execFile } from 'child_process';
import { promisify } from 'util';

const execFileAsync = promisify(execFile);

export interface SearchResult {
    name: string;
    qualifiedName: string;
    kind: string;
    filePath: string;
    line: number;
    score: number;
    signature?: string;
    docComment?: string;
}

export interface CallGraphNode {
    name: string;
    filePath: string;
    line: number;
    kind: string;
    children: CallGraphNode[];
}

/**
 * Bridge between VS Code extension and the plex CLI binary.
 * All communication happens via subprocess calls — no sockets, no ports.
 */
export class PlexBridge {
    private getPlexPath(): string {
        const config = vscode.workspace.getConfiguration('plex');
        const custom = config.get<string>('binaryPath', '');
        return custom || 'plex';
    }

    public getPlexPathPublic(): string {
        return this.getPlexPath();
    }

    private getCwd(): string {
        return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || process.cwd();
    }

    private async run(args: string[], timeoutMs: number = 120_000): Promise<string> {
        const plexPath = this.getPlexPath();
        const cwd = this.getCwd();

        try {
            const { stdout, stderr } = await execFileAsync(plexPath, args, {
                cwd,
                timeout: timeoutMs,
                maxBuffer: 10 * 1024 * 1024, // 10MB
                env: { ...process.env, NO_COLOR: '1' },
            });
            return stdout.trim();
        } catch (err: any) {
            // If plex wrote to stderr, include that
            const msg = err.stderr || err.message || 'Unknown error';
            throw new Error(msg);
        }
    }

    async index(withEmbeddings: boolean): Promise<string> {
        const args = ['index', '.'];
        if (!withEmbeddings) {
            args.push('--no-embed');
        }
        // Embeddings can take minutes on large projects — 30 min timeout
        const timeout = withEmbeddings ? 30 * 60 * 1000 : 5 * 60 * 1000;
        const output = await this.run(args, timeout);
        // Extract the summary line
        const lines = output.split('\n');
        return lines[lines.length - 1] || output;
    }

    async search(query: string, limit: number = 15): Promise<SearchResult[]> {
        const output = await this.run(['search', query, '--limit', limit.toString(), '--json']);
        try {
            return JSON.parse(output);
        } catch {
            // Fallback: parse text output
            return this.parseSearchText(output);
        }
    }

    async callGraph(symbolName: string, depth: number = 3): Promise<CallGraphNode[]> {
        const output = await this.run(['calls', symbolName, '--depth', depth.toString(), '--json']);
        try {
            const data = JSON.parse(output);
            // CLI outputs { nodes: [...], edges: [...] } — convert to tree
            if (data.nodes && data.edges) {
                return this.buildCallTree(data, symbolName);
            }
            return data;
        } catch {
            return this.parseCallGraphText(output, symbolName);
        }
    }

    async stats(): Promise<string> {
        return this.run(['stats']);
    }

    async symbolInfo(name: string): Promise<string> {
        return this.run(['search', name, '--limit', '5']);
    }

    async getFileSymbols(filePath: string): Promise<SearchResult[]> {
        const output = await this.run(['symbols', filePath, '--json']);
        try {
            return JSON.parse(output);
        } catch {
            return [];
        }
    }

    // --- Text parsing fallbacks (when --json isn't supported yet) ---

    private parseSearchText(text: string): SearchResult[] {
        const results: SearchResult[] = [];
        const blocks = text.split(/\n(?=\d+\.\s)/);

        for (const block of blocks) {
            const nameMatch = block.match(/\d+\.\s+(.+?)\s+\((\w+)\)/);
            const fileMatch = block.match(/(?:File:\s*)?(\S+?):(\d+)/);
            const scoreMatch = block.match(/Score:\s*([\d.]+)/);
            const sigMatch = block.match(/Signature:\s*(.+)/);

            if (nameMatch && fileMatch) {
                results.push({
                    name: nameMatch[1].split('::').pop() || nameMatch[1],
                    qualifiedName: nameMatch[1],
                    kind: nameMatch[2],
                    filePath: fileMatch[1],
                    line: parseInt(fileMatch[2], 10),
                    score: scoreMatch ? parseFloat(scoreMatch[1]) : 0,
                    signature: sigMatch?.[1],
                });
            }
        }
        return results;
    }

    private parseCallGraphText(text: string, rootName: string): CallGraphNode[] {
        const root: CallGraphNode = {
            name: rootName,
            filePath: '',
            line: 0,
            kind: 'function',
            children: [],
        };

        const lines = text.split('\n');
        for (const line of lines) {
            const match = line.match(/→\s+(\S+?)::(\S+)\s+\((\w+)\)\s+at\s+(\S+?):(\d+)/);
            if (match) {
                root.children.push({
                    name: match[2],
                    filePath: match[4],
                    line: parseInt(match[5], 10),
                    kind: match[3],
                    children: [],
                });
            }
        }

        return [root];
    }

    private buildCallTree(data: { nodes: any[]; edges: any[] }, rootName: string): CallGraphNode[] {
        const nodeMap = new Map<string, CallGraphNode>();
        for (const n of data.nodes) {
            const qname = n.qualifiedName || n.name;
            nodeMap.set(qname, {
                name: n.name || qname.split('::').pop() || qname,
                filePath: n.filePath || '',
                line: n.line || 0,
                kind: n.kind || 'function',
                children: [],
            });
        }

        for (const e of data.edges) {
            const parent = nodeMap.get(e.source);
            const child = nodeMap.get(e.target);
            if (parent && child) {
                parent.children.push(child);
            }
        }

        // Find the root node (matches rootName)
        for (const [qname, node] of nodeMap) {
            if (qname.includes(rootName) || node.name === rootName) {
                return [node];
            }
        }

        // Fallback: return all root-level nodes
        const childNames = new Set(data.edges.map((e: any) => e.target));
        const roots = [...nodeMap.entries()]
            .filter(([qname]) => !childNames.has(qname))
            .map(([, node]) => node);
        return roots.length > 0 ? roots : [...nodeMap.values()];
    }
}
