<script lang="ts">
	import { ResourceMarker } from '$lib/graphql/generated';
	import { formatRecord } from '$lib/format';

	type SourceFrame = {
		moduleId: string;
		span: string;
		name: string;
	};

	type ResourceData = {
		type: string;
		name: string;
		inputs?: any;
		outputs?: any;
		markers: ResourceMarker[];
		owner?: { id: string } | null;
		dependencies?: { type: string; name: string }[];
		sourceTrace?: SourceFrame[];
	};

	let { resource, repoName = '', onNavigateToSource }: {
		resource: ResourceData;
		repoName?: string;
		onNavigateToSource?: (moduleId: string, line: number) => void;
	} = $props();
	let expanded = $state(false);

	function formatJson(value: any): string {
		if (value == null) return 'null';
		return JSON.stringify(formatRecord(value), null, 2);
	}

	/**
	 * Strip the repo QID prefix from a moduleId.
	 * Module IDs are fully qualified: "org/repo/Module" where "org/repo" is the repo name.
	 * The file path within the repo is the suffix after the repo name prefix.
	 */
	function stripRepoPrefix(moduleId: string): string {
		if (repoName && moduleId.startsWith(repoName + '/')) {
			return moduleId.slice(repoName.length + 1);
		}
		return moduleId;
	}

	/** Parse the first source frame into a displayable location string and line number. */
	function parseSourceLocation(trace: SourceFrame[] | undefined): { label: string; moduleId: string; line: number } | null {
		if (!trace || trace.length === 0) return null;
		const frame = trace[0];
		const line = parseSpanStartLine(frame.span);
		const localPath = stripRepoPrefix(frame.moduleId);
		const filePath = localPath + '.scl';
		return { label: `${filePath}:${line}`, moduleId: frame.moduleId, line };
	}

	function parseSpanStartLine(span: string): number {
		// Span format: "startLine:startChar,endLine:endChar"
		const startPart = span.split(',')[0];
		const line = parseInt(startPart.split(':')[0], 10);
		return isNaN(line) ? 1 : line;
	}

	let sourceLocation = $derived(parseSourceLocation(resource.sourceTrace));
</script>

<div class="bg-gray-900 border border-gray-800 rounded-lg overflow-hidden">
	<div class="w-full text-left px-4 py-3 flex items-center justify-between hover:bg-gray-800/50 transition-colors">
		<button class="flex items-center gap-3 min-w-0 flex-1" onclick={() => expanded = !expanded}>
			<span class="text-indigo-400 font-medium text-sm">{resource.type}</span>
			<span class="text-gray-300 text-sm">{resource.name}</span>
			{#each resource.markers as marker}
				<span class="text-xs px-1.5 py-0.5 rounded border {marker === ResourceMarker.Volatile ? 'border-yellow-700 text-yellow-400' : 'border-blue-700 text-blue-400'}">
					{marker}
				</span>
			{/each}
		</button>
		{#if sourceLocation}
			<button
				class="text-xs text-gray-500 hover:text-indigo-400 font-mono mx-3 transition-colors shrink-0"
				onclick={() => onNavigateToSource?.(sourceLocation!.moduleId, sourceLocation!.line)}
				title="Go to source"
			>
				{sourceLocation.label}
			</button>
		{/if}
		<button
			class="shrink-0"
			onclick={() => expanded = !expanded}
			title="Toggle details"
		>
			<svg
				class="w-4 h-4 text-gray-500 transition-transform {expanded ? 'rotate-180' : ''}"
				fill="none" viewBox="0 0 24 24" stroke="currentColor"
			>
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
			</svg>
		</button>
	</div>

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
