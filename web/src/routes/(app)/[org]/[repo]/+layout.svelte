<script lang="ts">
import { page } from "$app/stores";
import { orgHref, repoHref } from "$lib/paths";

let { children } = $props();

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
</script>

<div class="p-6">
    <nav class="text-xl font-bold text-gray-900 mb-3">
        <a href={orgHref(orgName)} class="hover:text-gray-700">{orgName}</a>
        <span class="mx-1 text-gray-400">/</span>
        <a href={repoHref(orgName, repoName)} class="hover:text-gray-700">{repoName}</a>
    </nav>

    {@render children()}
</div>
