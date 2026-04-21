<script lang="ts">
import { page } from "$app/stores";
import RootTree from "$lib/components/RootTree.svelte";
import { DeploymentState, EnvironmentDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));

const envDetail = graphqlQuery(() => ({
    document: EnvironmentDetailDocument,
    variables: { org: orgName, repo: repoName, env: envName },
    refetchInterval: 10_000,
}));

let env = $derived(envDetail.data?.organization.repository.environment ?? null);

let desiredDeployment = $derived(
    env?.deployments.find((d) => d.state === DeploymentState.Desired) ?? null,
);
</script>

<svelte:head>
    <title>{orgName}/{repoName} ({envName}) – Skyr</title>
</svelte:head>

{#if desiredDeployment}
  <RootTree {orgName} {repoName} commitHash={desiredDeployment.commit.hash} />
{:else if env}
  <div class="text-center py-16">
    <p class="text-gray-500 mb-2">No deployment yet.</p>
    <p class="text-gray-400">
      Push a branch to get started.
    </p>
  </div>
{/if}
