<script lang="ts">
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { query } from '$lib/graphql/client';
	import {
		EnvironmentDetailDocument,
		DeploymentLogsDocument,
		type EnvironmentDetailQuery
	} from '$lib/graphql/generated';
	import DeploymentStateBadge from '$lib/components/DeploymentState.svelte';
	import ResourceCard from '$lib/components/ResourceCard.svelte';
	import LogStream from '$lib/components/LogStream.svelte';
	import FileBrowser from '$lib/components/FileBrowser.svelte';
	import { decodeSegment, repoHref, envHref } from '$lib/paths';

	let repoName = $derived(decodeSegment($page.params.repo ?? ''));
	let envName = $derived(decodeSegment($page.params.env ?? ''));
	let deploymentId = $derived(decodeSegment($page.params.deployment ?? ''));

	type DeploymentData = EnvironmentDetailQuery['repositories'][0]['environments'][0]['deployments'][0];
	let deployment = $state<DeploymentData | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let showFiles = $state(true);

	onMount(async () => {
		try {
			const data = await query(EnvironmentDetailDocument);
			const repo = data.repositories.find((r) => r.name === repoName);
			const env = repo?.environments.find((e) => e.name === envName);
			deployment = env?.deployments.find((d) => d.id === deploymentId) ?? null;
			if (!deployment) {
				error = `Deployment "${deploymentId}" not found`;
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load deployment';
		} finally {
			loading = false;
		}
	});
</script>

<div class="p-6">
	<nav class="text-sm text-gray-500 mb-4">
		<a href="/repos" class="hover:text-gray-300">Repositories</a>
		<span class="mx-2">/</span>
		<a href={repoHref(repoName)} class="hover:text-gray-300">{repoName}</a>
		<span class="mx-2">/</span>
		<a href={envHref(repoName, envName)} class="hover:text-gray-300">{envName}</a>
		<span class="mx-2">/</span>
		<span class="text-gray-300 font-mono text-xs">{deploymentId}</span>
	</nav>

	{#if loading}
		<p class="text-gray-400">Loading deployment...</p>
	{:else if error}
		<div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">{error}</div>
	{:else if deployment}
		<!-- Header -->
		<div class="flex items-center gap-4 mb-6">
			<DeploymentStateBadge state={deployment.state} />
			<h1 class="text-xl font-bold text-white font-mono">{deployment.id}</h1>
		</div>

		<!-- Metadata -->
		<div class="bg-gray-900 border border-gray-800 rounded-lg p-4 mb-6">
			<dl class="grid grid-cols-2 gap-x-6 gap-y-3 text-sm">
				<div>
					<dt class="text-gray-500">Ref</dt>
					<dd class="text-gray-200">{deployment.ref}</dd>
				</div>
				<div>
					<dt class="text-gray-500">Commit</dt>
					<dd class="text-gray-200 font-mono" title={deployment.commit.message}>{deployment.commit.hash.substring(0, 8)} &mdash; {deployment.commit.message}</dd>
				</div>
				<div>
					<dt class="text-gray-500">Created</dt>
					<dd class="text-gray-200">{new Date(deployment.createdAt).toLocaleString()}</dd>
				</div>
				<div>
					<dt class="text-gray-500">State</dt>
					<dd class="text-gray-200">{deployment.state}</dd>
				</div>
			</dl>
		</div>

		<!-- File Browser -->
		<section class="mb-6">
			<div class="flex items-center justify-between mb-3">
				<h2 class="text-lg font-medium text-gray-300">Files</h2>
				<button
					class="text-sm px-3 py-1.5 rounded border transition-colors {showFiles ? 'bg-indigo-600 border-indigo-500 text-white' : 'border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600'}"
					onclick={() => showFiles = !showFiles}
				>
					{showFiles ? 'Hide Files' : 'Browse Files'}
				</button>
			</div>
			{#if showFiles}
				<FileBrowser
					{repoName}
					{envName}
					deploymentId={deployment.id}
					commitHash={deployment.commit.hash}
				/>
			{/if}
		</section>

		<!-- Artifacts -->
		{#if deployment.artifacts.length > 0}
			<section class="mb-6">
				<h2 class="text-lg font-medium text-gray-300 mb-3">Artifacts</h2>
				<div class="space-y-2">
					{#each deployment.artifacts as artifact}
						<div class="bg-gray-900 border border-gray-800 rounded-lg px-4 py-3 flex items-center justify-between">
							<div>
								<span class="text-gray-200 text-sm">{artifact.name}</span>
								<span class="text-gray-500 text-xs ml-2">({artifact.mediaType})</span>
							</div>
							<a
								href={artifact.url}
								target="_blank"
								rel="noopener noreferrer"
								class="text-indigo-400 hover:text-indigo-300 text-sm"
							>
								Download
							</a>
						</div>
					{/each}
				</div>
			</section>
		{/if}

		<!-- Resources -->
		<section class="mb-6">
			<h2 class="text-lg font-medium text-gray-300 mb-3">
				Resources
				<span class="text-gray-500 text-sm font-normal ml-1">({deployment.resources.length})</span>
			</h2>
			{#if deployment.resources.length === 0}
				<p class="text-gray-500">No resources.</p>
			{:else}
				<div class="space-y-2">
					{#each deployment.resources as resource}
						<ResourceCard resource={{ ...resource, markers: resource.markers, owner: null, dependencies: [] }} />
					{/each}
				</div>
			{/if}
		</section>

		<!-- Streaming Logs -->
		<section>
			<h2 class="text-lg font-medium text-gray-300 mb-3">Live Logs</h2>
			<div class="h-96 bg-gray-900 border border-gray-800 rounded-lg overflow-hidden">
				<LogStream
					document={DeploymentLogsDocument}
					variables={{ deploymentId: deployment.id, initialAmount: 100 }}
					logField="deploymentLogs"
				/>
			</div>
		</section>

		<!-- Recent logs snapshot (from query) -->
		{#if deployment.lastLogs.length > 0}
			<section class="mt-6">
				<h2 class="text-lg font-medium text-gray-300 mb-3">Recent Log Snapshot</h2>
				<div class="bg-gray-900 border border-gray-800 rounded-lg p-3 font-mono text-xs space-y-0.5 max-h-60 overflow-y-auto">
					{#each deployment.lastLogs as log}
						<div class="flex gap-2 leading-5">
							<span class="text-gray-500 shrink-0">{new Date(log.timestamp).toLocaleTimeString()}</span>
							<span class="{log.severity === 'ERROR' ? 'text-red-400' : log.severity === 'WARNING' ? 'text-yellow-400' : 'text-gray-300'}">{log.message}</span>
						</div>
					{/each}
				</div>
			</section>
		{/if}
	{/if}
</div>
