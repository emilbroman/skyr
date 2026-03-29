<script lang="ts">
import { FileText, Folder, FolderPlus, FilePlus, Trash2, Pencil } from "lucide-svelte";
import type { FileTreeNode } from "./state.svelte.js";

type Props = {
    tree: FileTreeNode[];
    activeFile: string;
    onSelectFile: (path: string) => void;
    onCreateFile: (path: string) => void;
    onDeleteEntry: (path: string) => void;
    onRenameEntry: (oldPath: string, newPath: string) => void;
};

let { tree, activeFile, onSelectFile, onCreateFile, onDeleteEntry, onRenameEntry }: Props =
    $props();

let expandedFolders = $state<Set<string>>(new Set());
let creatingIn = $state<string | null>(null);
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

function startCreating(folderPath: string) {
    creatingIn = folderPath;
    newFileName = "";
    // Expand the folder
    if (folderPath) {
        expandedFolders = new Set([...expandedFolders, folderPath]);
    }
}

function confirmCreate() {
    if (!newFileName.trim()) {
        creatingIn = null;
        return;
    }
    let name = newFileName.trim();
    if (!name.endsWith(".scl")) name += ".scl";
    const path = creatingIn ? `${creatingIn}/${name}` : name;
    onCreateFile(path);
    creatingIn = null;
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
    if (e.key === "Escape") creatingIn = null;
}

function handleRenameKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") confirmRename();
    if (e.key === "Escape") renamingPath = null;
}
</script>

<div class="flex flex-col h-full text-sm">
    <div class="flex items-center justify-between px-3 py-2 border-b border-gray-200">
        <span class="text-xs font-semibold text-gray-500 uppercase tracking-wide">Files</span>
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
                onclick={() => startCreating("")}
            >
                <FolderPlus class="w-3.5 h-3.5" />
            </button>
        </div>
    </div>
    <div class="flex-1 overflow-y-auto py-1">
        {#each tree as node}
            {@render treeNode(node, 0)}
        {/each}
        {#if creatingIn === ""}
            <div class="flex items-center gap-1 px-3 py-0.5" style="padding-left: 12px;">
                <FileText class="w-3.5 h-3.5 text-gray-400 shrink-0" />
                <!-- svelte-ignore a11y_autofocus -->
                <input
                    class="flex-1 text-xs bg-white border border-blue-400 rounded px-1 py-0.5 outline-none"
                    bind:value={newFileName}
                    onkeydown={handleCreateKeydown}
                    onblur={confirmCreate}
                    placeholder="filename.scl"
                    autofocus
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
            <Folder class="w-3.5 h-3.5 text-orange-500 shrink-0" />
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
                    class="flex items-center gap-1 px-3 py-0.5"
                    style="padding-left: {12 + (depth + 1) * 16}px;"
                >
                    <FileText class="w-3.5 h-3.5 text-gray-400 shrink-0" />
                    <!-- svelte-ignore a11y_autofocus -->
                    <input
                        class="flex-1 text-xs bg-white border border-blue-400 rounded px-1 py-0.5 outline-none"
                        bind:value={newFileName}
                        onkeydown={handleCreateKeydown}
                        onblur={confirmCreate}
                        placeholder="filename.scl"
                        autofocus
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
                <!-- svelte-ignore a11y_autofocus -->
                <input
                    class="flex-1 text-xs bg-white border border-blue-400 rounded px-1 py-0.5 outline-none"
                    bind:value={renameValue}
                    onkeydown={handleRenameKeydown}
                    onblur={confirmRename}
                    autofocus
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
