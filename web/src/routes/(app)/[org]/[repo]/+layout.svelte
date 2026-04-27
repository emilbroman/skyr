<script lang="ts">
import { page } from "$app/stores";
import { orgHref, repoHref } from "$lib/paths";

let { children } = $props();

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
</script>

<div class="max-w-6xl mx-auto px-6 py-6">
    <h1 class="text-lg text-gray-500 mb-4 flex items-center gap-1">
        <a href={orgHref(orgName)} class="hover:text-gray-900 transition-colors">{orgName}</a>
        <span class="text-gray-300">/</span>
        <a href={repoHref(orgName, repoName)} class="text-gray-900 font-semibold hover:text-blue-600 transition-colors">{repoName}</a>
    </h1>

    {@render children()}
</div>
