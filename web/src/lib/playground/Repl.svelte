<script lang="ts">
import { RotateCcw, Trash2 } from "lucide-svelte";
import type { ReplEntry } from "./state.svelte.js";

type Props = {
    history: ReplEntry[];
    onEval: (line: string) => void;
    onReset: () => void;
    onClear: () => void;
};

let { history, onEval, onReset, onClear }: Props = $props();

let inputValue = $state("");
let inputHistory: string[] = [];
let historyIndex = $state(-1);
let scrollContainer: HTMLDivElement | undefined = $state();
let isEvaluating = $state(false);

function handleSubmit() {
    const line = inputValue.trim();
    if (!line || isEvaluating) return;
    inputHistory.push(line);
    historyIndex = -1;
    isEvaluating = true;
    onEval(line);
    inputValue = "";
    // isEvaluating will be reset when history changes (new entry added)
}

// Reset evaluating state when history gets a new entry
$effect(() => {
    if (history.length > 0) {
        isEvaluating = false;
    }
});

// Auto-scroll to bottom when history changes
$effect(() => {
    if (history.length > 0 && scrollContainer) {
        // Use a microtask to let the DOM update first
        queueMicrotask(() => {
            scrollContainer?.scrollTo({ top: scrollContainer.scrollHeight });
        });
    }
});

function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSubmit();
    } else if (e.key === "ArrowUp") {
        if (inputHistory.length === 0) return;
        e.preventDefault();
        if (historyIndex === -1) {
            historyIndex = inputHistory.length - 1;
        } else if (historyIndex > 0) {
            historyIndex--;
        }
        inputValue = inputHistory[historyIndex];
    } else if (e.key === "ArrowDown") {
        if (historyIndex === -1) return;
        e.preventDefault();
        if (historyIndex < inputHistory.length - 1) {
            historyIndex++;
            inputValue = inputHistory[historyIndex];
        } else {
            historyIndex = -1;
            inputValue = "";
        }
    }
}
</script>

<div class="flex flex-col h-full text-sm font-mono">
    <div class="flex items-center justify-between px-3 py-1 border-b border-gray-200 shrink-0">
        <span class="text-xs text-gray-500">REPL</span>
        <div class="flex gap-1">
            <button
                class="p-1 text-gray-400 hover:text-gray-600 rounded"
                title="Clear output"
                onclick={onClear}
            >
                <Trash2 class="w-3.5 h-3.5" />
            </button>
            <button
                class="p-1 text-gray-400 hover:text-gray-600 rounded"
                title="Reset REPL state"
                onclick={onReset}
            >
                <RotateCcw class="w-3.5 h-3.5" />
            </button>
        </div>
    </div>
    <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1" bind:this={scrollContainer}>
        {#each history as entry}
            <div>
                <div class="text-gray-500">
                    <span class="text-blue-500 select-none">scl&gt; </span>{entry.input}
                </div>
                {#if entry.effects && entry.effects.length > 0}
                    {#each entry.effects as effect}
                        <div class="text-purple-600 pl-6 text-xs">{effect}</div>
                    {/each}
                {/if}
                {#if entry.output}
                    <div class="text-gray-900 pl-6">{entry.output}</div>
                {/if}
                {#if entry.error}
                    <div class="text-red-600 pl-6 text-xs whitespace-pre-wrap">{entry.error}</div>
                {/if}
            </div>
        {/each}
        {#if isEvaluating}
            <div class="text-gray-400 pl-6 text-xs">Evaluating...</div>
        {/if}
    </div>
    <div class="flex items-center border-t border-gray-200 px-3 py-1.5 shrink-0">
        <span class="text-blue-500 select-none mr-1">scl&gt;</span>
        <input
            class="flex-1 bg-transparent outline-none text-gray-900 text-sm font-mono"
            bind:value={inputValue}
            onkeydown={handleKeydown}
            placeholder="Enter expression..."
            disabled={isEvaluating}
        />
    </div>
</div>
