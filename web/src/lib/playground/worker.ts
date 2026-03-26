import init, {
    analyze,
    completions,
    format,
    goto_definition,
    hover,
} from "$lib/sclc-wasm/sclc_wasm.js";

type Request =
    | { id: number; type: "analyze"; source: string }
    | { id: number; type: "hover"; source: string; line: number; col: number }
    | { id: number; type: "completions"; source: string; line: number; col: number }
    | { id: number; type: "gotoDefinition"; source: string; line: number; col: number }
    | { id: number; type: "format"; source: string };

let ready = false;
const queue: Request[] = [];

async function initialize() {
    await init();
    ready = true;
    for (const msg of queue) {
        await handleMessage(msg);
    }
    queue.length = 0;
}

async function handleMessage(msg: Request) {
    try {
        let result: unknown;
        switch (msg.type) {
            case "analyze":
                result = JSON.parse(await analyze(msg.source));
                break;
            case "hover": {
                const h = await hover(msg.source, msg.line, msg.col);
                result = h ? JSON.parse(h) : null;
                break;
            }
            case "completions":
                result = JSON.parse(await completions(msg.source, msg.line, msg.col));
                break;
            case "gotoDefinition": {
                const loc = await goto_definition(msg.source, msg.line, msg.col);
                result = loc ? JSON.parse(loc) : null;
                break;
            }
            case "format":
                result = format(msg.source) ?? null;
                break;
        }
        self.postMessage({ id: msg.id, result });
    } catch (err) {
        self.postMessage({ id: msg.id, error: String(err) });
    }
}

self.onmessage = async (e: MessageEvent<Request>) => {
    if (!ready) {
        queue.push(e.data);
        return;
    }
    await handleMessage(e.data);
};

initialize();
