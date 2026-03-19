<script lang="ts">
	import { query } from '$lib/graphql/client';
	import {
		DeploymentRootTreeDocument,
		DeploymentTreeDocument,
		type DeploymentRootTreeQuery,
		type DeploymentTreeQuery
	} from '$lib/graphql/generated';

	type Props = {
		repoName: string;
		envName: string;
		deploymentId: string;
		commitHash: string;
	};

	let { repoName, envName, deploymentId, commitHash }: Props = $props();

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
	let loading = $state(true);
	let error = $state<string | null>(null);

	let pathString = $derived(currentPath.join('/'));

	function findDeploymentData<T extends { id: string }>(
		repos: Array<{
			name: string;
			environments: Array<{
				name: string;
				deployments: Array<T>;
			}>;
		}>
	): T | undefined {
		const repo = repos.find((r) => r.name === repoName);
		const env = repo?.environments.find((e) => e.name === envName);
		return env?.deployments.find((d) => d.id === deploymentId);
	}

	async function loadRoot() {
		loading = true;
		error = null;
		blobContent = null;
		try {
			const data = await query(DeploymentRootTreeDocument);
			const dep = findDeploymentData(data.repositories);
			if (!dep) {
				error = 'Deployment not found';
				return;
			}
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
		try {
			const data = await query(DeploymentTreeDocument, { path });
			const dep = findDeploymentData(data.repositories);
			if (!dep) {
				error = 'Deployment not found';
				return;
			}
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
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load path';
		} finally {
			loading = false;
		}
	}

	function sortEntries(raw: TreeEntry[]): TreeEntry[] {
		return [...raw].sort((a, b) => {
			// Directories first
			if (a.__typename !== b.__typename) {
				return a.__typename === 'Tree' ? -1 : 1;
			}
			return (a.name ?? '').localeCompare(b.name ?? '');
		});
	}

	function navigateTo(name: string) {
		currentPath = [...currentPath, name];
	}

	function navigateUp() {
		currentPath = currentPath.slice(0, -1);
	}

	function navigateToIndex(index: number) {
		currentPath = currentPath.slice(0, index + 1);
	}

	function formatSize(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	// React to path changes
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
		{#if blobContent.content != null}
			<div class="overflow-x-auto">
				<pre class="p-4 text-sm text-gray-300 font-mono leading-6 whitespace-pre">{#each blobContent.content.split('\n') as line, i}<span class="inline-block w-12 text-right text-gray-600 select-none mr-4">{i + 1}</span>{line}
{/each}</pre>
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
