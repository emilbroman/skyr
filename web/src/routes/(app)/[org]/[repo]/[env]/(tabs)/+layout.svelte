<script lang="ts">
	import { page } from '$app/stores';
	import { graphqlQuery } from '$lib/graphql/query';
	import { EnvironmentDetailDocument } from '$lib/graphql/generated';
	import { decodeSegment, orgHref, repoHref, envHref, envDeploymentsHref, envLogsHref, resourcesHref } from '$lib/paths';

	let { children } = $props();

	let orgName = $derived($page.params.org ?? '');
	let repoName = $derived($page.params.repo ?? '');
	let envName = $derived(decodeSegment($page.params.env ?? ''));

	const envDetail = graphqlQuery(() => ({
		document: EnvironmentDetailDocument,
		variables: { org: orgName, repo: repoName, env: envName },
		refetchInterval: 10_000
	}));

	let env = $derived(envDetail.data?.organization.repository.environment ?? null);

	let currentPath = $derived($page.url.pathname);
	let envBase = $derived(envHref(orgName, repoName, envName));
	let deploymentsPath = $derived(envDeploymentsHref(orgName, repoName, envName));
	let resPath = $derived(resourcesHref(orgName, repoName, envName));
	let logsPath = $derived(envLogsHref(orgName, repoName, envName));
	let activeTab = $derived(
		currentPath.startsWith(deploymentsPath) ? 'deployments' :
		currentPath.startsWith(resPath) ? 'resources' :
		currentPath.startsWith(logsPath) ? 'logs' :
		'files'
	);
</script>

<div class="p-6">
	<nav class="text-sm text-gray-500 mb-4">
		<a href={orgHref(orgName)} class="hover:text-gray-300">{orgName}</a>
		<span class="mx-2">/</span>
		<a href={repoHref(orgName, repoName)} class="hover:text-gray-300">{repoName}</a>
		<span class="mx-2">/</span>
		<span class="text-gray-300">{envName}</span>
	</nav>

	<h1 class="text-2xl font-bold text-white mb-6">{orgName}/{repoName} &mdash; {envName}</h1>

	{#if envDetail.isPending}
		<p class="text-gray-400">Loading environment...</p>
	{:else if envDetail.error}
		<div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">{envDetail.error.message}</div>
	{:else if env}
		<!-- Tabs -->
		<div class="flex gap-1 border-b border-gray-800 mb-4">
			<a
				href={envBase}
				class="px-3 py-2 text-sm transition-colors border-b-2 -mb-px {activeTab === 'files' ? 'border-indigo-500 text-white' : 'border-transparent text-gray-400 hover:text-gray-200'}"
			>
				Files
			</a>
			<a
				href={deploymentsPath}
				class="px-3 py-2 text-sm transition-colors border-b-2 -mb-px {activeTab === 'deployments' ? 'border-indigo-500 text-white' : 'border-transparent text-gray-400 hover:text-gray-200'}"
			>
				Deployments <span class="text-gray-500 ml-1">({env.deployments.length})</span>
			</a>
			<a
				href={resPath}
				class="px-3 py-2 text-sm transition-colors border-b-2 -mb-px {activeTab === 'resources' ? 'border-indigo-500 text-white' : 'border-transparent text-gray-400 hover:text-gray-200'}"
			>
				Resources <span class="text-gray-500 ml-1">({env.resources.length})</span>
			</a>
			<a
				href={logsPath}
				class="px-3 py-2 text-sm transition-colors border-b-2 -mb-px {activeTab === 'logs' ? 'border-indigo-500 text-white' : 'border-transparent text-gray-400 hover:text-gray-200'}"
			>
				Logs
			</a>
		</div>

		{@render children()}
	{/if}
</div>
