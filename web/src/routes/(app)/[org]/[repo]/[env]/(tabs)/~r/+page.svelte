<script lang="ts">
import { page } from "$app/stores";
import ResourceDag from "$lib/components/ResourceDag.svelte";
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

{#if env}
  <ResourceDag
    resources={env.resources}
    org={orgName}
    repo={repoName}
    env={envName}
  />
{/if}
