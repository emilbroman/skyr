<script lang="ts">
	import { page } from '$app/stores';
	import { graphqlQuery } from '$lib/graphql/query';
	import { DeploymentState, EnvironmentDetailDocument } from '$lib/graphql/generated';
	import RootTree from '$lib/components/RootTree.svelte';
	import { decodeSegment } from '$lib/paths';

	let orgName = $derived($page.params.org ?? '');
	let repoName = $derived($page.params.repo ?? '');
	let envName = $derived(decodeSegment($page.params.env ?? ''));

	const envDetail = graphqlQuery(() => ({
		document: EnvironmentDetailDocument,
		variables: { org: orgName, repo: repoName, env: envName },
		refetchInterval: 10_000
	}));

	let env = $derived(envDetail.data?.organization.repository.environment ?? null);

	let desiredDeployment = $derived(
		env?.deployments.find((d) => d.state === DeploymentState.Desired || d.state === DeploymentState.Up) ?? null
	);
</script>

{#if desiredDeployment}
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
{/if}
