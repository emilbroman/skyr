<script lang="ts">
import { onMount, onDestroy } from "svelte";
import * as monaco from "monaco-editor";
import { SclWorker } from "$lib/playground/client.js";
import {
    registerSclLanguage,
    registerProviders,
    setupDiagnostics,
    createEditor,
} from "$lib/playground/monaco.js";

let editorContainer: HTMLDivElement;
let editor: ReturnType<typeof createEditor> | undefined;
let worker: SclWorker | undefined;
let providersDisposable: { dispose(): void } | undefined;
let diagnosticsDisposable: { dispose(): void } | undefined;
let errorCount = $state(0);
let warningCount = $state(0);

const defaultContent = `// Welcome to the SCL Playground!
let greeting = "Hello, world!"
`;

onMount(() => {
    registerSclLanguage();
    worker = new SclWorker();

    editor = createEditor(editorContainer, defaultContent);
    providersDisposable = registerProviders(worker);
    diagnosticsDisposable = setupDiagnostics(editor, worker);

    // Track marker counts for the status bar
    const markerListener = editor.onDidChangeModelDecorations(() => {
        const model = editor?.getModel();
        if (!model) return;
        const markers = monaco.editor.getModelMarkers({ resource: model.uri });
        errorCount = markers.filter((m) => m.severity === monaco.MarkerSeverity.Error).length;
        warningCount = markers.filter((m) => m.severity === monaco.MarkerSeverity.Warning).length;
    });

    return () => {
        markerListener.dispose();
    };
});

onDestroy(() => {
    diagnosticsDisposable?.dispose();
    providersDisposable?.dispose();
    editor?.dispose();
    worker?.dispose();
});
</script>

<svelte:head>
    <title>SCL Playground</title>
</svelte:head>

<div class="flex flex-col flex-1 min-h-0">
    <div
        class="h-10 bg-white border-b border-gray-200 flex items-center justify-between px-4 shrink-0"
    >
        <span class="text-sm font-medium text-gray-700">SCL Playground</span>
        <div class="flex items-center gap-3 text-xs">
            {#if errorCount > 0}
                <span class="text-red-600">{errorCount} error{errorCount !== 1 ? "s" : ""}</span>
            {/if}
            {#if warningCount > 0}
                <span class="text-yellow-600"
                    >{warningCount} warning{warningCount !== 1 ? "s" : ""}</span
                >
            {/if}
            {#if errorCount === 0 && warningCount === 0}
                <span class="text-green-600">No issues</span>
            {/if}
        </div>
    </div>
    <div class="flex-1 min-h-0" bind:this={editorContainer}></div>
</div>
