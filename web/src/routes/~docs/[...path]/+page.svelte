<script lang="ts">
let { data } = $props();
</script>

<svelte:head>
    <title>{data.title} – Skyr Docs</title>
</svelte:head>

<article class="prose max-w-none">
    {@html data.html}
</article>
