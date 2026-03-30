<script lang="ts">
import { AlertCircle, AlertTriangle } from "lucide-svelte";
import type { DiagnosticInfo } from "./client.js";

type Props = {
    diagnostics: DiagnosticInfo[];
    onNavigate: (file: string, line: number, character: number) => void;
};

let { diagnostics, onNavigate }: Props = $props();
</script>

<div class="flex flex-col h-full text-sm overflow-y-auto">
        {#if diagnostics.length === 0}
            <div class="p-4 text-center text-gray-400 text-xs">No diagnostics</div>
        {:else}
            {#each diagnostics as diag}
                <button
                    class="w-full text-left px-3 py-1.5 hover:bg-gray-50 flex items-start gap-2 border-b border-gray-100"
                    onclick={() => onNavigate(diag.file, diag.line, diag.character)}
                >
                    {#if diag.severity === "error"}
                        <AlertCircle class="w-3.5 h-3.5 text-red-500 shrink-0 mt-0.5" />
                    {:else}
                        <AlertTriangle class="w-3.5 h-3.5 text-yellow-500 shrink-0 mt-0.5" />
                    {/if}
                    <div class="min-w-0 flex-1">
                        <div class="text-gray-900 text-xs">{diag.message}</div>
                        <div class="text-gray-400 text-xs mt-0.5">
                            {diag.file}:{diag.line + 1}:{diag.character + 1}
                        </div>
                    </div>
                </button>
            {/each}
        {/if}
</div>
