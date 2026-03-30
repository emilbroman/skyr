<script lang="ts">
import type { ReplEntry } from "./state.svelte.js";

function autofocus(node: HTMLElement) {
    node.focus();
}

type Props = {
    history: ReplEntry[];
    onEval: (line: string) => void;
};

let { history, onEval }: Props = $props();

let inputValue = $state("");
let inputHistory: string[] = [];
let historyIndex = $state(-1);
let inputEl: HTMLInputElement | undefined = $state();
let isEvaluating = $state(false);

function handleSubmit() {
    const line = inputValue.trim();
    if (!line || isEvaluating) return;
    inputHistory.push(line);
    historyIndex = -1;
    isEvaluating = true;
    onEval(line);
    inputValue = "";
    // Re-focus after DOM updates
    queueMicrotask(() => inputEl?.focus());
    // isEvaluating will be reset when history changes (new entry added)
}

// Reset evaluating state when history gets a new entry, and re-focus input
$effect(() => {
    if (history.length > 0) {
        isEvaluating = false;
        queueMicrotask(() => inputEl?.focus());
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

<div class="flex flex-col h-full text-xs leading-5 font-mono">
    <div class="flex-1 overflow-y-auto flex flex-col-reverse px-3 py-2">
        <div class="space-y-1">
            {#each history as entry}
                <div>
                    <div class="text-gray-500">
                        <span class="text-blue-500 select-none">scl&gt;&nbsp;</span>{entry.input}
                    </div>
                    {#if entry.effects && entry.effects.length > 0}
                        {#each entry.effects as effect}
                            <div class="text-purple-600 pl-6">{effect}</div>
                        {/each}
                    {/if}
                    {#if entry.output}
                        <div class="text-gray-900 pl-6">{entry.output}</div>
                    {/if}
                    {#if entry.error}
                        <div class="text-red-600 pl-6 whitespace-pre-wrap"
                            >{entry.error}</div
                        >
                    {/if}
                </div>
            {/each}
            {#if isEvaluating}
                <div class="text-gray-400 pl-6">Evaluating...</div>
            {/if}
        </div>
    </div>
    <div class="flex items-center border-t border-gray-200 px-3 py-1.5 shrink-0">
        <span class="text-blue-500 select-none">scl&gt;&nbsp;</span>
        <input
            use:autofocus
            bind:this={inputEl}
            class="flex-1 bg-transparent outline-none text-gray-900"
            bind:value={inputValue}
            onkeydown={handleKeydown}
            placeholder="Enter expression..."
            disabled={isEvaluating}
        />
    </div>
</div>
