import { createQuery, createMutation, type CreateMutationOptions } from '@tanstack/svelte-query';
import type { TypedDocumentNode } from '@graphql-typed-document-node/core';
import { print } from 'graphql';
import { execute } from './client';

export function graphqlQuery<TData, TVars extends Record<string, unknown>>(
	options: () => {
		document: TypedDocumentNode<TData, TVars>;
		variables?: TVars;
		enabled?: boolean;
		refetchInterval?: number | false;
	}
) {
	return createQuery(() => {
		const { document, variables, enabled, refetchInterval } = options();
		return {
			queryKey: [print(document), variables ?? {}],
			queryFn: () => execute(document, variables ?? {}),
			enabled: enabled ?? true,
			refetchInterval: refetchInterval ?? false
		};
	});
}

export function graphqlMutation<TData, TVars extends Record<string, unknown>>(
	document: TypedDocumentNode<TData, TVars>,
	options?: Omit<CreateMutationOptions<TData, Error, TVars>, 'mutationFn'>
) {
	return createMutation(() => ({
		mutationFn: (variables: TVars) => execute(document, variables),
		...options
	}));
}
