<script lang="ts">
	import { page } from '$app/stores';
	import { graphqlQuery } from '$lib/graphql/query';
	import { EnvironmentDetailDocument } from '$lib/graphql/generated';
	import ResourceCardCompact from '$lib/components/ResourceCardCompact.svelte';
	import { decodeSegment, deploymentHref, resourceHref } from '$lib/paths';

	let orgName = $derived($page.params.org ?? '');
	let repoName = $derived($page.params.repo ?? '');
	let envName = $derived(decodeSegment($page.params.env ?? ''));

	const envDetail = graphqlQuery(() => ({
		document: EnvironmentDetailDocument,
		variables: { org: orgName, repo: repoName, env: envName },
		refetchInterval: 10_000
	}));

	let env = $derived(envDetail.data?.organization.repository.environment ?? null);

	type Deployment = NonNullable<typeof env>['deployments'][number];

	let deploymentMap = $derived(() => {
		const map = new Map<string, Deployment>();
		for (const d of env?.deployments ?? []) {
			map.set(d.id, d);
		}
		return map;
	});

	type ResourceGroup = {
		deployment: Deployment | null;
		resources: NonNullable<typeof env>['resources'];
	};

	let groupedResources = $derived.by(() => {
		if (!env) return [];
		const dMap = deploymentMap();
		const groups = new Map<string, ResourceGroup>();
		const ungrouped: NonNullable<typeof env>['resources'] = [];

		for (const resource of env.resources) {
			const ownerId = resource.owner?.id;
			if (ownerId) {
				let group = groups.get(ownerId);
				if (!group) {
					group = { deployment: dMap.get(ownerId) ?? null, resources: [] };
					groups.set(ownerId, group);
				}
				group.resources.push(resource);
			} else {
				ungrouped.push(resource);
			}
		}

		// Sort groups: deployments with known info first (by createdAt desc), unknown owners last
		const result: ResourceGroup[] = [...groups.values()].sort((a, b) => {
			if (a.deployment && b.deployment) {
				return new Date(b.deployment.createdAt).getTime() - new Date(a.deployment.createdAt).getTime();
			}
			return a.deployment ? -1 : 1;
		});

		if (ungrouped.length > 0) {
			result.push({ deployment: null, resources: ungrouped });
		}

		return result;
	});
</script>

{#if env}
	{#if env.resources.length === 0}
		<p class="text-gray-400">No resources in this environment.</p>
	{:else}
		<div class="space-y-6">
			{#each groupedResources as group}
				<section>
					{#if group.deployment}
						<a
							href={deploymentHref(orgName, repoName, envName, group.deployment.commit.hash)}
							class="flex items-center gap-2 mb-2 group"
						>
							<h3 class="text-sm font-medium text-gray-300 group-hover:text-white transition-colors truncate">
								{group.deployment.commit.message.split('\n')[0]}
							</h3>
							<span class="text-xs text-gray-600 font-mono shrink-0">{group.deployment.commit.hash.substring(0, 7)}</span>
						</a>
					{:else}
						<h3 class="text-sm font-medium text-gray-500 mb-2">Unowned</h3>
					{/if}
					<div class="space-y-1.5">
						{#each group.resources as resource}
							<ResourceCardCompact {resource} href={resourceHref(orgName, repoName, envName, `${resource.type}:${resource.name}`)} />
						{/each}
					</div>
				</section>
			{/each}
		</div>
	{/if}
{/if}
