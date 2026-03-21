<script lang="ts">
import { CommitRootTreeDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import DirectoryView from "./DirectoryView.svelte";
import Spinner from "./Spinner.svelte";

type Props = {
    orgName: string;
    repoName: string;
    commitHash: string;
};

let { orgName, repoName, commitHash }: Props = $props();

const rootTree = graphqlQuery(() => ({
    document: CommitRootTreeDocument,
    variables: { org: orgName, repo: repoName, commit: commitHash },
}));

let entries = $derived(rootTree.data?.organization.repository.commit.tree.entries ?? []);
</script>

{#if rootTree.isPending}
  <Spinner />
{:else if rootTree.error}
  <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
    {rootTree.error.message}
  </div>
{:else}
  <DirectoryView {orgName} {repoName} {commitHash} {entries} />
{/if}
