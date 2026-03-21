<script lang="ts">
import { onMount } from "svelte";
import { goto } from "$app/navigation";
import { clearAuth, isAuthenticated, user } from "$lib/stores/auth";

let { children } = $props();

onMount(() => {
    return isAuthenticated.subscribe((authed) => {
        if (!authed) {
            goto("/~signin");
        }
    });
});

function signOut() {
    clearAuth();
    goto("/~signin");
}
</script>

{#if $isAuthenticated}
  <div class="min-h-screen bg-gray-100 flex flex-col">
    <!-- Header -->
    <header
      class="h-14 bg-white border-b border-gray-200 flex items-center justify-between px-4 shrink-0"
    >
      <a href="/" class="font-bold text-gray-900">Skyr</a>

      <div class="flex items-center gap-4">
        <span class="text-gray-500">{$user?.username ?? ""}</span>
        <a
          href="/~settings"
          class="text-gray-500 hover:text-gray-800 transition-colors"
        >
          Settings
        </a>
        <button
          onclick={signOut}
          class="text-gray-500 hover:text-gray-800 transition-colors"
        >
          Sign Out
        </button>
      </div>
    </header>

    <!-- Main content -->
    <main class="flex-1 min-w-0">
      {@render children()}
    </main>
  </div>
{/if}
