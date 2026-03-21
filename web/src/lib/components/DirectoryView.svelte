<script lang="ts">
import { CommitTreeEntryDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { commitTreeHref } from "$lib/paths";
import FileView from "./FileView.svelte";

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

<div class="bg-gray-900 border border-gray-800 rounded-lg overflow-hidden">
  <div class="divide-y divide-gray-800/50">
    {#if path.length > 0}
      <a
        href={parentHref()}
        class="w-full text-left px-4 py-2.5 flex items-center gap-3 hover:bg-gray-800/50 transition-colors text-sm"
      >
        <svg
          class="w-4 h-4 text-gray-500"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M11 17l-5-5m0 0l5-5m-5 5h12"
          />
        </svg>
        <span class="text-gray-400">..</span>
      </a>
    {/if}
    {#each sortedEntries as entry}
      <a
        href={entryHref(entry.name ?? "")}
        class="w-full text-left px-4 py-2.5 flex items-center gap-3 hover:bg-gray-800/50 transition-colors text-sm"
      >
        {#if entry.__typename === "Tree"}
          <svg
            class="w-4 h-4 text-indigo-400 shrink-0"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
          >
            <path
              stroke-linecap="round"
              stroke-linejoin="round"
              stroke-width="2"
              d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"
            />
          </svg>
          <span class="text-gray-200">{entry.name}</span>
        {:else}
          <svg
            class="w-4 h-4 text-gray-500 shrink-0"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
          >
            <path
              stroke-linecap="round"
              stroke-linejoin="round"
              stroke-width="2"
              d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
            />
          </svg>
          <span class="text-gray-300">{entry.name}</span>
          <span class="ml-auto text-xs text-gray-600"
            >{formatSize(entry.size)}</span
          >
        {/if}
      </a>
    {/each}
    {#if sortedEntries.length === 0}
      <div class="p-8 text-center text-gray-500">Empty directory</div>
    {/if}
  </div>
</div>

{#if readmeQuery.isPending && readmePath != null}
  <div
    class="mt-4 bg-gray-900 border border-gray-800 rounded-lg p-8 text-center text-gray-400"
  >
    Loading README...
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
