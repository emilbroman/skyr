<script lang="ts">
	import JsonTree from './JsonTree.svelte';

	/**
	 * Renders serde-serialized sclc::Value JSON as an interactive tree.
	 *
	 * Tagged enum format from the backend:
	 *   {"Str": "hello"}, {"Int": 42}, {"Bool": true}, "Nil", {"List": [...]},
	 *   {"Record": {"fields": {...}}}, {"Dict": {"entries": [...]}},
	 *   {"Float": 3.14}, {"Pending": ...}, {"Fn": ...}, {"ExternFn": ...}, {"Exception": ...}
	 *
	 * Top-level inputs/outputs are bare records: {"fields": {"key": <Value>, ...}}.
	 */

	type Props = {
		value: unknown;
		key?: string;
		depth?: number;
		defaultExpanded?: number;
	};

	let { value, key, depth = 0, defaultExpanded = 2 }: Props = $props();

	let expandedOverride: boolean | null = $state(null);
	let isExpanded = $derived(expandedOverride ?? depth < defaultExpanded);

	function toggle() {
		expandedOverride = !isExpanded;
	}

	// --- serde tag interpretation ---

	type Resolved =
		| { kind: 'record'; entries: [string, unknown][]; label?: undefined }
		| { kind: 'dict'; entries: [string, unknown][]; label?: undefined }
		| { kind: 'list'; items: unknown[]; label?: undefined }
		| { kind: 'string'; display: string; raw: string; multiline: boolean; label?: undefined }
		| { kind: 'number'; display: string; label?: undefined }
		| { kind: 'boolean'; display: string; label?: undefined }
		| { kind: 'null'; display: string; label?: undefined }
		| { kind: 'special'; display: string; label: string };

	function resolve(raw: unknown): Resolved {
		if (raw == null) return { kind: 'null', display: 'nil' };

		if (typeof raw === 'number') return { kind: 'number', display: String(raw) };
		if (typeof raw === 'boolean') return { kind: 'boolean', display: String(raw) };

		// "Nil" as a bare string tag
		if (raw === 'Nil') return { kind: 'null', display: 'nil' };
		if (typeof raw === 'string') return { kind: 'string', display: JSON.stringify(raw), raw, multiline: raw.includes('\n') };

		if (Array.isArray(raw)) {
			return { kind: 'list', items: raw };
		}

		const obj = raw as Record<string, unknown>;
		const keys = Object.keys(obj);

		// Bare record (top-level inputs/outputs shape): {"fields": {...}}
		if (keys.length === 1 && keys[0] === 'fields' && typeof obj.fields === 'object' && obj.fields != null && !Array.isArray(obj.fields)) {
			const fields = obj.fields as Record<string, unknown>;
			return { kind: 'record', entries: Object.entries(fields) };
		}

		// Tagged enum: single key is the variant tag
		if (keys.length === 1) {
			const tag = keys[0];
			const inner = obj[tag];

			switch (tag) {
				case 'Str': {
					const s = String(inner);
					return { kind: 'string', display: JSON.stringify(s), raw: s, multiline: s.includes('\n') };
				}
				case 'Int':
					return { kind: 'number', display: String(inner) };
				case 'Float':
					return { kind: 'number', display: String(inner) };
				case 'Bool':
					return { kind: 'boolean', display: String(inner) };
				case 'Nil':
					return { kind: 'null', display: 'nil' };
				case 'List':
					if (Array.isArray(inner)) return { kind: 'list', items: inner };
					return { kind: 'special', display: '<list>', label: 'List' };
				case 'Record':
					if (inner && typeof inner === 'object' && 'fields' in (inner as Record<string, unknown>)) {
						const fields = (inner as Record<string, unknown>).fields as Record<string, unknown>;
						return { kind: 'record', entries: Object.entries(fields) };
					}
					return { kind: 'special', display: '<record>', label: 'Record' };
				case 'Dict': {
					if (inner && typeof inner === 'object' && 'entries' in (inner as Record<string, unknown>)) {
						const raw_entries = (inner as Record<string, unknown>).entries;
						if (Array.isArray(raw_entries)) {
							const dict_entries: [string, unknown][] = [];
							for (const entry of raw_entries) {
								if (Array.isArray(entry) && entry.length === 2) {
									const k = resolveToDisplayKey(entry[0]);
									dict_entries.push([k, entry[1]]);
								}
							}
							return { kind: 'dict', entries: dict_entries };
						}
					}
					return { kind: 'special', display: '<dict>', label: 'Dict' };
				}
				case 'Pending':
					return { kind: 'special', display: '<pending>', label: 'Pending' };
				case 'Fn':
					return { kind: 'special', display: '<function>', label: 'Fn' };
				case 'ExternFn':
					return { kind: 'special', display: '<function>', label: 'Fn' };
				case 'Exception':
					return { kind: 'special', display: '<exception>', label: 'Exception' };
			}
		}

		// Unrecognized object — treat as record
		return { kind: 'record', entries: Object.entries(obj) };
	}

	function resolveToDisplayKey(raw: unknown): string {
		const r = resolve(raw);
		if (r.kind === 'string') return JSON.parse(r.display);
		if ('display' in r) return r.display;
		return String(raw);
	}

	let resolved = $derived(resolve(value));
</script>

{#if resolved.kind === 'record' || resolved.kind === 'dict'}
	{@const entries = resolved.entries}
	{@const isDict = resolved.kind === 'dict'}
	{@const count = entries.length}
	<span>
		{#if key !== undefined}
			<span class="text-purple-300">{key}</span><span class="text-gray-500">: </span>
		{/if}
		{#if count === 0}
			<span class="text-gray-600">{isDict ? 'Dict {}' : '{}'}</span>
		{:else}
			<button
				onclick={toggle}
				class="inline text-gray-500 hover:text-gray-300 transition-colors cursor-pointer select-none"
				aria-expanded={isExpanded}
			>
				<span class="inline-block w-3 text-center text-[10px] {isExpanded ? '' : 'rotate-[-90deg]'} transition-transform">&#9660;</span>
				{#if !isExpanded}
					<span class="text-gray-600">{isDict ? `Dict {${count} entr${count === 1 ? 'y' : 'ies'}}` : `{${count} field${count === 1 ? '' : 's'}}`}</span>
				{:else}
					<span class="text-gray-600">{isDict ? 'Dict {' : '{'}</span>
				{/if}
			</button>
			{#if isExpanded}
				<div class="ml-4 border-l border-gray-800 pl-3">
					{#each entries as [k, v]}
						<div class="leading-6">
							<JsonTree value={v} key={k} depth={depth + 1} {defaultExpanded} />
						</div>
					{/each}
				</div>
				<span class="text-gray-600">{'}'}</span>
			{/if}
		{/if}
	</span>
{:else if resolved.kind === 'list'}
	{@const items = resolved.items}
	{@const count = items.length}
	<span>
		{#if key !== undefined}
			<span class="text-purple-300">{key}</span><span class="text-gray-500">: </span>
		{/if}
		{#if count === 0}
			<span class="text-gray-600">[]</span>
		{:else}
			<button
				onclick={toggle}
				class="inline text-gray-500 hover:text-gray-300 transition-colors cursor-pointer select-none"
				aria-expanded={isExpanded}
			>
				<span class="inline-block w-3 text-center text-[10px] {isExpanded ? '' : 'rotate-[-90deg]'} transition-transform">&#9660;</span>
				{#if !isExpanded}
					<span class="text-gray-600">[{count} item{count === 1 ? '' : 's'}]</span>
				{:else}
					<span class="text-gray-600">[</span>
				{/if}
			</button>
			{#if isExpanded}
				<div class="ml-4 border-l border-gray-800 pl-3">
					{#each items as item}
						<div class="leading-6">
							<JsonTree value={item} depth={depth + 1} {defaultExpanded} />
						</div>
					{/each}
				</div>
				<span class="text-gray-600">]</span>
			{/if}
		{/if}
	</span>
{:else}
	<span>
		{#if key !== undefined}
			<span class="text-purple-300">{key}</span><span class="text-gray-500">: </span>
		{/if}
		{#if resolved.kind === 'string' && resolved.multiline}
			<div class="ml-4 border-l border-gray-800 pl-3">
				<pre class="text-green-400 whitespace-pre multiline-string">{resolved.raw}</pre>
			</div>
		{:else if resolved.kind === 'string'}
			<span class="text-green-400">{resolved.display}</span>
		{:else if resolved.kind === 'number'}
			<span class="text-blue-400">{resolved.display}</span>
		{:else if resolved.kind === 'boolean'}
			<span class="text-yellow-400">{resolved.display}</span>
		{:else if resolved.kind === 'null'}
			<span class="text-gray-500 italic">{resolved.display}</span>
		{:else if resolved.kind === 'special'}
			<span class="text-orange-400 italic">{resolved.display}</span>
		{/if}
	</span>
{/if}

<style>
	.multiline-string {
		position: relative;
	}
	.multiline-string::before {
		content: '"';
		position: absolute;
		right: 100%;
		top: 0;
		color: var(--color-green-400);
		opacity: 0.5;
		user-select: none;
		pointer-events: none;
	}
	.multiline-string::after {
		content: '"';
		color: var(--color-green-400);
		opacity: 0.5;
		user-select: none;
		pointer-events: none;
	}
</style>
