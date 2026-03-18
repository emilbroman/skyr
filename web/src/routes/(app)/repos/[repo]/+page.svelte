<script lang="ts">
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { query } from '$lib/graphql/client';
	import { RepositoriesDocument, type RepositoriesQuery } from '$lib/graphql/generated';

	let repoName = $derived($page.params.repo);
	let repo = $state<RepositoriesQuery['repositories'][0] | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);

	onMount(async () => {
		try {
			const data = await query(RepositoriesDocument);
			repo = data.repositories.find((r) => r.name === repoName) ?? null;
			if (!repo) {
				error = `Repository "${repoName}" not found`;
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load repository';
		} finally {
			loading = false;
		}
	});
</script>

<div class="p-6">
	<nav class="text-sm text-gray-500 mb-4">
		<a href="/repos" class="hover:text-gray-300">Repositories</a>
		<span class="mx-2">/</span>
		<span class="text-gray-300">{repoName}</span>
	</nav>

	<h1 class="text-2xl font-bold text-white mb-6">{repoName}</h1>

	{#if loading}
		<p class="text-gray-400">Loading environments...</p>
	{:else if error}
		<div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">{error}</div>
	{:else if repo}
		{#if repo.environments.length === 0}
			<p class="text-gray-400">No environments found in this repository.</p>
		{:else}
			<h2 class="text-lg font-medium text-gray-300 mb-4">Environments</h2>
			<div class="grid gap-4">
				{#each repo.environments as env}
					<a
						href="/repos/{repoName}/{env.name}"
						class="block bg-gray-900 border border-gray-800 rounded-lg p-5 hover:border-gray-700 transition-colors"
					>
						<div class="flex items-center justify-between">
							<h3 class="text-lg font-medium text-white">{env.name}</h3>
							<span class="text-xs text-gray-500 font-mono">{env.qid}</span>
						</div>
					</a>
				{/each}
			</div>
		{/if}
	{/if}
</div>
