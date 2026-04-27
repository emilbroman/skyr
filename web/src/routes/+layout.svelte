<script lang="ts">
import "../app.css";
import { QueryClient, QueryClientProvider } from "@tanstack/svelte-query";
import { onDestroy, onMount } from "svelte";
import { goto } from "$app/navigation";
import { page } from "$app/state";
import {
    clearAuth,
    isAuthenticated,
    startExpiryWatch,
    stopExpiryWatch,
    user,
} from "$lib/stores/auth";
import { ChevronDown, LogIn, LogOut, Settings } from "lucide-svelte";
import SkyrLogo from "$lib/components/SkyrLogo.svelte";

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
      class="h-14 bg-white border-b border-gray-200 flex items-center justify-between px-4 shrink-0 sticky top-0 z-40"
    >
      <div class="flex items-center gap-4">
        <a href="/" class="flex items-center gap-1.5 font-bold text-gray-900">
          <SkyrLogo class="w-5 h-5" />
          Skyr
        </a>
        <a
          href="/~docs/"
          class="relative {page.url.pathname.startsWith('/~docs/') ? 'text-gray-900 font-medium after:absolute after:left-0 after:right-0 after:mt-1 after:top-full after:h-0.5 after:bg-orange-500' : 'text-gray-500'} hover:text-gray-800 transition-colors"
        >
          Docs
        </a>
        <a
          href="/~playground"
          class="relative {page.url.pathname === '/~playground' ? 'text-gray-900 font-medium after:absolute after:left-0 after:right-0 after:mt-1 after:top-full after:h-0.5 after:bg-orange-500' : 'text-gray-500'} hover:text-gray-800 transition-colors"
        >
          Playground
        </a>
      </div>

      {#if $isAuthenticated}
        <div class="relative">
          <button
            onclick={toggleDropdown}
            class="flex items-center gap-1 text-gray-500 hover:text-gray-800 transition-colors cursor-pointer"
          >
            {#if $user?.fullname}
              <div class="flex flex-col items-end leading-tight">
                <span class="text-gray-800 font-semibold text-xs">{$user.fullname}</span>
                <span class="text-gray-400 text-xs">@{$user.username}</span>
              </div>
            {:else}
              <span class="text-gray-800 font-semibold">@{$user?.username ?? ""}</span>
            {/if}
            <ChevronDown class="w-4 h-4" />
          </button>

          {#if dropdownOpen}
            <button
              class="fixed inset-0 z-10 cursor-default"
              onclick={closeDropdown}
              tabindex="-1"
              aria-label="Close menu"
            ></button>
            <div
              class="absolute right-0 mt-2 w-44 bg-white rounded-md shadow-lg border border-gray-200 py-1 z-20"
            >
              <a
                href="/~settings"
                onclick={closeDropdown}
                class="flex items-center gap-2 px-3 py-1.5 text-xs text-gray-700 hover:bg-gray-100"
              >
                <Settings class="w-3.5 h-3.5" />
                Settings
              </a>
              <button
                onclick={signOut}
                class="flex items-center gap-2 w-full text-left px-3 py-1.5 text-xs text-gray-700 hover:bg-gray-100 cursor-pointer"
              >
                <LogOut class="w-3.5 h-3.5" />
                Sign Out
              </button>
            </div>
          {/if}
        </div>
      {:else}
        <a
          href="/~signin"
          class="flex items-center gap-1 text-gray-500 hover:text-gray-800 transition-colors"
        >
          <LogIn class="w-4 h-4" />
          Sign In
        </a>
      {/if}
    </header>

    {@render children()}
  </div>
</QueryClientProvider>
