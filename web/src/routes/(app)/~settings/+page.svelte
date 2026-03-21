<script lang="ts">
import { useQueryClient } from "@tanstack/svelte-query";
import { print } from "graphql";
import {
    AddPublicKeyDocument,
    RemovePublicKeyDocument,
    UpdateFullnameDocument,
    UserSettingsDocument,
} from "$lib/graphql/generated";
import Spinner from "$lib/components/Spinner.svelte";
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
    <h1 class="font-bold text-gray-900 mb-8">Settings</h1>

    {#if settings.isPending}
        <Spinner />
    {:else if settings.error}
        <div
            class="p-4 bg-red-50 border border-red-200 rounded text-red-600"
        >
            {settings.error.message}
        </div>
    {:else}
        <!-- Profile section -->
        <section class="mb-10">
            <h2 class="font-medium text-gray-900 mb-4">Profile</h2>
            <div class="bg-white border border-gray-200 rounded-lg p-5">
                <div class="mb-4">
                    <label
                        class="block font-medium text-gray-500 mb-1"
                        for="username"
                    >
                        Username
                    </label>
                    <p id="username" class="text-gray-900">
                        {settings.data.me.username}
                    </p>
                </div>

                <div class="mb-4">
                    <label
                        class="block font-medium text-gray-500 mb-1"
                        for="email"
                    >
                        Email
                    </label>
                    <p id="email" class="text-gray-900">
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
                        class="block font-medium text-gray-500 mb-1"
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
                            class="flex-1 px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500"
                        />
                        <button
                            type="submit"
                            disabled={fullnameSaving}
                            class="px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            {fullnameSaving ? "Saving..." : "Save"}
                        </button>
                    </div>
                    {#if fullnameSuccess}
                        <p class="mt-2 text-green-700">Saved.</p>
                    {/if}
                    {#if fullnameError}
                        <p class="mt-2 text-red-600">
                            {fullnameError}
                        </p>
                    {/if}
                </form>
            </div>
        </section>

        <!-- Public keys section -->
        <section>
            <h2 class="font-medium text-gray-900 mb-4">
                SSH Public Keys
            </h2>
            <div class="bg-white border border-gray-200 rounded-lg p-5">
                {#if settings.data.me.publicKeys.length === 0}
                    <p class="text-gray-500 mb-4">No public keys registered.</p>
                {:else}
                    <ul class="space-y-3 mb-4">
                        {#each settings.data.me.publicKeys as fingerprint}
                            <li
                                class="flex items-center justify-between bg-gray-100 border border-gray-300 rounded px-3 py-2"
                            >
                                <code class="text-gray-600 truncate">
                                    {fingerprint}
                                </code>
                                <button
                                    onclick={() => removeKey(fingerprint)}
                                    disabled={removePublicKey.isPending}
                                    class="ml-3 text-red-600 hover:text-red-500 transition-colors disabled:opacity-50 shrink-0"
                                >
                                    Remove
                                </button>
                            </li>
                        {/each}
                    </ul>
                {/if}

                {#if removeKeyError}
                    <p class="mb-3 text-red-600">{removeKeyError}</p>
                {/if}

                <form
                    onsubmit={(e) => {
                        e.preventDefault();
                        addKey();
                    }}
                >
                    <label
                        class="block font-medium text-gray-500 mb-1"
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
                            class="flex-1 px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500 font-mono text-xs"
                        />
                        <button
                            type="submit"
                            disabled={addPublicKey.isPending ||
                                !newFingerprint.trim()}
                            class="px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            {addPublicKey.isPending ? "Adding..." : "Add"}
                        </button>
                    </div>
                    {#if addKeyError}
                        <p class="mt-2 text-red-600">{addKeyError}</p>
                    {/if}
                </form>
            </div>
        </section>
    {/if}
</div>
