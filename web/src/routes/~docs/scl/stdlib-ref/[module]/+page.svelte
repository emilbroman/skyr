<script lang="ts">
import type { RecordType, Type, FnType } from "$lib/stdlib";

let { data } = $props();

const knownTypes = $derived(new Set(Object.keys(data.typeExports.fields)));

function esc(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function typeLink(name: string): string {
    return `<a href="#type-${name.toLowerCase()}" class="text-orange-600 hover:underline">${esc(name)}</a>`;
}

/** Compact inline HTML representation of a type. */
function formatType(ty: Type, typeParamIds?: number[]): string {
    if (ty.name) {
        if (knownTypes.has(ty.name)) return typeLink(ty.name);
        return esc(ty.name);
    }
    return formatKind(ty.kind, typeParamIds);
}

function typeParamName(index: number): string {
    const letter = String.fromCharCode(65 + (index % 26));
    const suffix = Math.floor(index / 26);
    return suffix === 0 ? letter : `${letter}${suffix}`;
}

function formatKind(kind: Type["kind"], typeParamIds?: number[]): string {
    if (typeof kind === "string") return esc(kind);
    if ("Optional" in kind) return `${formatType(kind.Optional, typeParamIds)}?`;
    if ("List" in kind) return `[${formatType(kind.List, typeParamIds)}]`;
    if ("Dict" in kind)
        return `#{${formatType(kind.Dict.key, typeParamIds)}: ${formatType(kind.Dict.value, typeParamIds)}}`;
    if ("Var" in kind) {
        if (typeParamIds) {
            const idx = typeParamIds.indexOf(kind.Var);
            if (idx >= 0) return esc(typeParamName(idx));
        }
        return esc(`T${kind.Var}`);
    }
    if ("IsoRec" in kind) return formatType(kind.IsoRec[1], typeParamIds);
    if ("Exception" in kind) return esc("!");
    if ("Fn" in kind) return formatFnSig(kind.Fn, typeParamIds);
    if ("Record" in kind) return formatRecordInline(kind.Record, typeParamIds);
    return esc("unknown");
}

function formatFnSig(fn: FnType, outerTypeParamIds?: number[]): string {
    const allParamIds = [...(outerTypeParamIds ?? []), ...fn.type_params.map(([id]) => id)];
    const typeParams =
        fn.type_params.length > 0
            ? `&lt;${fn.type_params.map(([_id], i) => esc(typeParamName((outerTypeParamIds?.length ?? 0) + i))).join(", ")}&gt;`
            : "";
    const params = fn.params.map((p) => formatType(p, allParamIds)).join(", ");
    const ret = formatType(fn.ret, allParamIds);
    return `fn${typeParams}(${params}) ${ret}`;
}

function formatRecordInline(record: RecordType, typeParamIds?: number[]): string {
    const entries = Object.entries(record.fields);
    if (entries.length === 0) return "{}";
    const fields = entries
        .map(([name, ty]) => `${esc(name)}: ${formatType(ty, typeParamIds)}`)
        .join(", ");
    return `{ ${fields} }`;
}

/** Peel through Optional, List, IsoRec to reach the "interesting" inner type. */
function unwrapShallow(ty: Type): Type {
    const kind = ty.kind;
    if (typeof kind !== "object") return ty;
    if ("IsoRec" in kind) return unwrapShallow(kind.IsoRec[1]);
    if ("Optional" in kind) return unwrapShallow(kind.Optional);
    if ("List" in kind) return unwrapShallow(kind.List);
    return ty;
}

/** Does this type warrant structural (block-level) expansion? */
function needsExpansion(ty: Type): boolean {
    const inner = unwrapShallow(ty);
    if (inner.name && knownTypes.has(inner.name)) return false;
    const kind = inner.kind;
    if (typeof kind !== "object") return false;
    if ("Record" in kind) return Object.keys(kind.Record.fields).length > 0;
    if ("Fn" in kind) return true;
    return false;
}

/** Extract anonymous record (not behind a known type name). */
function getAnonymousRecord(ty: Type): RecordType | null {
    const inner = unwrapShallow(ty);
    if (inner.name && knownTypes.has(inner.name)) return null;
    const kind = inner.kind;
    if (typeof kind === "object" && "Record" in kind && Object.keys(kind.Record.fields).length > 0)
        return kind.Record;
    return null;
}

/** Extract function type (unwrapping wrappers). */
function getFnType(ty: Type): FnType | null {
    const inner = unwrapShallow(ty);
    const kind = inner.kind;
    if (typeof kind === "object" && "Fn" in kind) return kind.Fn;
    return null;
}

/** Heading size classes by depth: 0 = top-level member sections, increasing = deeper nesting. */
const headingSizes = [
    "text-sm font-semibold text-gray-500",
    "text-xs font-semibold text-gray-500",
    "text-xs font-medium text-gray-400",
];

function headingClass(depth: number): string {
    return headingSizes[Math.min(depth, headingSizes.length - 1)];
}

const sortedTypeExports = $derived(
    Object.entries(data.typeExports.fields).sort(([a], [b]) => a.localeCompare(b)),
);
const sortedValueExports = $derived(
    Object.entries(data.valueExports.fields).sort(([a], [b]) => a.localeCompare(b)),
);
const hasTypeExports = $derived(sortedTypeExports.length > 0);
const hasValueExports = $derived(sortedValueExports.length > 0);
</script>

<svelte:head>
    <title>{data.name} – Skyr Docs</title>
</svelte:head>

<article>
    <h1 class="text-2xl font-bold mb-6"><code>{data.name}</code></h1>

    {#if hasTypeExports}
        <h2 class="text-xs font-semibold text-gray-400 uppercase tracking-wide mb-2">Types</h2>
        <div class="divide-y divide-gray-200 mb-8">
            {#each sortedTypeExports as [typeName, typeType]}
                {@const doc = data.typeExports.doc_comments[typeName]}
                <section class="py-5" id={"type-" + typeName.toLowerCase()}>
                    <h3 class="text-lg font-semibold mb-1"><code>{typeName}</code></h3>
                    {#if doc}
                        <div class="prose prose-sm max-w-none mt-2">{@html doc}</div>
                    {/if}
                    {@render typeDetail(typeType, [], 0)}
                </section>
            {/each}
        </div>
    {/if}

    {#if hasValueExports}
        {#if hasTypeExports}
            <h2 class="text-xs font-semibold text-gray-400 uppercase tracking-wide mb-2">
                Functions & Values
            </h2>
        {/if}
        <div class="divide-y divide-gray-200">
            {#each sortedValueExports as [fieldName, fieldType]}
                {@const doc = data.valueExports.doc_comments[fieldName]}
                {@const fnTy = getFnType(fieldType)}
                {@const typeParamIds = fnTy ? fnTy.type_params.map(([id]) => id) : []}

                <section class="py-5" id={fieldName.toLowerCase()}>
                    <h3 class="text-lg font-semibold mb-1"><code>{fieldName}</code></h3>

                    {#if fnTy}
                        <pre
                            class="bg-gray-50 border border-gray-200 rounded px-3 py-2 text-sm overflow-x-auto my-2"
                            ><code>{@html formatFnSig(fnTy)}</code></pre>
                    {:else}
                        <p class="text-sm text-gray-600 my-1">
                            <strong>Type:</strong>
                            <code class="text-xs">{@html formatType(fieldType)}</code>
                        </p>
                    {/if}

                    {#if doc}
                        <div class="prose prose-sm max-w-none mt-2">{@html doc}</div>
                    {/if}

                    {#if fnTy}
                        {@render fnDetail(fnTy, typeParamIds, 0)}
                    {/if}
                </section>
            {/each}
        </div>
    {/if}
</article>

<!-- Expand a function's params and returns.
     Simple record params → field table.
     Complex fields within those → pushed to subsections at depth+1. -->
{#snippet fnDetail(fn: FnType, outerTypeParamIds: number[], depth: number)}
    {@const typeParamIds = [...outerTypeParamIds, ...fn.type_params.map(([id]) => id)]}
    {@const expandableParamCount = fn.params.filter((p) => needsExpansion(p)).length}

    {#each fn.params as param, i}
        {#if needsExpansion(param)}
            <div class="{headingClass(depth)} mt-3 mb-1">
                {expandableParamCount === 1 ? "Parameters" : `Parameter ${i + 1}`}
            </div>
            {@render typeDetail(param, typeParamIds, depth)}
        {/if}
    {/each}

    {#if needsExpansion(fn.ret)}
        <div class="{headingClass(depth)} mt-3 mb-1">Returns</div>
        {@render typeDetail(fn.ret, typeParamIds, depth)}
    {/if}
{/snippet}

<!-- Dispatch: anonymous record → recordDetail, function → signature + fnDetail. -->
{#snippet typeDetail(ty: Type, typeParamIds: number[], depth: number)}
    {@const record = getAnonymousRecord(ty)}
    {@const fnTy = !record ? getFnType(ty) : null}

    {#if record}
        {@render recordDetail(record, typeParamIds, depth)}
    {:else if fnTy}
        <pre
            class="bg-gray-50 border border-gray-200 rounded px-3 py-2 text-sm overflow-x-auto my-2"
            ><code>{@html formatFnSig(fnTy)}</code></pre>
        {@render fnDetail(fnTy, typeParamIds, depth + 1)}
    {/if}
{/snippet}

<!-- Render a record's fields. Simple fields go in a table.
     Complex fields (functions, nested anonymous records) are listed
     in the table too, but then get their own headed subsection after. -->
{#snippet recordDetail(record: RecordType, typeParamIds: number[], depth: number)}
    {@const entries = Object.entries(record.fields)}
    {@const complexFields = entries.filter(([_, ty]) => needsExpansion(ty))}
    {@const complexNames = new Set(complexFields.map(([name]) => name))}

    <table class="w-full text-sm border-collapse">
        <thead>
            <tr class="text-left text-xs text-gray-500 border-b border-gray-200">
                <th class="py-1 pr-3 font-medium">Name</th>
                <th class="py-1 pr-3 font-medium">Type</th>
                <th class="py-1 font-medium">Description</th>
            </tr>
        </thead>
        <tbody>
            {#each entries as [name, ty]}
                <tr class="border-b border-gray-100">
                    <td class="py-1 pr-3 align-top"><code class="text-xs">{name}</code></td>
                    <td class="py-1 pr-3 align-top">
                        {#if complexNames.has(name)}
                            <span class="text-xs text-gray-400 italic">see below</span>
                        {:else}
                            <code class="text-xs">{@html formatType(ty, typeParamIds)}</code>
                        {/if}
                    </td>
                    <td class="py-1 align-top prose prose-sm max-w-none [&>p]:m-0">
                        {@html record.doc_comments[name] ?? ""}
                    </td>
                </tr>
            {/each}
        </tbody>
    </table>

    {#each complexFields as [name, ty]}
        <div class="ml-4 mt-4">
            <div class="{headingClass(depth + 1)} mb-1">
                <code>{name}</code>
            </div>
            {@render typeDetail(ty, typeParamIds, depth + 1)}
        </div>
    {/each}
{/snippet}
