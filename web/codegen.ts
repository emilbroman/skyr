import type { CodegenConfig } from '@graphql-codegen/cli';

const config: CodegenConfig = {
	schema: '../crates/api/schema.graphql',
	documents: 'src/lib/graphql/documents/**/*.graphql',
	generates: {
		'src/lib/graphql/generated.ts': {
			plugins: [
				'typescript',
				'typescript-operations',
				'typed-document-node'
			],
			config: {
				useTypeImports: true
			}
		}
	}
};

export default config;
