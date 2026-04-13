<script lang="ts">
import {
    FileText,
    Folder,
    FolderOpen,
    FolderPlus,
    FilePlus,
    Trash2,
    Pencil,
    X,
} from "lucide-svelte";
import type { FileTreeNode } from "./state.svelte.js";

function autofocus(node: HTMLElement) {
    node.focus();
}

type Props = {
    tree: FileTreeNode[];
    activeFile: string;
    onSelectFile: (path: string) => void;
    onCreateFile: (path: string) => void;
    onCreateFolder: (path: string) => void;
    onDeleteEntry: (path: string) => void;
    onRenameEntry: (oldPath: string, newPath: string) => void;
    onClose?: () => void;
};

let {
    tree,
    activeFile,
    onSelectFile,
    onCreateFile,
    onCreateFolder,
    onDeleteEntry,
    onRenameEntry,
    onClose,
}: Props = $props();

let expandedFolders = $state<Set<string>>(new Set());
let creatingIn = $state<string | null>(null);
let creatingFolder = $state(false);
let newFileName = $state("");
let renamingPath = $state<string | null>(null);
let renameValue = $state("");

function toggleFolder(path: string) {
    const next = new Set(expandedFolders);
    if (next.has(path)) {
        next.delete(path);
    } else {
        next.add(path);
    }
    expandedFolders = next;
}

function startCreating(folderPath: string, isFolder = false) {
    creatingIn = folderPath;
    creatingFolder = isFolder;
    newFileName = "";
    // Expand the folder
    if (folderPath) {
        expandedFolders = new Set([...expandedFolders, folderPath]);
    }
}

function confirmCreate() {
    if (!newFileName.trim()) {
        creatingIn = null;
        creatingFolder = false;
        return;
    }
    const name = newFileName.trim();
    const path = creatingIn ? `${creatingIn}/${name}` : name;
    if (creatingFolder) {
        onCreateFolder(path);
        expandedFolders = new Set([...expandedFolders, path]);
    } else {
        let fileName = name;
        if (!fileName.endsWith(".scl") && !fileName.endsWith(".scle")) fileName += ".scl";
        const filePath = creatingIn ? `${creatingIn}/${fileName}` : fileName;
        onCreateFile(filePath);
    }
    creatingIn = null;
    creatingFolder = false;
    newFileName = "";
}

function startRename(path: string, currentName: string) {
    renamingPath = path;
    renameValue = currentName;
}

function confirmRename() {
    if (!renamingPath || !renameValue.trim()) {
        renamingPath = null;
        return;
    }
    const parts = renamingPath.split("/");
    parts[parts.length - 1] = renameValue.trim();
    const newPath = parts.join("/");
    if (newPath !== renamingPath) {
        onRenameEntry(renamingPath, newPath);
    }
    renamingPath = null;
}

function handleCreateKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") confirmCreate();
    if (e.key === "Escape") {
        creatingIn = null;
        creatingFolder = false;
    }
}

function handleRenameKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") confirmRename();
    if (e.key === "Escape") renamingPath = null;
}
</script>

<div class="flex flex-col h-full w-full text-sm">
    <div class="flex items-center justify-between px-3 h-10 shrink-0 border-b border-gray-200">
        <span class="text-xs font-medium text-gray-700">Files</span>
        <div class="flex gap-1">
            <button
                class="p-1 text-gray-400 hover:text-gray-600 rounded"
                title="New file"
                onclick={() => startCreating("")}
            >
                <FilePlus class="w-3.5 h-3.5" />
            </button>
            <button
                class="p-1 text-gray-400 hover:text-gray-600 rounded"
                title="New folder"
                onclick={() => startCreating("", true)}
            >
                <FolderPlus class="w-3.5 h-3.5" />
            </button>
            {#if onClose}
                <button
                    class="p-1 text-gray-400 hover:text-gray-600 rounded"
                    onclick={onClose}
                    title="Close"
                >
                    <X class="w-3.5 h-3.5" />
                </button>
            {/if}
        </div>
    </div>
    <div class="flex-1 overflow-y-auto py-1">
        {#each tree as node}
            {@render treeNode(node, 0)}
        {/each}
        {#if creatingIn === ""}
            <div class="flex items-center gap-1.5 px-3 py-0.5" style="padding-left: 12px;">
                {#if creatingFolder}
                    <Folder class="w-3.5 h-3.5 text-orange-500 shrink-0" />
                {:else}
                    <FileText class="w-3.5 h-3.5 text-gray-400 shrink-0" />
                {/if}
                <input
                    use:autofocus
                    class="flex-1 bg-transparent outline-none placeholder:text-gray-400"
                    bind:value={newFileName}
                    onkeydown={handleCreateKeydown}
                    onblur={confirmCreate}
                    placeholder={creatingFolder ? "folder name" : "filename.scl"}
                />
            </div>
        {/if}
    </div>
</div>

{#snippet treeNode(node: FileTreeNode, depth: number)}
    {#if node.type === "folder"}
        {@const isExpanded = expandedFolders.has(node.path)}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="w-full flex items-center gap-1.5 px-3 py-0.5 hover:bg-gray-100 text-left group cursor-pointer"
            style="padding-left: {12 + depth * 16}px;"
            role="button"
            tabindex="0"
            onclick={() => toggleFolder(node.path)}
            onkeydown={(e) => { if (e.key === "Enter") toggleFolder(node.path); }}
        >
            {#if isExpanded}
                <FolderOpen class="w-3.5 h-3.5 text-orange-500 shrink-0" />
            {:else}
                <Folder class="w-3.5 h-3.5 text-orange-500 shrink-0" />
            {/if}
            <span class="flex-1 truncate text-gray-700">{node.name}</span>
            <div class="hidden group-hover:flex gap-0.5">
                <button
                    class="p-0.5 text-gray-400 hover:text-gray-600"
                    title="New file in folder"
                    onclick={(e) => { e.stopPropagation(); startCreating(node.path); }}
                >
                    <FilePlus class="w-3 h-3" />
                </button>
                <button
                    class="p-0.5 text-gray-400 hover:text-red-500"
                    title="Delete folder"
                    onclick={(e) => { e.stopPropagation(); onDeleteEntry(node.path); }}
                >
                    <Trash2 class="w-3 h-3" />
                </button>
            </div>
        </div>
        {#if isExpanded && node.children}
            {#each node.children as child}
                {@render treeNode(child, depth + 1)}
            {/each}
            {#if creatingIn === node.path}
                <div
                    class="flex items-center gap-1.5 px-3 py-0.5"
                    style="padding-left: {12 + (depth + 1) * 16}px;"
                >
                    {#if creatingFolder}
                        <Folder class="w-3.5 h-3.5 text-orange-500 shrink-0" />
                    {:else}
                        <FileText class="w-3.5 h-3.5 text-gray-400 shrink-0" />
                    {/if}
                    <input
                        use:autofocus
                        class="flex-1 bg-transparent outline-none placeholder:text-gray-400"
                        bind:value={newFileName}
                        onkeydown={handleCreateKeydown}
                        onblur={confirmCreate}
                        placeholder={creatingFolder ? "folder name" : "filename.scl"}
                    />
                </div>
            {/if}
        {/if}
    {:else}
        {#if renamingPath === node.path}
            <div
                class="flex items-center gap-1.5 px-3 py-0.5"
                style="padding-left: {12 + depth * 16}px;"
            >
                <FileText class="w-3.5 h-3.5 text-gray-400 shrink-0" />
                <input
                    use:autofocus
                    class="flex-1 bg-transparent outline-none placeholder:text-gray-400"
                    bind:value={renameValue}
                    onkeydown={handleRenameKeydown}
                    onblur={confirmRename}
                />
            </div>
        {:else}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="w-full flex items-center gap-1.5 px-3 py-0.5 hover:bg-gray-100 text-left group cursor-pointer {activeFile ===
                node.path
                    ? 'bg-blue-50 text-blue-700'
                    : 'text-gray-600'}"
                style="padding-left: {12 + depth * 16}px;"
                role="button"
                tabindex="0"
                onclick={() => onSelectFile(node.path)}
                onkeydown={(e) => { if (e.key === "Enter") onSelectFile(node.path); }}
            >
                <FileText class="w-3.5 h-3.5 shrink-0 {activeFile === node.path
                    ? 'text-blue-500'
                    : 'text-gray-400'}" />
                <span class="flex-1 truncate">{node.name}</span>
                <div class="hidden group-hover:flex gap-0.5">
                    <button
                        class="p-0.5 text-gray-400 hover:text-gray-600"
                        title="Rename"
                        onclick={(e) => { e.stopPropagation(); startRename(node.path, node.name); }}
                    >
                        <Pencil class="w-3 h-3" />
                    </button>
                    <button
                        class="p-0.5 text-gray-400 hover:text-red-500"
                        title="Delete"
                        onclick={(e) => { e.stopPropagation(); onDeleteEntry(node.path); }}
                    >
                        <Trash2 class="w-3 h-3" />
                    </button>
                </div>
            </div>
        {/if}
    {/if}
{/snippet}
