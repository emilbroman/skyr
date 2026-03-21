<script lang="ts">
import { page } from "$app/stores";
import { orgHref, repoHref } from "$lib/paths";

let { children } = $props();

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
</script>

<div class="p-6">
    <nav class="text-lg font-bold text-white mb-6">
        <a href={orgHref(orgName)} class="hover:text-gray-300">{orgName}</a>
        <span class="mx-1 text-gray-500">/</span>
        <a href={repoHref(orgName, repoName)} class="hover:text-gray-300">{repoName}</a>
    </nav>

    {@render children()}
</div>
