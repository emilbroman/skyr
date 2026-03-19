<script lang="ts">
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { query } from '$lib/graphql/client';
	import { DeploymentState, RepositoryDetailDocument, type RepositoryDetailQuery } from '$lib/graphql/generated';
	import DeploymentStateBadge from '$lib/components/DeploymentState.svelte';
	import FileBrowser from '$lib/components/FileBrowser.svelte';
	import { orgHref, envHref } from '$lib/paths';

	let orgName = $derived($page.params.org ?? '');
	let repoName = $derived($page.params.repo ?? '');

	type RepoData = RepositoryDetailQuery['organization']['repository'];
	let repo = $state<RepoData | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);

	// Find the "main" environment (named "main" or the first one) and its desired deployment
	let mainEnv = $derived(
		repo?.environments.find((e) => e.name === 'main') ?? repo?.environments[0] ?? null
	);

	let mainDesiredDeployment = $derived(
		mainEnv?.deployments.find(
			(d) => d.state === DeploymentState.Desired || d.state === DeploymentState.Up
		) ?? null
	);

	onMount(async () => {
		try {
			const data = await query(RepositoryDetailDocument, { org: orgName, repo: repoName });
			repo = data.organization.repository;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load repository';
		} finally {
			loading = false;
		}
	});
</script>

<div class="p-6">
	<nav class="text-sm text-gray-500 mb-4">
		<a href="/" class="hover:text-gray-300">Home</a>
		<span class="mx-2">/</span>
		<a href={orgHref(orgName)} class="hover:text-gray-300">{orgName}</a>
		<span class="mx-2">/</span>
		<span class="text-gray-300">{repoName}</span>
	</nav>

	<h1 class="text-2xl font-bold text-white mb-6">{orgName}/{repoName}</h1>

	{#if loading}
		<p class="text-gray-400">Loading repository...</p>
	{:else if error}
		<div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">{error}</div>
	{:else if repo}
		<!-- File Browser for main environment's desired deployment -->
		{#if mainEnv && mainDesiredDeployment}
			<section class="mb-8">
				<h2 class="text-lg font-medium text-gray-300 mb-3">
					Files
					<span class="text-gray-500 text-sm font-normal ml-2">
						{mainEnv.name} &middot; <span class="font-mono">{mainDesiredDeployment.commit.hash.substring(0, 8)}</span>
						&mdash; {mainDesiredDeployment.commit.message}
					</span>
				</h2>
				<FileBrowser
					{orgName}
					{repoName}
					envName={mainEnv.name}
					commitHash={mainDesiredDeployment.commit.hash}
				/>
			</section>
		{/if}

		<!-- Environments -->
		{#if repo.environments.length === 0}
			<p class="text-gray-400">No environments found in this repository.</p>
		{:else}
			<h2 class="text-lg font-medium text-gray-300 mb-4">Environments</h2>
			<div class="grid gap-4">
				{#each repo.environments as env}
					{@const desired = env.deployments.find(
						(d) => d.state === DeploymentState.Desired || d.state === DeploymentState.Up
					)}
					<a
						href={envHref(orgName, repoName, env.name)}
						class="block bg-gray-900 border border-gray-800 rounded-lg p-5 hover:border-gray-700 transition-colors"
					>
						<div class="flex items-center justify-between">
							<div class="flex items-center gap-3">
								<h3 class="text-lg font-medium text-white">{env.name}</h3>
								{#if desired}
									<DeploymentStateBadge state={desired.state} />
								{/if}
							</div>
							<span class="text-xs text-gray-500 font-mono">{env.qid}</span>
						</div>
						{#if desired}
							<div class="mt-2 text-xs text-gray-500 flex items-center gap-3">
								<span class="font-mono">{desired.commit.hash.substring(0, 8)}</span>
								<span class="truncate">{desired.commit.message}</span>
							</div>
						{/if}
						<div class="mt-1 text-xs text-gray-600">
							{env.deployments.length} deployment{env.deployments.length !== 1 ? 's' : ''}
						</div>
					</a>
				{/each}
			</div>
		{/if}
	{/if}
</div>
