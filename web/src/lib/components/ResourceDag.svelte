<script lang="ts">
import { ResourceMarker } from "$lib/graphql/generated";
import { resourceHref } from "$lib/paths";
import ResourceCardContent from "./ResourceCardContent.svelte";

type ResourceInput = {
    type: string;
    name: string;
    markers: ResourceMarker[];
    owner?: { id: string } | null;
    dependencies: { type: string; name: string }[];
};

let {
    resources,
    org,
    repo,
    env,
}: {
    resources: ResourceInput[];
    org: string;
    repo: string;
    env: string;
} = $props();

const NODE_W = 200;
const NODE_H = 96;
const LAYER_GAP = 120;
const NODE_GAP = 24;

type DagNode = {
    id: string;
    type: string;
    name: string;
    markers: ResourceMarker[];
    isShadow: boolean;
    layer: number;
    x: number;
    y: number;
    href?: string;
};

type DagEdge = {
    from: string;
    to: string;
};

let dag = $derived.by(() => {
    const nodeMap = new Map<string, DagNode>();
    const edges: DagEdge[] = [];

    for (const r of resources) {
        const id = `${r.type}:${r.name}`;
        nodeMap.set(id, {
            id,
            type: r.type,
            name: r.name,
            markers: r.markers,
            isShadow: false,

            layer: 0,
            x: 0,
            y: 0,
            href: resourceHref(org, repo, env, id),
        });
    }

    for (const r of resources) {
        const id = `${r.type}:${r.name}`;
        for (const dep of r.dependencies) {
            const depId = `${dep.type}:${dep.name}`;
            if (!nodeMap.has(depId)) {
                nodeMap.set(depId, {
                    id: depId,
                    type: dep.type,
                    name: dep.name,
                    markers: [],
                    isShadow: true,

                    layer: 0,
                    x: 0,
                    y: 0,
                });
            }
            edges.push({ from: depId, to: id });
        }
    }

    // Build dependency lookup (node -> its dependencies)
    const depsOf = new Map<string, string[]>();
    for (const e of edges) {
        if (!depsOf.has(e.to)) depsOf.set(e.to, []);
        depsOf.get(e.to)!.push(e.from);
    }

    // Assign layers via longest-path from roots
    const layerCache = new Map<string, number>();
    const visiting = new Set<string>();

    function computeLayer(nodeId: string): number {
        if (layerCache.has(nodeId)) return layerCache.get(nodeId)!;
        if (visiting.has(nodeId)) return 0;
        visiting.add(nodeId);
        const deps = depsOf.get(nodeId) ?? [];
        const layer = deps.length === 0 ? 0 : Math.max(...deps.map(computeLayer)) + 1;
        visiting.delete(nodeId);
        layerCache.set(nodeId, layer);
        return layer;
    }

    const nodes = [...nodeMap.values()];
    for (const node of nodes) {
        node.layer = computeLayer(node.id);
    }

    // Group by layer
    const layers = new Map<number, DagNode[]>();
    for (const node of nodes) {
        if (!layers.has(node.layer)) layers.set(node.layer, []);
        layers.get(node.layer)!.push(node);
    }

    const maxLayer = Math.max(...nodes.map((n) => n.layer), 0);

    // Order within layers: layer 0 alphabetically, rest by barycenter
    const orderMap = new Map<string, number>();
    const layer0 = layers.get(0) ?? [];
    layer0.sort((a, b) => a.id.localeCompare(b.id));
    layer0.forEach((n, i) => {
        orderMap.set(n.id, i);
    });

    for (let l = 1; l <= maxLayer; l++) {
        const layerNodes = layers.get(l) ?? [];
        const bary = new Map<string, number>();
        for (const node of layerNodes) {
            const deps = depsOf.get(node.id) ?? [];
            if (deps.length > 0) {
                bary.set(
                    node.id,
                    deps.reduce((sum, d) => sum + (orderMap.get(d) ?? 0), 0) / deps.length,
                );
            } else {
                bary.set(node.id, 0);
            }
        }
        layerNodes.sort((a, b) => (bary.get(a.id) ?? 0) - (bary.get(b.id) ?? 0));
        layerNodes.forEach((n, i) => {
            orderMap.set(n.id, i);
        });
    }

    // Build reverse lookup (node -> its dependents) for barycenter positioning
    const dependentsOf = new Map<string, string[]>();
    for (const e of edges) {
        if (!dependentsOf.has(e.from)) dependentsOf.set(e.from, []);
        dependentsOf.get(e.from)!.push(e.to);
    }

    // Position nodes using barycenter Y from neighbors.
    // Layer 0: evenly spaced. Subsequent layers: each node is placed at
    // the average Y of its dependencies, then overlaps are resolved.
    const layer0Nodes = layers.get(0) ?? [];
    layer0Nodes.forEach((node, i) => {
        node.x = 0;
        node.y = i * (NODE_H + NODE_GAP);
    });

    for (let l = 1; l <= maxLayer; l++) {
        const layerNodes = layers.get(l) ?? [];
        // Place each node at barycenter of its dependencies
        for (const node of layerNodes) {
            node.x = l * (NODE_W + LAYER_GAP);
            const deps = depsOf.get(node.id) ?? [];
            if (deps.length > 0) {
                const avgY =
                    deps.reduce((sum, d) => sum + (nodeMap.get(d)!.y + NODE_H / 2), 0) /
                    deps.length;
                node.y = avgY - NODE_H / 2;
            } else {
                node.y = 0;
            }
        }
        // Resolve overlaps: sort by current Y, then push apart
        layerNodes.sort((a, b) => a.y - b.y);
        for (let i = 1; i < layerNodes.length; i++) {
            const minY = layerNodes[i - 1].y + NODE_H + NODE_GAP;
            if (layerNodes[i].y < minY) {
                layerNodes[i].y = minY;
            }
        }
    }

    // Second pass: refine layer 0 positions based on where their dependents
    // ended up, so source nodes also gravitate toward their edges.
    for (const node of layer0Nodes) {
        const deps = dependentsOf.get(node.id) ?? [];
        if (deps.length > 0) {
            const avgY =
                deps.reduce((sum, d) => sum + (nodeMap.get(d)!.y + NODE_H / 2), 0) / deps.length;
            node.y = avgY - NODE_H / 2;
        }
    }
    layer0Nodes.sort((a, b) => a.y - b.y);
    for (let i = 1; i < layer0Nodes.length; i++) {
        const minY = layer0Nodes[i - 1].y + NODE_H + NODE_GAP;
        if (layer0Nodes[i].y < minY) {
            layer0Nodes[i].y = minY;
        }
    }

    // For long-span edges, compute waypoints that route around real nodes
    // at intermediate layers. Only deflect where the edge would actually
    // overlap a node; otherwise let it pass straight through.
    const waypointMap = new Map<string, { x: number; y: number }[]>();
    const MARGIN = NODE_GAP / 2;

    for (const edge of edges) {
        const fromNode = nodeMap.get(edge.from)!;
        const toNode = nodeMap.get(edge.to)!;
        const span = toNode.layer - fromNode.layer;
        if (span <= 1) continue;

        const edgeKey = `${edge.from}|${edge.to}`;
        const waypoints: { x: number; y: number }[] = [];

        for (let l = fromNode.layer + 1; l < toNode.layer; l++) {
            const layerNodes = layers.get(l) ?? [];
            // Interpolate where the edge naturally wants to be at this layer
            const t = (l - fromNode.layer) / span;
            const naturalY =
                fromNode.y + NODE_H / 2 + t * (toNode.y + NODE_H / 2 - (fromNode.y + NODE_H / 2));

            // Check if any real node at this layer overlaps the natural Y
            let blocked = false;
            for (const n of layerNodes) {
                if (naturalY >= n.y - MARGIN && naturalY <= n.y + NODE_H + MARGIN) {
                    blocked = true;
                    break;
                }
            }

            if (blocked) {
                // Find the nearest gap (above or below the blocking nodes)
                const sortedNodes = layerNodes.sort((a, b) => a.y - b.y);

                // Candidate positions: above topmost, below bottommost, or
                // between consecutive nodes
                const candidates: number[] = [];
                if (sortedNodes.length > 0) {
                    candidates.push(sortedNodes[0].y - MARGIN - NODE_H / 2);
                    candidates.push(
                        sortedNodes[sortedNodes.length - 1].y + NODE_H + MARGIN + NODE_H / 2,
                    );
                    for (let i = 0; i < sortedNodes.length - 1; i++) {
                        const gapTop = sortedNodes[i].y + NODE_H;
                        const gapBot = sortedNodes[i + 1].y;
                        if (gapBot - gapTop >= MARGIN * 2) {
                            candidates.push((gapTop + gapBot) / 2);
                        }
                    }
                }

                // Pick the candidate closest to the natural Y
                let bestY = naturalY;
                let bestDist = Infinity;
                for (const cy of candidates) {
                    const dist = Math.abs(cy - naturalY);
                    if (dist < bestDist) {
                        bestDist = dist;
                        bestY = cy;
                    }
                }

                const layerX = l * (NODE_W + LAYER_GAP);
                waypoints.push({ x: layerX, y: bestY - NODE_H / 2 });
            }
        }

        if (waypoints.length > 0) {
            waypointMap.set(edgeKey, waypoints);
        }
    }

    return { nodes, edges, nodeMap, waypointMap };
});

function edgePath(edge: DagEdge): string {
    const from = dag.nodeMap.get(edge.from);
    const to = dag.nodeMap.get(edge.to);
    if (!from || !to) return "";

    const edgeKey = `${edge.from}|${edge.to}`;
    const waypoints = dag.waypointMap.get(edgeKey);

    // Build the list of points the edge passes through
    const points: { x: number; y: number }[] = [{ x: from.x + NODE_W, y: from.y + NODE_H / 2 }];
    if (waypoints) {
        // Dummy nodes occupy a full node slot; route through the center of that slot
        for (const wp of waypoints) {
            points.push({ x: wp.x + NODE_W / 2, y: wp.y + NODE_H / 2 });
        }
    }
    points.push({ x: to.x, y: to.y + NODE_H / 2 });

    const hasWaypoints = points.length > 2;

    // Single segment: one cubic bezier
    if (!hasWaypoints) {
        const [p0, p1] = points;
        const dx = Math.abs(p1.x - p0.x) * 0.4;
        return `M ${p0.x} ${p0.y} C ${p0.x + dx} ${p0.y}, ${p1.x - dx} ${p1.y}, ${p1.x} ${p1.y}`;
    }

    // Multiple segments: use tight control points so the edge converges
    // to each waypoint's Y quickly, staying clear of intermediate nodes.
    let d = `M ${points[0].x} ${points[0].y}`;
    for (let i = 0; i < points.length - 1; i++) {
        const p0 = points[i];
        const p1 = points[i + 1];
        const segDx = Math.abs(p1.x - p0.x);
        // First segment: leave source briefly then snap to waypoint Y
        // Last segment: arrive at target Y early then coast in
        // Middle segments: stay at waypoint Y throughout
        const isFirst = i === 0;
        const isLast = i === points.length - 2;
        const outDx = segDx * (isFirst ? 0.15 : 0.4);
        const inDx = segDx * (isLast ? 0.15 : 0.4);
        d += ` C ${p0.x + outDx} ${p0.y}, ${p1.x - inDx} ${p1.y}, ${p1.x} ${p1.y}`;
    }
    return d;
}

// Pan/zoom
let svgEl: SVGSVGElement | undefined = $state();
let panX = $state(0);
let panY = $state(0);
let zoom = $state(1);
let isPanning = $state(false);
let panStartX = $state(0);
let panStartY = $state(0);
let hoveredNode = $state<string | null>(null);
let lastNodeCount = $state(0);

function fitToView() {
    if (!svgEl || dag.nodes.length === 0) return;
    const rect = svgEl.getBoundingClientRect();
    const pad = 60;

    const minX = Math.min(...dag.nodes.map((n) => n.x));
    const maxX = Math.max(...dag.nodes.map((n) => n.x + NODE_W));
    const minY = Math.min(...dag.nodes.map((n) => n.y));
    const maxY = Math.max(...dag.nodes.map((n) => n.y + NODE_H));

    const graphW = maxX - minX || 1;
    const graphH = maxY - minY || 1;

    const scaleX = (rect.width - pad * 2) / graphW;
    const scaleY = (rect.height - pad * 2) / graphH;
    zoom = Math.min(scaleX, scaleY, 1.5);

    panX = (rect.width - graphW * zoom) / 2 - minX * zoom;
    panY = (rect.height - graphH * zoom) / 2 - minY * zoom;
}

$effect(() => {
    const count = dag.nodes.length;
    if (count > 0 && svgEl && count !== lastNodeCount) {
        fitToView();
        lastNodeCount = count;
    }
});

// Attach wheel listener with { passive: false } so preventDefault() works for pinch-to-zoom
$effect(() => {
    if (!svgEl) return;
    svgEl.addEventListener("wheel", onWheel, { passive: false });
    return () => svgEl!.removeEventListener("wheel", onWheel);
});

function onMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    isPanning = true;
    panStartX = e.clientX - panX;
    panStartY = e.clientY - panY;
}

function onMouseMove(e: MouseEvent) {
    if (!isPanning) return;
    panX = e.clientX - panStartX;
    panY = e.clientY - panStartY;
}

function onMouseUp() {
    isPanning = false;
}

function zoomAt(cx: number, cy: number, factor: number) {
    const newZoom = Math.max(0.1, Math.min(3, zoom * factor));
    panX = cx - (cx - panX) * (newZoom / zoom);
    panY = cy - (cy - panY) * (newZoom / zoom);
    zoom = newZoom;
}

function onWheel(e: WheelEvent) {
    // Only handle pinch-to-zoom (reported as ctrl+wheel by browsers)
    if (!e.ctrlKey) return;
    e.preventDefault();
    const rect = svgEl!.getBoundingClientRect();
    // Use a gentle factor so pinch-to-zoom isn't too jumpy
    const factor = 1 - e.deltaY * 0.01;
    zoomAt(e.clientX - rect.left, e.clientY - rect.top, Math.max(0.85, Math.min(1.15, factor)));
}

function zoomInCenter() {
    if (!svgEl) return;
    const rect = svgEl.getBoundingClientRect();
    zoomAt(rect.width / 2, rect.height / 2, 1.25);
}

function zoomOutCenter() {
    if (!svgEl) return;
    const rect = svgEl.getBoundingClientRect();
    zoomAt(rect.width / 2, rect.height / 2, 0.8);
}

function isEdgeHighlighted(edge: DagEdge): boolean {
    return hoveredNode !== null && (edge.from === hoveredNode || edge.to === hoveredNode);
}
</script>

<svelte:window onmousemove={onMouseMove} onmouseup={onMouseUp} />

<div
  class="relative w-full rounded-lg border border-gray-200 overflow-hidden"
  style="height: calc(100vh - 240px); min-height: 400px; background-color: #f9fafb; background-image: radial-gradient(circle, rgba(0,0,0,0.06) 1px, transparent 1px); background-size: 24px 24px;"
>
  {#if dag.nodes.length === 0}
    <div class="flex items-center justify-center h-full text-gray-500">
      No resources in this environment.
    </div>
  {:else}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <svg
      bind:this={svgEl}
      class="w-full h-full"
      onmousedown={onMouseDown}
      style="cursor: {isPanning ? 'grabbing' : 'grab'};"
    >
      <defs>
        <marker
          id="arrow"
          viewBox="0 0 8 8"
          refX="8"
          refY="4"
          markerWidth="8"
          markerHeight="8"
          orient="auto"
        >
          <path
            d="M 0 1 L 7 4 L 0 7"
            fill="none"
            stroke="#9ca3af"
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </marker>
        <marker
          id="arrow-hl"
          viewBox="0 0 8 8"
          refX="8"
          refY="4"
          markerWidth="8"
          markerHeight="8"
          orient="auto"
        >
          <path
            d="M 0 1 L 7 4 L 0 7"
            fill="none"
            stroke="#3b82f6"
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </marker>
      </defs>

      <g transform="translate({panX}, {panY}) scale({zoom})">
        {#each dag.edges as edge}
          <path
            d={edgePath(edge)}
            fill="none"
            stroke={isEdgeHighlighted(edge) ? "#3b82f6" : "#d1d5db"}
            stroke-width={isEdgeHighlighted(edge) ? 2 : 1.5}
            opacity={hoveredNode && !isEdgeHighlighted(edge) ? 0.15 : 1}
            marker-end="url(#{isEdgeHighlighted(edge) ? 'arrow-hl' : 'arrow'})"
          />
        {/each}

        {#each dag.nodes as node}
          <foreignObject x={node.x} y={node.y} width={NODE_W} height={NODE_H}>
            <div xmlns="http://www.w3.org/1999/xhtml" class="w-full h-full">
              {#if node.href}
                <a
                  href={node.href}
                  class="flex flex-col w-full h-full rounded-lg border p-3 transition-all duration-150
										{node.isShadow
                    ? 'border-dashed border-amber-300/60 bg-white/60'
                    : 'border-gray-200 bg-white hover:border-blue-500'}
										{hoveredNode === node.id
                    ? node.isShadow
                      ? 'border-amber-500 shadow-sm shadow-amber-500/10'
                      : 'border-blue-500 shadow-sm shadow-blue-500/10'
                    : ''}
										{hoveredNode && hoveredNode !== node.id ? 'opacity-40' : ''}"
                  onmouseenter={() => (hoveredNode = node.id)}
                  onmouseleave={() => (hoveredNode = null)}
                >
                  <ResourceCardContent resource={node} />
                  {#if node.isShadow}
                    <div class="text-amber-500/70 mt-0.5 text-xs">
                      External
                    </div>
                  {/if}
                </a>
              {:else}
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div
                  class="flex flex-col w-full h-full rounded-lg border border-dashed p-3 transition-all duration-150
										border-amber-300/60 bg-white/60
										{hoveredNode === node.id
                    ? 'border-amber-500 shadow-sm shadow-amber-500/10'
                    : ''}
										{hoveredNode && hoveredNode !== node.id ? 'opacity-40' : ''}"
                  onmouseenter={() => (hoveredNode = node.id)}
                  onmouseleave={() => (hoveredNode = null)}
                >
                  <ResourceCardContent resource={node} />
                  <div class="text-amber-500/70 mt-0.5 text-xs">
                    External
                  </div>
                </div>
              {/if}
            </div>
          </foreignObject>
        {/each}
      </g>
    </svg>

    <div class="absolute bottom-3 right-3 flex gap-1">
      <button
        class="px-2.5 py-1 bg-white/80 backdrop-blur border border-gray-300 rounded text-gray-600 hover:bg-gray-200 transition-colors"
        onclick={fitToView}
      >
        Fit
      </button>
      <button
        class="px-2.5 py-1 bg-white/80 backdrop-blur border border-gray-300 rounded text-gray-600 hover:bg-gray-200 transition-colors"
        onclick={zoomInCenter}
      >
        +
      </button>
      <button
        class="px-2.5 py-1 bg-white/80 backdrop-blur border border-gray-300 rounded text-gray-600 hover:bg-gray-200 transition-colors"
        onclick={zoomOutCenter}
      >
        &minus;
      </button>
    </div>
  {/if}
</div>
