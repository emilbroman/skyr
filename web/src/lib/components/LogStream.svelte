<script lang="ts">
	import type { TypedDocumentNode } from '@graphql-typed-document-node/core';
	import { subscribe as wsSubscribe } from '$lib/graphql/ws';
	import { Severity, type Log } from '$lib/graphql/generated';
	import { onMount, onDestroy, tick } from 'svelte';

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
			case Severity.Error: return 'text-red-400';
			case Severity.Warning: return 'text-yellow-400';
			default: return 'text-gray-300';
		}
	}

	function formatTimestamp(ts: string): string {
		try {
			return new Date(ts).toLocaleTimeString();
		} catch {
			return ts;
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
					logs = [...logs.slice(-MAX_LOGS + 1), log];
					scrollToBottom();
				}
			},
			(err: Error) => {
				error = err.message;
			}
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

<div class="flex flex-col h-full">
	<div class="flex items-center justify-between px-3 py-2 border-b border-gray-700 bg-gray-900/50">
		<span class="text-xs font-medium text-gray-400 uppercase tracking-wide">Logs</span>
		<div class="flex items-center gap-2">
			{#if !autoScroll}
				<button
					class="text-xs text-indigo-400 hover:text-indigo-300"
					onclick={() => { autoScroll = true; scrollToBottom(); }}
				>
					Scroll to bottom
				</button>
			{/if}
			<span class="text-xs text-gray-500">{logs.length} entries</span>
		</div>
	</div>

	{#if error}
		<div class="p-3 bg-red-900/20 border-b border-red-800 text-red-300 text-sm">
			{error}
		</div>
	{/if}

	<div
		bind:this={container}
		onscroll={handleScroll}
		class="flex-1 overflow-y-auto font-mono text-xs p-3 space-y-0.5 bg-gray-950"
	>
		{#each logs as log}
			<div class="flex gap-2 leading-5 hover:bg-gray-900/50 px-1 rounded">
				<span class="text-gray-500 shrink-0 select-none">{formatTimestamp(log.timestamp)}</span>
				<span class="{severityColor(log.severity)} whitespace-pre-wrap break-all">{log.message}</span>
			</div>
		{/each}
		{#if logs.length === 0 && !error}
			<p class="text-gray-500 text-center py-8">Waiting for logs...</p>
		{/if}
	</div>
</div>
