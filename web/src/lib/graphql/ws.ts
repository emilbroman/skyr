import type { TypedDocumentNode } from '@graphql-typed-document-node/core';
import { print } from 'graphql';
import { createClient, type Client } from 'graphql-ws';
import { getToken } from '$lib/stores/auth';
import type { Log } from '$lib/graphql/generated';

let client: Client | null = null;

function getWsUrl(): string {
	const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${protocol}//${window.location.host}/graphql`;
}

function getClient(): Client {
	if (client) return client;

	client = createClient({
		url: getWsUrl(),
		connectionParams: () => {
			const token = getToken();
			if (token) {
				return { Authorization: `Bearer ${token}` };
			}
			return {};
		},
		shouldRetry: () => true,
		retryAttempts: 5,
		retryWait: (attempt) => new Promise((resolve) => setTimeout(resolve, Math.min(1000 * 2 ** attempt, 30000)))
	});

	return client;
}

export function resetWsClient() {
	if (client) {
		client.dispose();
		client = null;
	}
}

export function subscribe<TData, TVars extends Record<string, unknown>>(
	document: TypedDocumentNode<TData, TVars>,
	variables: TVars,
	onData: (data: TData) => void,
	onError?: (error: Error) => void
): () => void {
	const wsClient = getClient();

	const unsubscribe = wsClient.subscribe<TData>(
		{
			query: print(document),
			variables: variables as Record<string, unknown>
		},
		{
			next: (result) => {
				if (result.data) {
					onData(result.data);
				}
			},
			error: (err) => {
				const error = err instanceof Error ? err : new Error(String(err));
				onError?.(error);
			},
			complete: () => {}
		}
	);

	return unsubscribe;
}
