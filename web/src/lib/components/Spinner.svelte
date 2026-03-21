<script lang="ts">
let { size = 20 }: { size?: number } = $props();
</script>

<div class="flex items-center justify-center py-12">
    <svg
        class="spinner"
        width={size}
        height={size}
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        stroke-width="2.5"
        stroke-linecap="round"
    >
        <path d="M8 1.5a6.5 6.5 0 1 1-6.5 6.5" />
    </svg>
</div>

<style>
    .spinner {
        animation: spin 4s linear infinite;
        color: var(--color-gray-400);
    }

    @keyframes spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
    }
</style>
