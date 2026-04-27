<script lang="ts">
import type { ThemedToken } from "shiki";
import { highlightAs } from "$lib/highlight";

let { message }: { message: string } = $props();

let highlightedLines = $state<ThemedToken[][] | null>(null);

let trimmed = $derived(message.replace(/\n+$/, ""));

$effect(() => {
    highlightedLines = null;
    const text = trimmed;
    if (text) {
        highlightAs(text, "semantic-commit")
            .then((result) => {
                highlightedLines = result.lines;
            })
            .catch(() => {
                // fall back to plain text
            });
    }
});
</script>

<div class="font-mono text-xs leading-5 text-gray-700">
  {#if highlightedLines}
    {#each highlightedLines as tokens}
      <div class="whitespace-pre-wrap min-h-5"
        >{#each tokens as token}<span
            style="color:{token.color ?? ''};font-style:{token.fontStyle === 1 ? 'italic' : 'normal'}"
            >{token.content}</span
          >{/each}</div>
    {/each}
  {:else}
    {#each trimmed.split("\n") as line}
      <div class="whitespace-pre-wrap min-h-5">{line}</div>
    {/each}
  {/if}
</div>
