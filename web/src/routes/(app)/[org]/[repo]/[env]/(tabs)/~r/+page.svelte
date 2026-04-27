<script lang="ts">
import { page } from "$app/stores";
import ResourceList from "$lib/components/ResourceList.svelte";
import { EnvironmentDetailDocument } from "$lib/graphql/generated";
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
</script>

<svelte:head>
    <title>Resources · {orgName}/{repoName} ({envName}) – Skyr</title>
</svelte:head>

{#if env}
  <ResourceList
    resources={env.resources}
    org={orgName}
    repo={repoName}
    env={envName}
    emptyMessage="No resources in this environment."
  />
{/if}
