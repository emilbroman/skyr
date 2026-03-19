<script lang="ts">
	import { onMount } from 'svelte';
	import { query } from '$lib/graphql/client';
	import { OrganizationsDocument, type OrganizationsQuery } from '$lib/graphql/generated';
	import { orgHref } from '$lib/paths';

	let organizations = $state<OrganizationsQuery['organizations']>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	onMount(async () => {
		try {
			const data = await query(OrganizationsDocument);
			organizations = data.organizations;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load organizations';
		} finally {
			loading = false;
		}
	});
</script>

<div class="p-6">
	<h1 class="text-2xl font-bold text-white mb-6">Organizations</h1>

	{#if loading}
		<p class="text-gray-400">Loading organizations...</p>
	{:else if error}
		<div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">{error}</div>
	{:else if organizations.length === 0}
		<div class="text-center py-16">
			<p class="text-gray-400 mb-2">No organizations found.</p>
			<p class="text-gray-500 text-sm">Create an organization to get started.</p>
		</div>
	{:else}
		<div class="grid gap-4">
			{#each organizations as org}
				<a
					href={orgHref(org.name)}
					class="block bg-gray-900 border border-gray-800 rounded-lg p-5 hover:border-gray-700 transition-colors"
				>
					<div class="flex items-center justify-between">
						<h2 class="text-lg font-medium text-white">{org.name}</h2>
						<span class="text-sm text-gray-500">{org.repositories.length} repositor{org.repositories.length !== 1 ? 'ies' : 'y'}</span>
					</div>
				</a>
			{/each}
		</div>
	{/if}
</div>
