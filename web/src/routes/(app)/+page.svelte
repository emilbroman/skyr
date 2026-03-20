<script lang="ts">
	import { graphqlQuery } from '$lib/graphql/query';
	import { OrganizationsDocument } from '$lib/graphql/generated';
	import { orgHref } from '$lib/paths';

	const organizations = graphqlQuery(() => ({
		document: OrganizationsDocument
	}));
</script>

<div class="p-6">
	<h1 class="text-2xl font-bold text-white mb-6">Organizations</h1>

	{#if organizations.isPending}
		<p class="text-gray-400">Loading organizations...</p>
	{:else if organizations.error}
		<div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">{organizations.error.message}</div>
	{:else if organizations.data.organizations.length === 0}
		<div class="text-center py-16">
			<p class="text-gray-400 mb-2">No organizations found.</p>
			<p class="text-gray-500 text-sm">Create an organization to get started.</p>
		</div>
	{:else}
		<div class="grid gap-4">
			{#each organizations.data.organizations as org}
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
