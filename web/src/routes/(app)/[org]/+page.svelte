<script lang="ts">
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { query } from '$lib/graphql/client';
	import { OrganizationDetailDocument, type OrganizationDetailQuery } from '$lib/graphql/generated';
	import { DeploymentState } from '$lib/graphql/generated';
	import { repoHref } from '$lib/paths';

	let orgName = $derived($page.params.org ?? '');

	type OrgData = OrganizationDetailQuery['organization'];
	let org = $state<OrgData | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);

	onMount(async () => {
		try {
			const data = await query(OrganizationDetailDocument, { org: orgName });
			org = data.organization;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load organization';
		} finally {
			loading = false;
		}
	});
</script>

<div class="p-6">
	<nav class="text-sm text-gray-500 mb-4">
		<a href="/" class="hover:text-gray-300">Home</a>
		<span class="mx-2">/</span>
		<span class="text-gray-300">{orgName}</span>
	</nav>

	<h1 class="text-2xl font-bold text-white mb-6">{orgName}</h1>

	{#if loading}
		<p class="text-gray-400">Loading repositories...</p>
	{:else if error}
		<div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">{error}</div>
	{:else if org}
		{#if org.repositories.length === 0}
			<div class="text-center py-16">
				<p class="text-gray-400 mb-2">No repositories found.</p>
				<p class="text-gray-500 text-sm">Push an SCL project to create your first repository.</p>
			</div>
		{:else}
			<div class="grid gap-4">
				{#each org.repositories as repo}
					<a
						href={repoHref(orgName, repo.name)}
						class="block bg-gray-900 border border-gray-800 rounded-lg p-5 hover:border-gray-700 transition-colors"
					>
						<div class="flex items-center justify-between">
							<h2 class="text-lg font-medium text-white">{repo.name}</h2>
							<span class="text-sm text-gray-500">{repo.environments.length} environment{repo.environments.length !== 1 ? 's' : ''}</span>
						</div>
						{#if repo.environments.length > 0}
							<div class="mt-3 flex flex-wrap gap-2">
								{#each repo.environments as env}
									<span class="text-xs px-2 py-1 bg-gray-800 rounded text-gray-400">
										{env.name}
									</span>
								{/each}
							</div>
						{/if}
					</a>
				{/each}
			</div>
		{/if}
	{/if}
</div>
