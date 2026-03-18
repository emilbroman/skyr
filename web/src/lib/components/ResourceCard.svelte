<script lang="ts">
	import { ResourceMarker } from '$lib/graphql/generated';

	type ResourceData = {
		type: string;
		name: string;
		inputs?: any;
		outputs?: any;
		markers: ResourceMarker[];
		owner?: { id: string } | null;
		dependencies?: { type: string; name: string }[];
	};

	let { resource }: { resource: ResourceData } = $props();
	let expanded = $state(false);

	function formatJson(value: any): string {
		if (value == null) return 'null';
		return JSON.stringify(value, null, 2);
	}
</script>

<div class="bg-gray-900 border border-gray-800 rounded-lg overflow-hidden">
	<button
		class="w-full text-left px-4 py-3 flex items-center justify-between hover:bg-gray-800/50 transition-colors"
		onclick={() => expanded = !expanded}
	>
		<div class="flex items-center gap-3">
			<span class="text-indigo-400 font-medium text-sm">{resource.type}</span>
			<span class="text-gray-300 text-sm">{resource.name}</span>
			{#each resource.markers as marker}
				<span class="text-xs px-1.5 py-0.5 rounded border {marker === ResourceMarker.Volatile ? 'border-yellow-700 text-yellow-400' : 'border-blue-700 text-blue-400'}">
					{marker}
				</span>
			{/each}
		</div>
		<svg
			class="w-4 h-4 text-gray-500 transition-transform {expanded ? 'rotate-180' : ''}"
			fill="none" viewBox="0 0 24 24" stroke="currentColor"
		>
			<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
		</svg>
	</button>

	{#if expanded}
		<div class="border-t border-gray-800 px-4 py-3 space-y-3 text-sm">
			{#if resource.owner}
				<div>
					<span class="text-gray-500">Owner:</span>
					<span class="text-gray-300 ml-2 font-mono text-xs">{resource.owner.id}</span>
				</div>
			{/if}

			{#if resource.dependencies && resource.dependencies.length > 0}
				<div>
					<span class="text-gray-500">Dependencies:</span>
					<div class="mt-1 flex flex-wrap gap-1">
						{#each resource.dependencies as dep}
							<span class="text-xs px-2 py-0.5 bg-gray-800 rounded text-gray-400">
								{dep.type}/{dep.name}
							</span>
						{/each}
					</div>
				</div>
			{/if}

			{#if resource.inputs != null}
				<div>
					<span class="text-gray-500">Inputs:</span>
					<pre class="mt-1 bg-gray-800 rounded p-2 text-xs text-gray-300 overflow-x-auto">{formatJson(resource.inputs)}</pre>
				</div>
			{/if}

			{#if resource.outputs != null}
				<div>
					<span class="text-gray-500">Outputs:</span>
					<pre class="mt-1 bg-gray-800 rounded p-2 text-xs text-gray-300 overflow-x-auto">{formatJson(resource.outputs)}</pre>
				</div>
			{/if}
		</div>
	{/if}
</div>
