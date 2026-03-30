import type { DiagnosticInfo } from "./client.js";

const defaultContent = `// Welcome to the SCL Playground!
let greeting = "Hello, world!"
`;

export interface ReplEntry {
    input: string;
    output?: string;
    effects?: string[];
    error?: string;
}

export interface FileTreeNode {
    name: string;
    path: string;
    type: "file" | "folder";
    children?: FileTreeNode[];
}

function createPlaygroundState() {
    let files = $state<Record<string, string>>({ "Main.scl": defaultContent });
    let emptyFolders = $state<Set<string>>(new Set());
    let activeFile = $state<string>("Main.scl");
    let diagnostics = $state<DiagnosticInfo[]>([]);
    let replHistory = $state<ReplEntry[]>([]);

    function getFiles(): Record<string, string> {
        return $state.snapshot(files);
    }

    function getActiveFile(): string {
        return activeFile;
    }

    function getActiveFileContent(): string {
        return files[activeFile] ?? "";
    }

    function setActiveFile(path: string) {
        if (path in files) {
            activeFile = path;
        }
    }

    function updateFileContent(path: string, content: string) {
        files = { ...files, [path]: content };
    }

    function createFile(path: string) {
        if (path in files) return false;
        if (!path.endsWith(".scl")) {
            path = `${path}.scl`;
        }
        files = { ...files, [path]: "" };
        activeFile = path;
        // Remove any ancestor empty-folder markers (they now have content)
        const next = new Set(emptyFolders);
        const parts = path.split("/");
        for (let i = 1; i < parts.length; i++) {
            next.delete(parts.slice(0, i).join("/"));
        }
        emptyFolders = next;
        return true;
    }

    function createFolder(path: string) {
        // Check if the folder already exists implicitly (has files inside it)
        const prefix = path.endsWith("/") ? path : `${path}/`;
        const alreadyExists = Object.keys(files).some((f) => f.startsWith(prefix));
        if (alreadyExists || emptyFolders.has(path)) return;
        emptyFolders = new Set([...emptyFolders, path]);
    }

    function deleteEntry(path: string) {
        const newFiles: Record<string, string> = {};
        const isFolder = !path.endsWith(".scl");
        const prefix = isFolder ? (path.endsWith("/") ? path : `${path}/`) : null;

        for (const [name, content] of Object.entries(files)) {
            if (name === path) continue;
            if (prefix && name.startsWith(prefix)) continue;
            newFiles[name] = content;
        }

        files = newFiles;

        // Remove the folder itself and any nested empty folders
        if (isFolder) {
            const next = new Set(emptyFolders);
            next.delete(path);
            for (const f of emptyFolders) {
                if (prefix && f.startsWith(prefix)) next.delete(f);
            }
            emptyFolders = next;
        }

        // If active file was deleted, switch to first available
        if (!(activeFile in files)) {
            const remaining = Object.keys(files);
            activeFile = remaining[0] ?? "";
        }
    }

    function renameEntry(oldPath: string, newPath: string) {
        if (oldPath === newPath) return;

        const newFiles: Record<string, string> = {};
        const isFolder = !oldPath.endsWith(".scl");
        const oldPrefix = isFolder ? (oldPath.endsWith("/") ? oldPath : `${oldPath}/`) : null;
        const newPrefix = isFolder ? (newPath.endsWith("/") ? newPath : `${newPath}/`) : null;

        for (const [name, content] of Object.entries(files)) {
            if (name === oldPath) {
                newFiles[newPath] = content;
            } else if (oldPrefix && newPrefix && name.startsWith(oldPrefix)) {
                newFiles[`${newPrefix}${name.slice(oldPrefix.length)}`] = content;
            } else {
                newFiles[name] = content;
            }
        }

        files = newFiles;

        // Rename empty folder entries
        if (isFolder) {
            const next = new Set(emptyFolders);
            for (const f of emptyFolders) {
                if (f === oldPath) {
                    next.delete(f);
                    next.add(newPath);
                } else if (oldPrefix && newPrefix && f.startsWith(oldPrefix)) {
                    next.delete(f);
                    next.add(`${newPrefix}${f.slice(oldPrefix.length)}`);
                }
            }
            emptyFolders = next;
        }

        if (activeFile === oldPath) {
            activeFile = newPath;
        } else if (oldPrefix && newPrefix && activeFile.startsWith(oldPrefix)) {
            activeFile = `${newPrefix}${activeFile.slice(oldPrefix.length)}`;
        }
    }

    function getDiagnostics(): DiagnosticInfo[] {
        return diagnostics;
    }

    function setDiagnostics(diags: DiagnosticInfo[]) {
        diagnostics = diags;
    }

    function getReplHistory(): ReplEntry[] {
        return replHistory;
    }

    function addReplEntry(entry: ReplEntry) {
        replHistory = [...replHistory, entry];
    }

    function clearReplHistory() {
        replHistory = [];
    }

    function buildFileTree(): FileTreeNode[] {
        const root: FileTreeNode[] = [];
        const folderMap = new Map<string, FileTreeNode>();

        function ensureFolder(path: string, children: FileTreeNode[]): FileTreeNode {
            let folder = folderMap.get(path);
            if (!folder) {
                const name = path.includes("/") ? path.slice(path.lastIndexOf("/") + 1) : path;
                folder = { name, path, type: "folder", children: [] };
                folderMap.set(path, folder);
                children.push(folder);
            }
            return folder;
        }

        // Create nodes for explicit empty folders first
        for (const folderPath of [...emptyFolders].sort()) {
            const parts = folderPath.split("/");
            let currentChildren = root;
            for (let i = 0; i < parts.length; i++) {
                const currentPath = parts.slice(0, i + 1).join("/");
                const folder = ensureFolder(currentPath, currentChildren);
                currentChildren = folder.children!;
            }
        }

        const sortedPaths = Object.keys(files).sort();

        for (const filePath of sortedPaths) {
            const parts = filePath.split("/");
            const fileName = parts[parts.length - 1];

            if (parts.length === 1) {
                // Top-level file
                root.push({ name: fileName, path: filePath, type: "file" });
            } else {
                // Nested file — ensure parent folders exist
                let currentChildren = root;

                for (let i = 0; i < parts.length - 1; i++) {
                    const currentPath = parts.slice(0, i + 1).join("/");
                    const folder = ensureFolder(currentPath, currentChildren);
                    currentChildren = folder.children!;
                }

                currentChildren.push({ name: fileName, path: filePath, type: "file" });
            }
        }

        // Sort: folders first, then files, alphabetically within each group
        const sortNodes = (nodes: FileTreeNode[]) => {
            nodes.sort((a, b) => {
                if (a.type !== b.type) return a.type === "folder" ? -1 : 1;
                return a.name.localeCompare(b.name);
            });
            for (const node of nodes) {
                if (node.children) sortNodes(node.children);
            }
        };
        sortNodes(root);

        return root;
    }

    return {
        get files() {
            return getFiles();
        },
        get activeFile() {
            return getActiveFile();
        },
        get activeFileContent() {
            return getActiveFileContent();
        },
        get diagnostics() {
            return getDiagnostics();
        },
        get replHistory() {
            return getReplHistory();
        },
        get fileTree() {
            return buildFileTree();
        },
        setActiveFile,
        updateFileContent,
        createFile,
        createFolder,
        deleteEntry,
        renameEntry,
        setDiagnostics,
        addReplEntry,
        clearReplHistory,
    };
}

export const playgroundState = createPlaygroundState();
