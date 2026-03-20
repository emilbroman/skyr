import type { TypedDocumentNode } from '@graphql-typed-document-node/core';
import { print } from 'graphql';
import { getToken } from '$lib/stores/auth';

function getApiUrl(): string {
	return '/graphql';
}

export async function execute<TData>(
	document: TypedDocumentNode<TData, any>,
	variables: Record<string, unknown>
): Promise<TData> {
	const token = getToken();
	const headers: Record<string, string> = {
		'Content-Type': 'application/json'
	};
	if (token) {
		headers['Authorization'] = `Bearer ${token}`;
	}

	const response = await fetch(getApiUrl(), {
		method: 'POST',
		headers,
		body: JSON.stringify({
			query: print(document),
			variables
		})
	});

	if (!response.ok) {
		throw new Error(`GraphQL request failed: ${response.status} ${response.statusText}`);
	}

	const json = await response.json();

	if (json.errors?.length) {
		throw new Error(json.errors.map((e: { message: string }) => e.message).join('; '));
	}

	if (!json.data) {
		throw new Error('GraphQL response missing data');
	}

	return json.data;
}

export async function query<TData, TVars extends Record<string, unknown>>(
	document: TypedDocumentNode<TData, TVars>,
	...args: TVars extends Record<string, never> ? [variables?: TVars] : [variables: TVars]
): Promise<TData> {
	return execute(document, args[0] ?? {});
}
