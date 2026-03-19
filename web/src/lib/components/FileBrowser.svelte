<script lang="ts">
	import { query } from '$lib/graphql/client';
	import {
		DeploymentRootTreeDocument,
		DeploymentTreeDocument,
		type DeploymentRootTreeQuery,
		type DeploymentTreeQuery
	} from '$lib/graphql/generated';
	import { highlight, type HighlightedLine } from '$lib/highlight';
	import type { ThemedToken } from 'shiki';

	type SourceFrame = {
		moduleId: string;
		span: string;
		name: string;
	};

	type ResourceInfo = {
		type: string;
		name: string;
		sourceTrace?: SourceFrame[];
	};

	type Props = {
		orgName: string;
		repoName: string;
		envName: string;
		commitHash: string;
		resources?: ResourceInfo[];
		navigateToFile?: { moduleId: string; line: number } | null;
	};

	let { orgName, repoName, envName, commitHash, resources = [], navigateToFile = null }: Props = $props();

	type TreeEntry =
		| { __typename: 'Tree'; hash: string; name?: string | null }
		| { __typename: 'Blob'; hash: string; name?: string | null; size: number };

	type BlobContent = {
		__typename: 'Blob';
		hash: string;
		name?: string | null;
		size: number;
		content?: string | null;
	};

	let currentPath = $state<string[]>([]);
	let entries = $state<TreeEntry[]>([]);
	let blobContent = $state<BlobContent | null>(null);
	let highlightedLines = $state<ThemedToken[][] | null>(null);
	let highlightBg = $state<string>('#0d1117');
	let loading = $state(true);
	let error = $state<string | null>(null);

	let pathString = $derived(currentPath.join('/'));

	let queryVars = $derived({ org: orgName, repo: repoName, env: envName, commit: commitHash });

	async function loadRoot() {
		loading = true;
		error = null;
		blobContent = null;
		highlightedLines = null;
		try {
			const data = await query(DeploymentRootTreeDocument, queryVars);
			const dep = data.organization.repository.environment.deployment;
			const rawEntries = dep.commit.tree.entries;
			entries = sortEntries(rawEntries);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load tree';
		} finally {
			loading = false;
		}
	}

	async function loadPath(path: string) {
		loading = true;
		error = null;
		blobContent = null;
		highlightedLines = null;
		try {
			const data = await query(DeploymentTreeDocument, { ...queryVars, path });
			const dep = data.organization.repository.environment.deployment;
			const entry = dep.commit.treeEntry;
			if (!entry) {
				error = `Path "${path}" not found`;
				return;
			}
			if (entry.__typename === 'Tree') {
				entries = sortEntries(entry.entries);
				blobContent = null;
			} else {
				blobContent = entry;
				entries = [];
				if (entry.content != null && entry.name) {
					highlightCode(entry.content, entry.name);
				}
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load path';
		} finally {
			loading = false;
		}
	}

	async function highlightCode(code: string, filename: string) {
		try {
			const result = await highlight(code, filename);
			highlightedLines = result.lines;
			highlightBg = result.bg;
		} catch {
			// Highlighting failed — fall back to plain text (highlightedLines stays null)
		}
	}

	function sortEntries(raw: TreeEntry[]): TreeEntry[] {
		return [...raw].sort((a, b) => {
			if (a.__typename !== b.__typename) {
				return a.__typename === 'Tree' ? -1 : 1;
			}
			return (a.name ?? '').localeCompare(b.name ?? '');
		});
	}

	function navigateTo(name: string) {
		highlightLine = null;
		currentPath = [...currentPath, name];
	}

	function navigateUp() {
		highlightLine = null;
		currentPath = currentPath.slice(0, -1);
	}

	function navigateToIndex(index: number) {
		highlightLine = null;
		currentPath = currentPath.slice(0, index + 1);
	}

	function formatSize(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	let highlightLine = $state<number | null>(null);

	/**
	 * Strip the package prefix from a moduleId.
	 * Module IDs are fully qualified: "org/repo/Module" where "org/repo" is the
	 * 2-segment package prefix. The file path within the repo is everything after.
	 */
	function moduleIdToLocalPath(moduleId: string): string {
		const segments = moduleId.split('/');
		return segments.length > 2 ? segments.slice(2).join('/') : moduleId;
	}

	/**
	 * Build a map from line number to resource labels for the currently viewed file.
	 * Multiple resources can map to the same line.
	 */
	let resourceInlays = $derived.by(() => {
		if (!blobContent || !resources.length) return new Map<number, string[]>();

		// Current file path without .scl extension, to compare with stripped moduleId
		const currentFile = currentPath.join('/');
		const modulePathForFile = currentFile.replace(/\.scl$/, '');

		const inlays = new Map<number, string[]>();
		for (const resource of resources) {
			if (!resource.sourceTrace?.length) continue;
			const frame = resource.sourceTrace[0];
			if (moduleIdToLocalPath(frame.moduleId) !== modulePathForFile) continue;
			const line = parseSpanStartLine(frame.span);
			const label = `${resource.type}/${resource.name}`;
			const existing = inlays.get(line);
			if (existing) {
				existing.push(label);
			} else {
				inlays.set(line, [label]);
			}
		}
		return inlays;
	});

	function parseSpanStartLine(span: string): number {
		const startPart = span.split(',')[0];
		const line = parseInt(startPart.split(':')[0], 10);
		return isNaN(line) ? 1 : line;
	}

	// Handle external navigation requests
	$effect(() => {
		if (navigateToFile) {
			const localPath = moduleIdToLocalPath(navigateToFile.moduleId);
			const filePath = localPath.split('/');
			const lastSegment = filePath[filePath.length - 1];
			filePath[filePath.length - 1] = lastSegment + '.scl';
			currentPath = filePath;
			highlightLine = navigateToFile.line;
		}
	});

	// Scroll to highlighted line after render
	$effect(() => {
		if (highlightLine && !loading) {
			const el = document.getElementById(`line-${highlightLine}`);
			el?.scrollIntoView({ behavior: 'smooth', block: 'center' });
		}
	});

	$effect(() => {
		if (currentPath.length === 0) {
			loadRoot();
		} else {
			loadPath(currentPath.join('/'));
		}
	});
</script>

<div class="bg-gray-900 border border-gray-800 rounded-lg overflow-hidden">
	<!-- Header with breadcrumb -->
	<div class="flex items-center gap-2 px-4 py-3 border-b border-gray-800 bg-gray-900/80 text-sm">
		<button
			class="text-indigo-400 hover:text-indigo-300 font-medium"
			onclick={() => { currentPath = []; }}
		>
			{repoName}
		</button>
		{#each currentPath as segment, i}
			<span class="text-gray-600">/</span>
			{#if i < currentPath.length - 1}
				<button
					class="text-indigo-400 hover:text-indigo-300"
					onclick={() => navigateToIndex(i)}
				>
					{segment}
				</button>
			{:else}
				<span class="text-gray-300">{segment}</span>
			{/if}
		{/each}
		<span class="ml-auto text-xs text-gray-500 font-mono">{commitHash.substring(0, 8)}</span>
	</div>

	{#if loading}
		<div class="p-8 text-center text-gray-400">Loading...</div>
	{:else if error}
		<div class="p-4 text-red-400 text-sm">{error}</div>
	{:else if blobContent}
		<!-- File content view -->
		<div class="flex items-center justify-between px-4 py-2 border-b border-gray-800 bg-gray-800/30">
			<span class="text-sm text-gray-400">{formatSize(blobContent.size)}</span>
			<button
				class="text-xs text-indigo-400 hover:text-indigo-300"
				onclick={navigateUp}
			>
				Back to directory
			</button>
		</div>
		{#snippet resourceInlay(items: string[])}
			{#if items.length === 1}
				<span class="ml-4 text-xs text-indigo-400/70 font-sans select-none">{items[0]}</span>
			{:else}
				<span class="ml-4 relative inline-block font-sans select-none group/inlay">
					<span class="text-xs text-indigo-400/70 cursor-default">{items.length} resources</span>
					<div class="hidden group-hover/inlay:block absolute left-0 top-full z-10 mt-1 py-1 px-2 bg-gray-800 border border-gray-700 rounded shadow-lg whitespace-nowrap">
						{#each items as item}
							<div class="text-xs text-indigo-300 leading-5">{item}</div>
						{/each}
					</div>
				</span>
			{/if}
		{/snippet}
		{#if blobContent.content != null}
			<div class="overflow-x-auto" style="background:{highlightBg}">
				<table class="w-full text-sm font-mono leading-6 border-collapse">
					<tbody>
						{#if highlightedLines}
							{#each highlightedLines as tokens, i}
								{@const lineNum = i + 1}
								{@const inlay = resourceInlays.get(lineNum)}
								<tr
									id="line-{lineNum}"
									class="hover:bg-white/5 {highlightLine === lineNum ? 'bg-indigo-900/30' : ''}"
								>
									<td class="px-4 py-0 text-right text-gray-600 select-none align-top w-12 whitespace-nowrap">{lineNum}</td>
									<td class="px-4 py-0 whitespace-pre">{#each tokens as token}<span style="color:{token.color ?? ''};font-style:{token.fontStyle === 1 ? 'italic' : 'normal'}">{token.content}</span>{/each}{#if inlay}{@render resourceInlay(inlay)}{/if}</td>
								</tr>
							{/each}
						{:else}
							{#each blobContent.content.split('\n') as line, i}
								{@const lineNum = i + 1}
								{@const inlay = resourceInlays.get(lineNum)}
								<tr
									id="line-{lineNum}"
									class="hover:bg-white/5 {highlightLine === lineNum ? 'bg-indigo-900/30' : ''}"
								>
									<td class="px-4 py-0 text-right text-gray-600 select-none align-top w-12 whitespace-nowrap">{lineNum}</td>
									<td class="px-4 py-0 whitespace-pre text-gray-300">{line}{#if inlay}{@render resourceInlay(inlay)}{/if}</td>
								</tr>
							{/each}
						{/if}
					</tbody>
				</table>
			</div>
		{:else}
			<div class="p-8 text-center text-gray-500">
				Binary file ({formatSize(blobContent.size)})
			</div>
		{/if}
	{:else}
		<!-- Directory listing -->
		<div class="divide-y divide-gray-800/50">
			{#if currentPath.length > 0}
				<button
					class="w-full text-left px-4 py-2.5 flex items-center gap-3 hover:bg-gray-800/50 transition-colors text-sm"
					onclick={navigateUp}
				>
					<svg class="w-4 h-4 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
						<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 17l-5-5m0 0l5-5m-5 5h12" />
					</svg>
					<span class="text-gray-400">..</span>
				</button>
			{/if}
			{#each entries as entry}
				<button
					class="w-full text-left px-4 py-2.5 flex items-center gap-3 hover:bg-gray-800/50 transition-colors text-sm"
					onclick={() => navigateTo(entry.name ?? '')}
				>
					{#if entry.__typename === 'Tree'}
						<svg class="w-4 h-4 text-indigo-400 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
						</svg>
						<span class="text-gray-200">{entry.name}</span>
					{:else}
						<svg class="w-4 h-4 text-gray-500 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
						</svg>
						<span class="text-gray-300">{entry.name}</span>
						<span class="ml-auto text-xs text-gray-600">{formatSize(entry.size)}</span>
					{/if}
				</button>
			{/each}
			{#if entries.length === 0}
				<div class="p-8 text-center text-gray-500">Empty directory</div>
			{/if}
		</div>
	{/if}
</div>
