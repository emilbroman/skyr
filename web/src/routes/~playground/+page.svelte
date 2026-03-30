<script lang="ts">
import { onMount, onDestroy } from "svelte";
import * as monaco from "monaco-editor";
import { SclWorker } from "$lib/playground/client.js";
import {
    registerSclLanguage,
    registerProviders,
    setupDiagnostics,
    createEditor,
    getOrCreateModel,
    getModel,
    disposeModel,
    disposeAllModels,
    renameModel,
} from "$lib/playground/monaco.js";
import { playgroundState } from "$lib/playground/state.svelte.js";
import FileTree from "$lib/playground/FileTree.svelte";
import DiagnosticsPanel from "$lib/playground/DiagnosticsPanel.svelte";
import Repl from "$lib/playground/Repl.svelte";
import {
    PanelLeft,
    PanelBottom,
    AlertCircle,
    AlertTriangle,
    X,
    Trash2,
    RotateCcw,
} from "lucide-svelte";

let editorContainer: HTMLDivElement;
let editor: monaco.editor.IStandaloneCodeEditor | undefined;
let editorResizeObserver: ResizeObserver | undefined;
let worker: SclWorker | undefined;
let providersDisposable: { dispose(): void } | undefined;
let diagnosticsDisposable: { dispose(): void } | undefined;

// Panel visibility
let showFileTree = $state(true);
let showBottomPanel = $state(true);
let activeBottomTab = $state<"diagnostics" | "repl">("diagnostics");

// Panel sizes (pixels)
let sidebarWidth = $state(240);
let bottomPanelHeight = $state(240);

// Resize dragging state
let resizing = $state<"sidebar" | "bottom" | null>(null);

function startResize(panel: "sidebar" | "bottom", e: MouseEvent | TouchEvent) {
    e.preventDefault();
    resizing = panel;
    const isTouch = e instanceof TouchEvent;
    const startX = isTouch ? e.touches[0].clientX : e.clientX;
    const startY = isTouch ? e.touches[0].clientY : e.clientY;
    const startSidebarWidth = sidebarWidth;
    const startBottomHeight = bottomPanelHeight;

    function onPointerMove(e: MouseEvent | TouchEvent) {
        const clientX = e instanceof TouchEvent ? e.touches[0].clientX : e.clientX;
        const clientY = e instanceof TouchEvent ? e.touches[0].clientY : e.clientY;
        if (panel === "sidebar") {
            sidebarWidth = Math.max(120, Math.min(600, startSidebarWidth + (clientX - startX)));
        } else {
            bottomPanelHeight = Math.max(80, Math.min(600, startBottomHeight - (clientY - startY)));
        }
    }

    function onPointerUp() {
        resizing = null;
        document.removeEventListener("mousemove", onPointerMove);
        document.removeEventListener("mouseup", onPointerUp);
        document.removeEventListener("touchmove", onPointerMove);
        document.removeEventListener("touchend", onPointerUp);
    }

    if (isTouch) {
        document.addEventListener("touchmove", onPointerMove, { passive: false });
        document.addEventListener("touchend", onPointerUp);
    } else {
        document.addEventListener("mousemove", onPointerMove);
        document.addEventListener("mouseup", onPointerUp);
    }
}

// Mobile drawer state
let mobileFileTreeOpen = $state(false);

// Marker counts for status bar
let errorCount = $state(0);
let warningCount = $state(0);

function syncModelFromState() {
    const file = playgroundState.activeFile;
    const content = playgroundState.activeFileContent;
    const model = getOrCreateModel(file, content);
    if (editor && editor.getModel() !== model) {
        editor.setModel(model);
    }
}

function syncStateFromModel() {
    if (!editor) return;
    const model = editor.getModel();
    if (!model) return;
    const path = model.uri.path.startsWith("/") ? model.uri.path.slice(1) : model.uri.path;
    const content = model.getValue();
    playgroundState.updateFileContent(path, content);
}

onMount(async () => {
    registerSclLanguage();
    worker = new SclWorker();

    // Create initial model and editor
    const initialModel = getOrCreateModel(
        playgroundState.activeFile,
        playgroundState.activeFileContent,
    );
    editor = createEditor(editorContainer, initialModel);

    // Manually size Monaco to match its container
    const editorRef = editor;
    editorResizeObserver = new ResizeObserver((entries) => {
        const entry = entries[0];
        if (!entry) return;
        const { width, height } = entry.contentRect;
        editorRef.layout({ width, height });
    });
    editorResizeObserver.observe(editorContainer);

    // Sync editor content back to state on change
    editor.onDidChangeModelContent(() => {
        syncStateFromModel();
    });

    providersDisposable = registerProviders(
        worker,
        () => playgroundState.files,
        () => playgroundState.activeFile,
    );

    diagnosticsDisposable = setupDiagnostics(
        worker,
        () => playgroundState.files,
        (diags) => {
            playgroundState.setDiagnostics(diags);
            errorCount = diags.filter((d) => d.severity === "error").length;
            warningCount = diags.filter((d) => d.severity === "warning").length;
        },
    );

    // Initialize REPL
    await worker.replInit();
});

onDestroy(() => {
    editorResizeObserver?.disconnect();
    diagnosticsDisposable?.dispose();
    providersDisposable?.dispose();
    editor?.dispose();
    worker?.dispose();
    disposeAllModels();
});

function handleSelectFile(path: string) {
    // Save current model content
    syncStateFromModel();
    playgroundState.setActiveFile(path);
    syncModelFromState();
    mobileFileTreeOpen = false;
}

function handleCreateFile(path: string) {
    playgroundState.createFile(path);
    syncModelFromState();
}

function handleCreateFolder(path: string) {
    playgroundState.createFolder(path);
}

function handleDeleteEntry(path: string) {
    // Dispose Monaco model(s) for deleted files
    const files = playgroundState.files;
    const isFolder = !path.endsWith(".scl");
    if (isFolder) {
        const prefix = path.endsWith("/") ? path : `${path}/`;
        for (const filePath of Object.keys(files)) {
            if (filePath.startsWith(prefix)) {
                disposeModel(filePath);
            }
        }
    } else {
        disposeModel(path);
    }

    playgroundState.deleteEntry(path);
    syncModelFromState();
}

function handleRenameEntry(oldPath: string, newPath: string) {
    const content = playgroundState.files[oldPath] ?? "";
    playgroundState.renameEntry(oldPath, newPath);
    renameModel(oldPath, newPath, content);
    syncModelFromState();
}

function handleDiagnosticNavigate(file: string, line: number, character: number) {
    if (playgroundState.activeFile !== file) {
        syncStateFromModel();
        playgroundState.setActiveFile(file);
        syncModelFromState();
    }
    editor?.revealLineInCenter(line + 1);
    editor?.setPosition({ lineNumber: line + 1, column: character + 1 });
    editor?.focus();
}

async function handleReplEval(line: string) {
    if (!worker) return;
    const result = await worker.replEval(playgroundState.files, line);
    playgroundState.addReplEntry({
        input: line,
        output: result.output,
        effects: result.effects,
        error: result.error,
    });
}

async function handleReplReset() {
    if (!worker) return;
    await worker.replReset();
    playgroundState.clearReplHistory();
}

function handleReplClear() {
    playgroundState.clearReplHistory();
}
</script>

<svelte:head>
    <title>SCL Playground</title>
</svelte:head>

{#if resizing}
    <div
        class="fixed inset-0 z-50 touch-none {resizing === 'sidebar' ? 'cursor-col-resize' : 'cursor-row-resize'}"
    ></div>
{/if}
<div class="flex flex-1 min-h-0 relative" class:select-none={resizing}>
    <!-- File tree sidebar (desktop) -->
    {#if showFileTree}
        <div
            class="hidden md:flex border-r border-gray-200 bg-white shrink-0"
            style="width: {sidebarWidth}px"
        >
            <FileTree
                tree={playgroundState.fileTree}
                activeFile={playgroundState.activeFile}
                onSelectFile={handleSelectFile}
                onCreateFile={handleCreateFile}
                onCreateFolder={handleCreateFolder}
                onDeleteEntry={handleDeleteEntry}
                onRenameEntry={handleRenameEntry}
            />
        </div>
        <!-- Sidebar resize handle -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="hidden md:block w-1 -ml-0.5 -mr-0.5 shrink-0 cursor-col-resize hover:bg-blue-400 transition-colors z-10 relative before:absolute before:inset-y-0 before:-inset-x-2 before:content-[''] {resizing === 'sidebar' ? 'bg-blue-400' : ''}"
            onmousedown={(e) => startResize("sidebar", e)}
            ontouchstart={(e) => startResize("sidebar", e)}
        ></div>
    {/if}

    <!-- File tree drawer (mobile) -->
    {#if mobileFileTreeOpen}
        <div class="md:hidden absolute inset-0 z-30 flex">
            <div class="w-64 bg-white border-r border-gray-200 shadow-lg flex">
                <FileTree
                    tree={playgroundState.fileTree}
                    activeFile={playgroundState.activeFile}
                    onSelectFile={handleSelectFile}
                    onCreateFile={handleCreateFile}
                    onCreateFolder={handleCreateFolder}
                    onDeleteEntry={handleDeleteEntry}
                    onRenameEntry={handleRenameEntry}
                    onClose={() => (mobileFileTreeOpen = false)}
                />
            </div>
            <button
                class="flex-1 bg-black/20"
                onclick={() => (mobileFileTreeOpen = false)}
                aria-label="Close file tree"
            ></button>
        </div>
    {/if}

    <!-- Editor + top bar + bottom panel -->
    <div class="flex flex-col flex-1 min-w-0 min-h-0">
        <!-- Top bar -->
        <div
            class="h-10 bg-white border-b border-gray-200 flex items-center justify-between px-4 shrink-0"
        >
            <div class="flex items-center gap-2">
                <!-- Mobile toggles -->
                <button
                    class="md:hidden p-1 text-gray-500 hover:text-gray-700 rounded"
                    onclick={() => (mobileFileTreeOpen = !mobileFileTreeOpen)}
                    title="Toggle file tree"
                >
                    <PanelLeft class="w-4 h-4" />
                </button>
                <span class="text-xs font-medium text-gray-700"
                    >{playgroundState.activeFile}</span
                >
            </div>
            <div class="flex items-center gap-2">
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div
                    class="flex items-center gap-3 text-xs cursor-pointer"
                    class:hidden={showBottomPanel}
                    onclick={() => { showBottomPanel = true; activeBottomTab = "diagnostics"; }}
                    onkeydown={(e) => { if (e.key === "Enter") { showBottomPanel = true; activeBottomTab = "diagnostics"; } }}
                    role="button"
                    tabindex="0"
                >
                    {#if errorCount > 0}
                        <span class="text-red-600 flex items-center gap-1">
                            <AlertCircle class="w-3.5 h-3.5" />
                            {errorCount}
                        </span>
                    {/if}
                    {#if warningCount > 0}
                        <span class="text-yellow-600 flex items-center gap-1">
                            <AlertTriangle class="w-3.5 h-3.5" />
                            {warningCount}
                        </span>
                    {/if}
                </div>
                <!-- Desktop panel toggles -->
                <button
                    class="hidden md:block p-1 text-gray-400 hover:text-gray-600 rounded {showFileTree

                        ? 'bg-gray-100'
                        : ''}"
                    onclick={() => (showFileTree = !showFileTree)}
                    title="Toggle file tree"
                >
                    <PanelLeft class="w-4 h-4" />
                </button>
                <button
                    class="p-1 text-gray-400 hover:text-gray-600 rounded {showBottomPanel
                        ? 'bg-gray-100'
                        : ''}"
                    onclick={() => (showBottomPanel = !showBottomPanel)}
                    title="Toggle bottom panel"
                >
                    <PanelBottom class="w-4 h-4" />
                </button>
            </div>
        </div>

        <!-- Editor -->
        <div class="flex-1 min-h-0 relative">
            <div class="absolute inset-0" bind:this={editorContainer}></div>
        </div>

            <!-- Bottom panel (desktop) -->
            {#if showBottomPanel}
                <!-- Bottom panel resize handle -->
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div
                    class="h-1 -mt-0.5 -mb-0.5 shrink-0 cursor-row-resize hover:bg-blue-400 transition-colors z-10 relative before:absolute before:-inset-y-2 before:inset-x-0 before:content-[''] {resizing === 'bottom' ? 'bg-blue-400' : ''}"
                    onmousedown={(e) => startResize("bottom", e)}
                    ontouchstart={(e) => startResize("bottom", e)}
                ></div>
                <div
                    class="flex flex-col border-t border-gray-200 bg-white shrink-0"
                    style="height: {bottomPanelHeight}px"
                >
                    <!-- Tab bar -->
                    <div class="flex border-b border-gray-200 shrink-0">
                        <button
                            class="px-3 py-1.5 text-xs font-medium border-b-2 {activeBottomTab ===
                            'diagnostics'
                                ? 'border-blue-500 text-blue-600'
                                : 'border-transparent text-gray-500 hover:text-gray-700'}"
                            onclick={() => (activeBottomTab = "diagnostics")}
                        >
                            Diagnostics
                            <span
                                class="ml-1 px-1.5 py-0.5 rounded-full text-xs {errorCount > 0
                                    ? 'bg-red-100 text-red-700'
                                    : warningCount > 0
                                      ? 'bg-yellow-100 text-yellow-700'
                                      : 'bg-green-100 text-green-700'}"
                            >
                                {errorCount + warningCount}
                            </span>
                        </button>
                        <button
                            class="px-3 py-1.5 text-xs font-medium border-b-2 {activeBottomTab ===
                            'repl'
                                ? 'border-blue-500 text-blue-600'
                                : 'border-transparent text-gray-500 hover:text-gray-700'}"
                            onclick={() => (activeBottomTab = "repl")}
                        >
                            REPL
                        </button>
                        {#if activeBottomTab === "repl"}
                            <div class="flex-1"></div>
                            <div class="flex items-center gap-1 px-2">
                                <button
                                    class="p-1 text-gray-400 hover:text-gray-600 rounded"
                                    title="Clear output"
                                    onclick={handleReplClear}
                                >
                                    <Trash2 class="w-3.5 h-3.5" />
                                </button>
                                <button
                                    class="p-1 text-gray-400 hover:text-gray-600 rounded"
                                    title="Reset REPL state"
                                    onclick={handleReplReset}
                                >
                                    <RotateCcw class="w-3.5 h-3.5" />
                                </button>
                            </div>
                        {/if}
                    </div>
                    <!-- Tab content -->
                    <div class="flex-1 min-h-0 overflow-hidden">
                        {#if activeBottomTab === "diagnostics"}
                            <DiagnosticsPanel
                                diagnostics={playgroundState.diagnostics}
                                onNavigate={handleDiagnosticNavigate}
                            />
                        {:else}
                            <Repl
                                history={playgroundState.replHistory}
                                onEval={handleReplEval}
                            />
                        {/if}
                    </div>
                </div>
            {/if}
        </div>
    </div>
