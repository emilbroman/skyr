<script lang="ts">
	import { query } from '$lib/graphql/client';
	import { CommitRootTreeDocument } from '$lib/graphql/generated';
	import DirectoryView from './DirectoryView.svelte';

	type TreeEntry =
		| { __typename: 'Tree'; hash: string; name?: string | null }
		| { __typename: 'Blob'; hash: string; name?: string | null; size: number };

	type Props = {
		orgName: string;
		repoName: string;
		commitHash: string;
	};

	let { orgName, repoName, commitHash }: Props = $props();

	let entries = $state<TreeEntry[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	$effect(() => {
		loading = true;
		error = null;
		entries = [];

		query(CommitRootTreeDocument, { org: orgName, repo: repoName, commit: commitHash })
			.then((data) => {
				entries = data.organization.repository.commit.tree.entries;
			})
			.catch((e) => {
				error = e instanceof Error ? e.message : 'Failed to load tree';
			})
			.finally(() => {
				loading = false;
			});
	});
</script>

{#if loading}
	<div class="bg-gray-900 border border-gray-800 rounded-lg p-8 text-center text-gray-400">
		Loading...
	</div>
{:else if error}
	<div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">{error}</div>
{:else}
	<DirectoryView
		{orgName}
		{repoName}
		{commitHash}
		{entries}
	/>
{/if}
