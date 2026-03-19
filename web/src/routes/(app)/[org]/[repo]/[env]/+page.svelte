<script lang="ts">
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { query } from '$lib/graphql/client';
	import {
		DeploymentState,
		EnvironmentDetailDocument,
		EnvironmentLogsDocument,
		type EnvironmentDetailQuery
	} from '$lib/graphql/generated';
	import DeploymentStateBadge from '$lib/components/DeploymentState.svelte';
	import ResourceCard from '$lib/components/ResourceCard.svelte';
	import LogStream from '$lib/components/LogStream.svelte';
	import FileBrowser from '$lib/components/FileBrowser.svelte';
	import { decodeSegment, orgHref, repoHref, deploymentHref } from '$lib/paths';

	let orgName = $derived($page.params.org ?? '');
	let repoName = $derived($page.params.repo ?? '');
	let envName = $derived(decodeSegment($page.params.env ?? ''));

	type EnvData = EnvironmentDetailQuery['organization']['repository']['environment'];
	let env = $state<EnvData | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let showLogs = $state(false);

	let desiredDeployment = $derived(
		env?.deployments.find((d) => d.state === DeploymentState.Desired || d.state === DeploymentState.Up) ?? null
	);

	let navigateToFile = $state<{ moduleId: string; line: number } | null>(null);

	function handleNavigateToSource(moduleId: string, line: number) {
		navigateToFile = { moduleId, line };
		// Scroll to the file browser section
		document.querySelector('[data-file-browser]')?.scrollIntoView({ behavior: 'smooth', block: 'start' });
	}

	onMount(async () => {
		try {
			const data = await query(EnvironmentDetailDocument, { org: orgName, repo: repoName, env: envName });
			env = data.organization.repository.environment;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load environment';
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
		<a href={repoHref(orgName, repoName)} class="hover:text-gray-300">{repoName}</a>
		<span class="mx-2">/</span>
		<span class="text-gray-300">{envName}</span>
	</nav>

	<div class="flex items-center justify-between mb-6">
		<h1 class="text-2xl font-bold text-white">{orgName}/{repoName} &mdash; {envName}</h1>
		{#if env}
			<button
				class="text-sm px-3 py-1.5 rounded border transition-colors {showLogs ? 'bg-indigo-600 border-indigo-500 text-white' : 'border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600'}"
				onclick={() => showLogs = !showLogs}
			>
				{showLogs ? 'Hide Logs' : 'Stream Logs'}
			</button>
		{/if}
	</div>

	{#if loading}
		<p class="text-gray-400">Loading environment...</p>
	{:else if error}
		<div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">{error}</div>
	{:else if env}
		{#if showLogs}
			<div class="mb-6 h-80 bg-gray-900 border border-gray-800 rounded-lg overflow-hidden">
				<LogStream
					document={EnvironmentLogsDocument}
					variables={{ environmentQid: env.qid, initialAmount: 50 }}
					logField="environmentLogs"
				/>
			</div>
		{/if}

		<!-- File Browser for desired deployment -->
		{#if desiredDeployment}
			<section class="mb-8" data-file-browser>
				<h2 class="text-lg font-medium text-gray-300 mb-3">
					Files
					<span class="text-gray-500 text-sm font-normal ml-2">
						from <span class="font-mono">{desiredDeployment.commit.hash.substring(0, 8)}</span>
					</span>
				</h2>
				<FileBrowser
					{orgName}
					{repoName}
					{envName}
					commitHash={desiredDeployment.commit.hash}
					resources={env?.resources}
					{navigateToFile}
				/>
			</section>
		{/if}

		<!-- Deployments -->
		<section class="mb-8">
			<h2 class="text-lg font-medium text-gray-300 mb-4">
				Deployments
				<span class="text-gray-500 text-sm font-normal ml-2">({env.deployments.length})</span>
			</h2>
			{#if env.deployments.length === 0}
				<p class="text-gray-500">No deployments.</p>
			{:else}
				<div class="space-y-3">
					{#each env.deployments as deployment}
						<a
							href={deploymentHref(orgName, repoName, envName, deployment.commit.hash)}
							class="block bg-gray-900 border border-gray-800 rounded-lg p-4 hover:border-gray-700 transition-colors"
						>
							<div class="flex items-center gap-4">
								<DeploymentStateBadge state={deployment.state} />
								<div class="min-w-0 flex-1">
									<div class="flex items-center gap-3">
										<span class="text-white font-mono text-sm truncate">{deployment.commit.hash.substring(0, 8)}</span>
										<span class="text-gray-500 text-xs">{deployment.ref}</span>
									</div>
									<div class="flex items-center gap-3 mt-1 text-xs text-gray-500">
										<span class="truncate">{deployment.commit.message}</span>
										<span>{new Date(deployment.createdAt).toLocaleString()}</span>
										<span>{deployment.resources.length} resource{deployment.resources.length !== 1 ? 's' : ''}</span>
									</div>
								</div>
							</div>
						</a>
					{/each}
				</div>
			{/if}
		</section>

		<!-- Resources -->
		<section>
			<h2 class="text-lg font-medium text-gray-300 mb-4">
				Resources
				<span class="text-gray-500 text-sm font-normal ml-2">({env.resources.length})</span>
			</h2>
			{#if env.resources.length === 0}
				<p class="text-gray-500">No resources.</p>
			{:else}
				<div class="space-y-2">
					{#each env.resources as resource}
						<ResourceCard {resource} onNavigateToSource={handleNavigateToSource} />
					{/each}
				</div>
			{/if}
		</section>
	{/if}
</div>
