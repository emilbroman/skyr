import {
	createHighlighter,
	type Highlighter,
	type ThemedToken
} from 'shiki';
import sclGrammar from '$lib/scl.tmLanguage.json';

let highlighterPromise: Promise<Highlighter> | null = null;

const SCL_LANG = {
	...sclGrammar,
	id: 'scl',
	scopeName: 'source.scl'
} as const;

function getHighlighter(): Promise<Highlighter> {
	if (!highlighterPromise) {
		highlighterPromise = createHighlighter({
			themes: ['github-dark'],
			langs: [
				'javascript',
				'typescript',
				'json',
				'yaml',
				'toml',
				'html',
				'css',
				'markdown',
				'bash',
				'shell',
				'python',
				'rust',
				'go',
				'dockerfile',
				'sql',
				'graphql',
				'xml',
				'ini',
				'diff',
				'plaintext',
				SCL_LANG
			]
		});
	}
	return highlighterPromise;
}

const EXT_TO_LANG: Record<string, string> = {
	js: 'javascript',
	mjs: 'javascript',
	cjs: 'javascript',
	ts: 'typescript',
	mts: 'typescript',
	cts: 'typescript',
	json: 'json',
	yaml: 'yaml',
	yml: 'yaml',
	toml: 'toml',
	html: 'html',
	htm: 'html',
	css: 'css',
	md: 'markdown',
	markdown: 'markdown',
	sh: 'bash',
	bash: 'bash',
	zsh: 'bash',
	py: 'python',
	rs: 'rust',
	go: 'go',
	dockerfile: 'dockerfile',
	sql: 'sql',
	graphql: 'graphql',
	gql: 'graphql',
	xml: 'xml',
	svg: 'xml',
	ini: 'ini',
	cfg: 'ini',
	diff: 'diff',
	patch: 'diff',
	scl: 'scl',
	txt: 'plaintext',
	lock: 'plaintext'
};

function detectLanguage(filename: string): string {
	const lower = filename.toLowerCase();
	if (lower === 'dockerfile' || lower.startsWith('dockerfile.')) return 'dockerfile';
	if (lower === 'makefile' || lower === 'gnumakefile') return 'bash';
	const dot = lower.lastIndexOf('.');
	if (dot === -1) return 'plaintext';
	const ext = lower.slice(dot + 1);
	return EXT_TO_LANG[ext] ?? 'plaintext';
}

export type HighlightedLine = ThemedToken[][];

export async function highlight(
	code: string,
	filename: string
): Promise<{ lines: ThemedToken[][]; bg: string }> {
	const hl = await getHighlighter();
	const lang = detectLanguage(filename);

	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	const result = hl.codeToTokens(code, {
		lang: lang as any,
		theme: 'github-dark'
	});

	return {
		lines: result.tokens,
		bg: result.bg ?? '#0d1117'
	};
}
