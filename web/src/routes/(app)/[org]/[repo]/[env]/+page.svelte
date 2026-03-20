<script lang="ts">
	import { page } from '$app/stores';
	import { graphqlQuery } from '$lib/graphql/query';
	import {
		DeploymentState,
		EnvironmentDetailDocument,
		EnvironmentLogsDocument
	} from '$lib/graphql/generated';
	import DeploymentStateBadge from '$lib/components/DeploymentState.svelte';
	import ResourceCard from '$lib/components/ResourceCard.svelte';
	import LogStream from '$lib/components/LogStream.svelte';
	import RootTree from '$lib/components/RootTree.svelte';
	import { decodeSegment, orgHref, repoHref, deploymentHref, commitTreeHref } from '$lib/paths';

	let orgName = $derived($page.params.org ?? '');
	let repoName = $derived($page.params.repo ?? '');
	let envName = $derived(decodeSegment($page.params.env ?? ''));

	const envDetail = graphqlQuery(() => ({
		document: EnvironmentDetailDocument,
		variables: { org: orgName, repo: repoName, env: envName },
		refetchInterval: 10_000
	}));

	let env = $derived(envDetail.data?.organization.repository.environment ?? null);
	let showLogs = $state(false);

	let desiredDeployment = $derived(
		env?.deployments.find((d) => d.state === DeploymentState.Desired || d.state === DeploymentState.Up) ?? null
	);

	function handleNavigateToSource(moduleId: string, line: number) {
		if (!desiredDeployment) return;
		const segments = moduleId.split('/');
		const localPath = segments.length > 2 ? segments.slice(2).join('/') : moduleId;
		const filePath = localPath + '.scl';
		const url = commitTreeHref(orgName, repoName, desiredDeployment.commit.hash, filePath) + `#line-${line}`;
		window.location.href = url;
	}
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

	{#if envDetail.isPending}
		<p class="text-gray-400">Loading environment...</p>
	{:else if envDetail.error}
		<div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">{envDetail.error.message}</div>
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

		<!-- Root tree for desired deployment -->
		{#if desiredDeployment}
			<section class="mb-8">
				<h2 class="text-lg font-medium text-gray-300 mb-3">
					Files
					<span class="text-gray-500 text-sm font-normal ml-2">
						from <span class="font-mono">{desiredDeployment.commit.hash.substring(0, 8)}</span>
					</span>
				</h2>
				<RootTree
					{orgName}
					{repoName}
					commitHash={desiredDeployment.commit.hash}
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
