<script lang="ts">
import { onMount } from "svelte";
import { goto } from "$app/navigation";
import { isAuthenticated } from "$lib/stores/auth";

let { children } = $props();

onMount(() => {
    return isAuthenticated.subscribe((authed) => {
        if (!authed) {
            goto("/~signin");
        }
    });
});
</script>

{#if $isAuthenticated}
  <main class="flex-1 min-w-0">
    {@render children()}
  </main>
{/if}
