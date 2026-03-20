<script lang="ts">
import { useQueryClient } from "@tanstack/svelte-query";
import { print } from "graphql";
import {
    AddPublicKeyDocument,
    RemovePublicKeyDocument,
    UpdateFullnameDocument,
    UserSettingsDocument,
} from "$lib/graphql/generated";
import { graphqlMutation, graphqlQuery } from "$lib/graphql/query";

const queryClient = useQueryClient();

const settings = graphqlQuery(() => ({
    document: UserSettingsDocument,
}));

let fullname = $state("");
let fullnameLoaded = $state(false);
let fullnameSaving = $state(false);
let fullnameSuccess = $state(false);
let fullnameError = $state<string | null>(null);

let newFingerprint = $state("");
let addKeyError = $state<string | null>(null);
let removeKeyError = $state<string | null>(null);

$effect(() => {
    if (settings.data && !fullnameLoaded) {
        fullname = settings.data.me.fullname ?? "";
        fullnameLoaded = true;
    }
});

const updateFullname = graphqlMutation(UpdateFullnameDocument, {
    onSuccess: () => {
        fullnameSaving = false;
        fullnameSuccess = true;
        fullnameError = null;
        setTimeout(() => (fullnameSuccess = false), 2000);
        queryClient.invalidateQueries({
            queryKey: [print(UserSettingsDocument)],
        });
    },
    onError: (e) => {
        fullnameSaving = false;
        fullnameError = e.message;
    },
});

const addPublicKey = graphqlMutation(AddPublicKeyDocument, {
    onSuccess: () => {
        newFingerprint = "";
        addKeyError = null;
        queryClient.invalidateQueries({
            queryKey: [print(UserSettingsDocument)],
        });
    },
    onError: (e) => {
        addKeyError = e.message;
    },
});

const removePublicKey = graphqlMutation(RemovePublicKeyDocument, {
    onSuccess: () => {
        removeKeyError = null;
        queryClient.invalidateQueries({
            queryKey: [print(UserSettingsDocument)],
        });
    },
    onError: (e) => {
        removeKeyError = e.message;
    },
});

function saveFullname() {
    fullnameSaving = true;
    fullnameSuccess = false;
    fullnameError = null;
    updateFullname.mutate({ fullname: fullname.trim() });
}

function addKey() {
    const fp = newFingerprint.trim();
    if (!fp) return;
    addKeyError = null;
    addPublicKey.mutate({ fingerprint: fp });
}

function removeKey(fingerprint: string) {
    removeKeyError = null;
    removePublicKey.mutate({ fingerprint });
}
</script>

<div class="p-6 max-w-2xl mx-auto">
    <h1 class="text-2xl font-bold text-white mb-8">Settings</h1>

    {#if settings.isPending}
        <p class="text-gray-400">Loading...</p>
    {:else if settings.error}
        <div
            class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300"
        >
            {settings.error.message}
        </div>
    {:else}
        <!-- Profile section -->
        <section class="mb-10">
            <h2 class="text-lg font-medium text-white mb-4">Profile</h2>
            <div class="bg-gray-900 border border-gray-800 rounded-lg p-5">
                <div class="mb-4">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-1"
                        for="username"
                    >
                        Username
                    </label>
                    <p id="username" class="text-white">
                        {settings.data.me.username}
                    </p>
                </div>

                <div class="mb-4">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-1"
                        for="email"
                    >
                        Email
                    </label>
                    <p id="email" class="text-white">
                        {settings.data.me.email}
                    </p>
                </div>

                <form
                    onsubmit={(e) => {
                        e.preventDefault();
                        saveFullname();
                    }}
                >
                    <label
                        class="block text-sm font-medium text-gray-400 mb-1"
                        for="fullname"
                    >
                        Full name
                    </label>
                    <div class="flex gap-3">
                        <input
                            id="fullname"
                            type="text"
                            bind:value={fullname}
                            placeholder="Your full name"
                            class="flex-1 px-3 py-2 bg-gray-800 border border-gray-700 rounded text-white placeholder-gray-500 focus:outline-none focus:border-indigo-500"
                        />
                        <button
                            type="submit"
                            disabled={fullnameSaving}
                            class="px-4 py-2 bg-indigo-600 hover:bg-indigo-500 text-white rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            {fullnameSaving ? "Saving..." : "Save"}
                        </button>
                    </div>
                    {#if fullnameSuccess}
                        <p class="mt-2 text-sm text-green-400">Saved.</p>
                    {/if}
                    {#if fullnameError}
                        <p class="mt-2 text-sm text-red-400">
                            {fullnameError}
                        </p>
                    {/if}
                </form>
            </div>
        </section>

        <!-- Public keys section -->
        <section>
            <h2 class="text-lg font-medium text-white mb-4">
                SSH Public Keys
            </h2>
            <div class="bg-gray-900 border border-gray-800 rounded-lg p-5">
                {#if settings.data.me.publicKeys.length === 0}
                    <p class="text-gray-400 mb-4">No public keys registered.</p>
                {:else}
                    <ul class="space-y-3 mb-4">
                        {#each settings.data.me.publicKeys as fingerprint}
                            <li
                                class="flex items-center justify-between bg-gray-800 border border-gray-700 rounded px-3 py-2"
                            >
                                <code class="text-sm text-gray-300 truncate">
                                    {fingerprint}
                                </code>
                                <button
                                    onclick={() => removeKey(fingerprint)}
                                    disabled={removePublicKey.isPending}
                                    class="ml-3 text-sm text-red-400 hover:text-red-300 transition-colors disabled:opacity-50 shrink-0"
                                >
                                    Remove
                                </button>
                            </li>
                        {/each}
                    </ul>
                {/if}

                {#if removeKeyError}
                    <p class="mb-3 text-sm text-red-400">{removeKeyError}</p>
                {/if}

                <form
                    onsubmit={(e) => {
                        e.preventDefault();
                        addKey();
                    }}
                >
                    <label
                        class="block text-sm font-medium text-gray-400 mb-1"
                        for="new-fingerprint"
                    >
                        Add a public key fingerprint
                    </label>
                    <div class="flex gap-3">
                        <input
                            id="new-fingerprint"
                            type="text"
                            bind:value={newFingerprint}
                            placeholder="SHA256:..."
                            class="flex-1 px-3 py-2 bg-gray-800 border border-gray-700 rounded text-white placeholder-gray-500 focus:outline-none focus:border-indigo-500 font-mono text-sm"
                        />
                        <button
                            type="submit"
                            disabled={addPublicKey.isPending ||
                                !newFingerprint.trim()}
                            class="px-4 py-2 bg-indigo-600 hover:bg-indigo-500 text-white rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            {addPublicKey.isPending ? "Adding..." : "Add"}
                        </button>
                    </div>
                    {#if addKeyError}
                        <p class="mt-2 text-sm text-red-400">{addKeyError}</p>
                    {/if}
                </form>
            </div>
        </section>
    {/if}
</div>
