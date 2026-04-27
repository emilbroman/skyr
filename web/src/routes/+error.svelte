<script lang="ts">
import { page } from "$app/state";
import { AlertOctagon, AlertTriangle } from "lucide-svelte";

let status = $derived(page.status);
let message = $derived(page.error?.message ?? "Something went wrong");
let isNotFound = $derived(status === 404);
</script>

<svelte:head>
    <title>{status} {isNotFound ? "Not Found" : "Error"} – Skyr</title>
</svelte:head>

<div class="flex flex-1 items-center justify-center px-6 py-16 min-h-[calc(100vh-3.5rem)]">
  <div class="text-center">
    <div class="flex items-center justify-center gap-1.5 text-gray-400 font-mono">
      <span>{status}</span>
      {#if isNotFound}
        <AlertOctagon class="w-4 h-4" />
      {:else}
        <AlertTriangle class="w-4 h-4" />
      {/if}
    </div>
    <p class="mt-1 font-bold text-gray-900 break-words">
      {isNotFound ? "Not found" : message}
    </p>
  </div>
</div>
