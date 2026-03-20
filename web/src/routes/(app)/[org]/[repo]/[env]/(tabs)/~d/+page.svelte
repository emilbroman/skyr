<script lang="ts">
import { page } from "$app/stores";
import DeploymentStateBadge from "$lib/components/DeploymentState.svelte";
import { EnvironmentDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment, deploymentHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));

const envDetail = graphqlQuery(() => ({
    document: EnvironmentDetailDocument,
    variables: { org: orgName, repo: repoName, env: envName },
    refetchInterval: 10_000,
}));

let env = $derived(envDetail.data?.organization.repository.environment ?? null);
</script>

{#if env}
  {#if env.deployments.length === 0}
    <p class="text-gray-400">No deployments.</p>
  {:else}
    <div class="space-y-3">
      {#each env.deployments as deployment}
        <a
          href={deploymentHref(
            orgName,
            repoName,
            envName,
            deployment.commit.hash,
          )}
          class="block bg-gray-900 border border-gray-800 rounded-lg p-4 hover:border-gray-700 transition-colors"
        >
          <div class="flex items-center justify-between gap-2">
            <span class="text-sm text-white truncate"
              >{deployment.commit.message.split("\n")[0]}</span
            >
            <span class="text-xs text-gray-500 font-mono shrink-0"
              >{deployment.commit.hash.substring(0, 7)}</span
            >
          </div>
          <div class="mt-1.5">
            <DeploymentStateBadge state={deployment.state} size="small" />
          </div>
          <div class="mt-1 text-xs text-gray-500 flex items-center gap-2">
            <span>{new Date(deployment.createdAt).toLocaleString()}</span>
            <span
              >{deployment.resources.length} resource{deployment.resources
                .length !== 1
                ? "s"
                : ""}</span
            >
          </div>
        </a>
      {/each}
    </div>
  {/if}
{/if}
