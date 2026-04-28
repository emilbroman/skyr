<script lang="ts">
import type { TypedDocumentNode } from "@graphql-typed-document-node/core";
import { onDestroy, onMount, tick } from "svelte";
import { type Log, Severity } from "$lib/graphql/generated";
import { subscribe as wsSubscribe } from "$lib/graphql/ws";
import { formatLogTimestamp } from "$lib/timestamps";

type Props = {
    document: TypedDocumentNode<any, any>;
    variables: Record<string, unknown>;
    logField: string;
};

let { document, variables, logField }: Props = $props();

let logs = $state<Log[]>([]);
let error = $state<string | null>(null);
let container: HTMLElement;
let autoScroll = $state(true);
let unsubscribe: (() => void) | null = null;

const MAX_LOGS = 1000;

function insertSorted(list: Log[], log: Log): Log[] {
    let lo = 0;
    let hi = list.length;
    while (lo < hi) {
        const mid = (lo + hi) >>> 1;
        if (list[mid].timestamp <= log.timestamp) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    const next = list.slice();
    next.splice(lo, 0, log);
    if (next.length > MAX_LOGS) {
        next.splice(0, next.length - MAX_LOGS);
    }
    return next;
}

function handleScroll() {
    if (!container) return;
    const { scrollTop, scrollHeight, clientHeight } = container;
    autoScroll = scrollHeight - scrollTop - clientHeight < 40;
}

async function scrollToBottom() {
    if (autoScroll && container) {
        await tick();
        container.scrollTop = container.scrollHeight;
    }
}

function severityColor(severity: Severity): string {
    switch (severity) {
        case Severity.Error:
            return "text-red-400";
        case Severity.Warning:
            return "text-yellow-400";
        default:
            return "text-gray-300";
    }
}

function startSubscription() {
    if (unsubscribe) {
        unsubscribe();
    }
    logs = [];
    error = null;

    unsubscribe = wsSubscribe(
        document,
        variables,
        (data: any) => {
            const log = data[logField] as Log;
            if (log) {
                logs = insertSorted(logs, log);
                scrollToBottom();
            }
        },
        (err: Error) => {
            error = err.message;
        },
    );
}

onMount(() => {
    startSubscription();
});

onDestroy(() => {
    if (unsubscribe) {
        unsubscribe();
        unsubscribe = null;
    }
});
</script>

<div class="relative h-full">
  {#if !autoScroll}
    <button
      class="absolute top-2 right-2 z-10 px-2 py-1 bg-gray-800/90 backdrop-blur border border-gray-700 rounded shadow-sm text-blue-400 hover:text-blue-300 transition-colors"
      onclick={() => {
        autoScroll = true;
        scrollToBottom();
      }}
    >
      Scroll to bottom
    </button>
  {/if}

  {#if error}
    <div class="p-3 bg-red-900/30 border-b border-red-800 text-red-400">
      {error}
    </div>
  {/if}

  <div
    bind:this={container}
    onscroll={handleScroll}
    class="h-full overflow-y-auto font-mono text-xs p-3 space-y-0.5 bg-gray-900"
  >
    {#each logs as log}
      <div class="flex flex-col sm:flex-row sm:gap-2 leading-5 hover:bg-gray-800 px-1 rounded">
        <span class="text-gray-500 shrink-0 select-none"
          >{formatLogTimestamp(log.timestamp)}</span
        >
        <span
          class="{severityColor(log.severity)} whitespace-pre-wrap break-all"
          >{log.message}</span
        >
      </div>
    {/each}
    {#if logs.length === 0 && !error}
      <p class="text-gray-500 text-center py-8">Waiting for logs...</p>
    {/if}
  </div>
</div>
