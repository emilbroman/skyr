import {
    createStore,
    delMany as idbDelMany,
    entries as idbEntries,
    get as idbGet,
    set as idbSet,
    setMany as idbSetMany,
} from "idb-keyval";
import type { DiagnosticInfo } from "./client.js";

const defaultContent = `// Welcome to the SCL Playground!
let greeting = "Hello, world!"
`;

const META_ACTIVE_FILE = "activeFile";
const META_EMPTY_FOLDERS = "emptyFolders";
const CONTENT_SAVE_DEBOUNCE_MS = 200;

// Two IndexedDB databases — idb-keyval's createStore only creates the object store in
// onupgradeneeded the first time the DB is opened, so two stores in the same DB would
// require a manual version bump. Separate DBs sidestep that entirely.
//   - "skyr-playground-files": key = file path,  value = file content (string)
//   - "skyr-playground-meta":  key = "activeFile" | "emptyFolders"
const filesStore =
    typeof indexedDB === "undefined" ? null : createStore("skyr-playground-files", "files");
const metaStore =
    typeof indexedDB === "undefined" ? null : createStore("skyr-playground-meta", "meta");

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

    let loaded = false;

    // Per-path debounce timers for content writes — so rapid typing in file A
    // doesn't delay writes for file B.
    const contentSaveTimers = new Map<string, ReturnType<typeof setTimeout>>();

    function warn(msg: string, err: unknown) {
        console.warn(msg, err);
    }

    const readyPromise =
        filesStore === null || metaStore === null
            ? Promise.resolve()
            : (async () => {
                  try {
                      const [fileEntries, storedActiveFile, storedEmptyFolders] = await Promise.all(
                          [
                              idbEntries<string, string>(filesStore),
                              idbGet<string>(META_ACTIVE_FILE, metaStore),
                              idbGet<string[]>(META_EMPTY_FOLDERS, metaStore),
                          ],
                      );

                      if (fileEntries.length > 0) {
                          const loadedFiles: Record<string, string> = {};
                          for (const [path, content] of fileEntries) {
                              loadedFiles[path] = content;
                          }
                          files = loadedFiles;
                          emptyFolders = new Set(storedEmptyFolders ?? []);
                          activeFile =
                              storedActiveFile && storedActiveFile in loadedFiles
                                  ? storedActiveFile
                                  : (Object.keys(loadedFiles)[0] ?? "");
                      }
                  } catch (err) {
                      warn("Failed to load playground state from IndexedDB:", err);
                  } finally {
                      loaded = true;
                  }
              })();

    function persistContent(path: string, content: string) {
        if (!loaded || filesStore === null) return;
        const existing = contentSaveTimers.get(path);
        if (existing) clearTimeout(existing);
        const timer = setTimeout(() => {
            contentSaveTimers.delete(path);
            idbSet(path, content, filesStore).catch((err) => {
                warn(`Failed to persist file ${path}:`, err);
            });
        }, CONTENT_SAVE_DEBOUNCE_MS);
        contentSaveTimers.set(path, timer);
    }

    function flushContentTimer(path: string) {
        const existing = contentSaveTimers.get(path);
        if (existing) {
            clearTimeout(existing);
            contentSaveTimers.delete(path);
        }
    }

    function persistActiveFile() {
        if (!loaded || metaStore === null) return;
        idbSet(META_ACTIVE_FILE, activeFile, metaStore).catch((err) => {
            warn("Failed to persist active file:", err);
        });
    }

    function persistEmptyFolders() {
        if (!loaded || metaStore === null) return;
        idbSet(META_EMPTY_FOLDERS, [...emptyFolders], metaStore).catch((err) => {
            warn("Failed to persist empty folders:", err);
        });
    }

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
            persistActiveFile();
        }
    }

    function updateFileContent(path: string, content: string) {
        files = { ...files, [path]: content };
        persistContent(path, content);
    }

    function createFile(path: string) {
        if (path in files) return false;
        if (!path.endsWith(".scl") && !path.endsWith(".scle")) {
            path = `${path}.scl`;
        }
        files = { ...files, [path]: "" };
        activeFile = path;

        // Remove any ancestor empty-folder markers (they now have content)
        const removedFolders: string[] = [];
        const next = new Set(emptyFolders);
        const parts = path.split("/");
        for (let i = 1; i < parts.length; i++) {
            const ancestor = parts.slice(0, i).join("/");
            if (next.delete(ancestor)) removedFolders.push(ancestor);
        }
        emptyFolders = next;

        if (loaded && filesStore !== null) {
            idbSet(path, "", filesStore).catch((err) =>
                warn(`Failed to persist file ${path}:`, err),
            );
        }
        persistActiveFile();
        if (removedFolders.length > 0) persistEmptyFolders();
        return true;
    }

    function createFolder(path: string) {
        // Check if the folder already exists implicitly (has files inside it)
        const prefix = path.endsWith("/") ? path : `${path}/`;
        const alreadyExists = Object.keys(files).some((f) => f.startsWith(prefix));
        if (alreadyExists || emptyFolders.has(path)) return;
        emptyFolders = new Set([...emptyFolders, path]);
        persistEmptyFolders();
    }

    function deleteEntry(path: string) {
        const newFiles: Record<string, string> = {};
        const isFolder = !path.endsWith(".scl") && !path.endsWith(".scle");
        const prefix = isFolder ? (path.endsWith("/") ? path : `${path}/`) : null;
        const removedPaths: string[] = [];

        for (const [name, content] of Object.entries(files)) {
            if (name === path) {
                removedPaths.push(name);
                continue;
            }
            if (prefix && name.startsWith(prefix)) {
                removedPaths.push(name);
                continue;
            }
            newFiles[name] = content;
        }

        files = newFiles;

        let emptyFoldersChanged = false;
        if (isFolder) {
            const next = new Set(emptyFolders);
            if (next.delete(path)) emptyFoldersChanged = true;
            for (const f of emptyFolders) {
                if (prefix && f.startsWith(prefix)) {
                    next.delete(f);
                    emptyFoldersChanged = true;
                }
            }
            emptyFolders = next;
        }

        // If active file was deleted, switch to first available
        const activeFileChanged = !(activeFile in files);
        if (activeFileChanged) {
            const remaining = Object.keys(files);
            activeFile = remaining[0] ?? "";
        }

        // Cancel any pending writes for deleted paths, then remove them from storage
        for (const p of removedPaths) flushContentTimer(p);
        if (loaded && filesStore !== null && removedPaths.length > 0) {
            idbDelMany(removedPaths, filesStore).catch((err) =>
                warn("Failed to delete files:", err),
            );
        }
        if (emptyFoldersChanged) persistEmptyFolders();
        if (activeFileChanged) persistActiveFile();
    }

    function renameEntry(oldPath: string, newPath: string) {
        if (oldPath === newPath) return;

        const newFiles: Record<string, string> = {};
        const isFolder = !oldPath.endsWith(".scl") && !oldPath.endsWith(".scle");
        const oldPrefix = isFolder ? (oldPath.endsWith("/") ? oldPath : `${oldPath}/`) : null;
        const newPrefix = isFolder ? (newPath.endsWith("/") ? newPath : `${newPath}/`) : null;

        const removedPaths: string[] = [];
        const addedEntries: [string, string][] = [];

        for (const [name, content] of Object.entries(files)) {
            if (name === oldPath) {
                newFiles[newPath] = content;
                removedPaths.push(name);
                addedEntries.push([newPath, content]);
            } else if (oldPrefix && newPrefix && name.startsWith(oldPrefix)) {
                const renamed = `${newPrefix}${name.slice(oldPrefix.length)}`;
                newFiles[renamed] = content;
                removedPaths.push(name);
                addedEntries.push([renamed, content]);
            } else {
                newFiles[name] = content;
            }
        }

        files = newFiles;

        // Rename empty folder entries
        let emptyFoldersChanged = false;
        if (isFolder) {
            const next = new Set(emptyFolders);
            for (const f of emptyFolders) {
                if (f === oldPath) {
                    next.delete(f);
                    next.add(newPath);
                    emptyFoldersChanged = true;
                } else if (oldPrefix && newPrefix && f.startsWith(oldPrefix)) {
                    next.delete(f);
                    next.add(`${newPrefix}${f.slice(oldPrefix.length)}`);
                    emptyFoldersChanged = true;
                }
            }
            emptyFolders = next;
        }

        let activeFileChanged = false;
        if (activeFile === oldPath) {
            activeFile = newPath;
            activeFileChanged = true;
        } else if (oldPrefix && newPrefix && activeFile.startsWith(oldPrefix)) {
            activeFile = `${newPrefix}${activeFile.slice(oldPrefix.length)}`;
            activeFileChanged = true;
        }

        // Flush pending writes for old paths so they don't resurrect the stale key,
        // then remove old keys and write the new ones.
        for (const p of removedPaths) flushContentTimer(p);
        if (loaded && filesStore !== null && removedPaths.length > 0) {
            idbDelMany(removedPaths, filesStore)
                .then(() => idbSetMany(addedEntries, filesStore))
                .catch((err) => warn("Failed to rename files:", err));
        }
        if (emptyFoldersChanged) persistEmptyFolders();
        if (activeFileChanged) persistActiveFile();
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
        ready: () => readyPromise,
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
