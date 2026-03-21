<script lang="ts">
import { CommitRootTreeDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import DirectoryView from "./DirectoryView.svelte";

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
  <div
    class="bg-gray-900 border border-gray-800 rounded-lg p-8 text-center text-gray-400"
  >
    Loading...
  </div>
{:else if rootTree.error}
  <div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">
    {rootTree.error.message}
  </div>
{:else}
  <DirectoryView {orgName} {repoName} {commitHash} {entries} />
{/if}
