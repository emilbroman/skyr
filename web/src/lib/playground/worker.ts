import init, {
    analyze,
    analyze_scle,
    completions,
    format,
    format_scle,
    goto_definition,
    hover,
    repl_eval,
    repl_init,
    repl_reset,
} from "$lib/sclc-wasm/sclc_wasm.js";

type Request =
    | { id: number; type: "analyze"; files: Record<string, string> }
    | {
          id: number;
          type: "hover";
          files: Record<string, string>;
          file: string;
          line: number;
          col: number;
      }
    | {
          id: number;
          type: "completions";
          files: Record<string, string>;
          file: string;
          line: number;
          col: number;
      }
    | {
          id: number;
          type: "gotoDefinition";
          files: Record<string, string>;
          file: string;
          line: number;
          col: number;
      }
    | { id: number; type: "format"; source: string }
    | { id: number; type: "formatScle"; source: string }
    | { id: number; type: "analyzeScle"; source: string }
    | { id: number; type: "replInit" }
    | { id: number; type: "replEval"; files: Record<string, string>; line: string }
    | { id: number; type: "replReset" };

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
        const filesJson = "files" in msg ? JSON.stringify(msg.files) : "";
        switch (msg.type) {
            case "analyze":
                result = JSON.parse(await analyze(filesJson));
                break;
            case "hover": {
                const h = await hover(filesJson, msg.file, msg.line, msg.col);
                result = h ? JSON.parse(h) : null;
                break;
            }
            case "completions":
                result = JSON.parse(await completions(filesJson, msg.file, msg.line, msg.col));
                break;
            case "gotoDefinition": {
                const loc = await goto_definition(filesJson, msg.file, msg.line, msg.col);
                result = loc ? JSON.parse(loc) : null;
                break;
            }
            case "format":
                result = format(msg.source) ?? null;
                break;
            case "formatScle":
                result = format_scle(msg.source) ?? null;
                break;
            case "analyzeScle":
                result = JSON.parse(await analyze_scle(msg.source));
                break;
            case "replInit":
                repl_init();
                result = null;
                break;
            case "replEval":
                result = JSON.parse(await repl_eval(filesJson, msg.line));
                break;
            case "replReset":
                repl_reset();
                result = null;
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
