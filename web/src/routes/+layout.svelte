<script lang="ts">
import "../app.css";
import { QueryClient, QueryClientProvider } from "@tanstack/svelte-query";
import { onDestroy, onMount } from "svelte";
import { goto } from "$app/navigation";
import {
    clearAuth,
    isAuthenticated,
    startExpiryWatch,
    stopExpiryWatch,
    user,
} from "$lib/stores/auth";

let { children } = $props();

const queryClient = new QueryClient({
    defaultOptions: {
        queries: {
            staleTime: 30_000,
            refetchOnWindowFocus: false,
        },
    },
});

onMount(() => {
    startExpiryWatch();
});

onDestroy(() => {
    stopExpiryWatch();
});

let dropdownOpen = $state(false);

function signOut() {
    clearAuth();
    goto("/~signin");
}

function toggleDropdown() {
    dropdownOpen = !dropdownOpen;
}

function closeDropdown() {
    dropdownOpen = false;
}
</script>

<QueryClientProvider client={queryClient}>
  <div class="min-h-screen bg-gray-100 flex flex-col">
    <header
      class="h-14 bg-white border-b border-gray-200 flex items-center justify-between px-4 shrink-0"
    >
      <div class="flex items-center gap-4">
        <a href="/" class="font-bold text-gray-900">Skyr</a>
        <a
          href="/~docs/"
          class="text-gray-500 hover:text-gray-800 transition-colors"
        >
          Docs
        </a>
      </div>

      {#if $isAuthenticated}
        <div class="relative">
          <button
            onclick={toggleDropdown}
            class="flex items-center gap-1 text-gray-500 hover:text-gray-800 transition-colors cursor-pointer"
          >
            {$user?.username ?? ""}
            <svg
              class="w-4 h-4"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M19 9l-7 7-7-7"
              />
            </svg>
          </button>

          {#if dropdownOpen}
            <button
              class="fixed inset-0 z-10 cursor-default"
              onclick={closeDropdown}
              tabindex="-1"
              aria-label="Close menu"
            ></button>
            <div
              class="absolute right-0 mt-2 w-48 bg-white rounded-md shadow-lg border border-gray-200 py-1 z-20"
            >
              <a
                href="/~settings"
                onclick={closeDropdown}
                class="flex items-center gap-2 px-4 py-2 text-sm text-gray-700 hover:bg-gray-100"
              >
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    stroke-width="2"
                    d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"
                  />
                  <path
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    stroke-width="2"
                    d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
                  />
                </svg>
                Settings
              </a>
              <button
                onclick={signOut}
                class="flex items-center gap-2 w-full text-left px-4 py-2 text-sm text-gray-700 hover:bg-gray-100 cursor-pointer"
              >
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    stroke-width="2"
                    d="M17 16l4-4m0 0l-4-4m4 4H7m6 4v1a3 3 0 01-3 3H6a3 3 0 01-3-3V7a3 3 0 013-3h4a3 3 0 013 3v1"
                  />
                </svg>
                Sign Out
              </button>
            </div>
          {/if}
        </div>
      {:else}
        <a
          href="/~signin"
          class="text-gray-500 hover:text-gray-800 transition-colors"
        >
          Sign In
        </a>
      {/if}
    </header>

    {@render children()}
  </div>
</QueryClientProvider>
