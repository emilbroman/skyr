<script lang="ts">
let { data } = $props();
</script>

<svelte:head>
    <title>Standard Library Reference – Skyr Docs</title>
</svelte:head>

<article class="prose max-w-none">
    <h1>Standard Library Reference</h1>

    <ul>
        {#each data.modules as mod}
            <li>
                <a href="/~docs/scl/stdlib-ref/{mod.slug}/"><code>{mod.name}</code></a>
            </li>
        {/each}
    </ul>
</article>
