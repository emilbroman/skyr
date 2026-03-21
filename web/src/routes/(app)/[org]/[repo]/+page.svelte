<script lang="ts">
import { goto } from "$app/navigation";
import { page } from "$app/stores";
import Spinner from "$lib/components/Spinner.svelte";
import { RepositoryDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { envHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");

const repoDetail = graphqlQuery(() => ({
    document: RepositoryDetailDocument,
    variables: { org: orgName, repo: repoName },
}));

let repo = $derived(repoDetail.data?.organization.repository ?? null);

let targetEnv = $derived(
    repo?.environments.find((e) => e.name === "main") ?? repo?.environments[0] ?? null,
);

$effect(() => {
    if (targetEnv) {
        goto(envHref(orgName, repoName, targetEnv.name), { replaceState: true });
    }
});
</script>

{#if repoDetail.isPending}
    <Spinner />
{:else if repoDetail.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
        {repoDetail.error.message}
    </div>
{:else if repo && repo.environments.length === 0}
    <p class="text-gray-500">No environments found.</p>
{/if}
