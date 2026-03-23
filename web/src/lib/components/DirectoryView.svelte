<script lang="ts">
import { CommitTreeEntryDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { commitTreeHref } from "$lib/paths";
import { ArrowLeft, FileText, Folder } from "lucide-svelte";
import FileView from "./FileView.svelte";
import Spinner from "./Spinner.svelte";

type TreeEntry =
    | { __typename: "Tree"; hash: string; name?: string | null }
    | { __typename: "Blob"; hash: string; name?: string | null; size: number };

type Props = {
    orgName: string;
    repoName: string;
    commitHash: string;
    /** Path segments leading to this directory (empty for root) */
    path?: string[];
    entries: TreeEntry[];
};

let { orgName, repoName, commitHash, path = [], entries }: Props = $props();

let sortedEntries = $derived(
    [...entries].sort((a, b) => {
        if (a.__typename !== b.__typename) return a.__typename === "Tree" ? -1 : 1;
        return (a.name ?? "").localeCompare(b.name ?? "");
    }),
);

function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function entryHref(name: string): string {
    const segments = [...path, name].join("/");
    return commitTreeHref(orgName, repoName, commitHash, segments);
}

function parentHref(): string {
    if (path.length <= 1) return commitTreeHref(orgName, repoName, commitHash);
    const parentPath = path.slice(0, -1).join("/");
    return commitTreeHref(orgName, repoName, commitHash, `${parentPath}/`);
}

// README detection and rendering
let readmeEntry = $derived(
    sortedEntries.find((e) => e.__typename === "Blob" && /^readme(\.\w+)?$/i.test(e.name ?? "")) as
        | { __typename: "Blob"; hash: string; name?: string | null; size: number }
        | undefined,
);

let readmePath = $derived(readmeEntry?.name ? [...path, readmeEntry.name].join("/") : null);

let readmeQuery = graphqlQuery(() => ({
    document: CommitTreeEntryDocument,
    variables: {
        org: orgName,
        repo: repoName,
        commit: commitHash,
        path: readmePath!,
    },
    enabled: readmePath != null,
}));

let readmeContent = $derived.by(() => {
    const entry = readmeQuery.data?.organization.repository.commit.treeEntry;
    if (entry?.__typename === "Blob" && entry.content != null) {
        return entry.content;
    }
    return null;
});
</script>

<div class="bg-white border border-gray-200 rounded-lg overflow-hidden">
  <div class="divide-y divide-gray-200">
    {#if path.length > 0}
      <a
        href={parentHref()}
        class="w-full text-left px-4 py-2.5 flex items-center gap-3 hover:bg-gray-100 transition-colors"
      >
        <ArrowLeft class="w-4 h-4 text-gray-400" />
        <span class="text-gray-500">..</span>
      </a>
    {/if}
    {#each sortedEntries as entry}
      <a
        href={entryHref(entry.name ?? "")}
        class="w-full text-left px-4 py-2.5 flex items-center gap-3 hover:bg-gray-100 transition-colors"
      >
        {#if entry.__typename === "Tree"}
          <Folder class="w-4 h-4 text-orange-600 shrink-0" />
          <span class="text-gray-700">{entry.name}</span>
        {:else}
          <FileText class="w-4 h-4 text-gray-400 shrink-0" />
          <span class="text-gray-600">{entry.name}</span>
          <span class="ml-auto text-gray-400"
            >{formatSize(entry.size)}</span
          >
        {/if}
      </a>
    {/each}
    {#if sortedEntries.length === 0}
      <div class="p-8 text-center text-gray-400">Empty directory</div>
    {/if}
  </div>
</div>

{#if readmeQuery.isPending && readmePath != null}
  <div class="mt-4">
    <Spinner />
  </div>
{:else if readmeContent != null && readmeEntry}
  <div class="mt-4">
    <FileView
      {orgName}
      {repoName}
      {commitHash}
      path={[...path, readmeEntry.name ?? ""]}
      content={readmeContent}
      size={readmeEntry.size}
    />
  </div>
{/if}
