import type { TypedDocumentNode as DocumentNode } from '@graphql-typed-document-node/core';
export type Maybe<T> = T | null;
export type InputMaybe<T> = Maybe<T>;
export type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };
export type MakeOptional<T, K extends keyof T> = Omit<T, K> & { [SubKey in K]?: Maybe<T[SubKey]> };
export type MakeMaybe<T, K extends keyof T> = Omit<T, K> & { [SubKey in K]: Maybe<T[SubKey]> };
export type MakeEmpty<T extends { [key: string]: unknown }, K extends keyof T> = { [_ in K]?: never };
export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };
/** All built-in and custom scalars, mapped to their actual values */
export type Scalars = {
  ID: { input: string; output: string; }
  String: { input: string; output: string; }
  Boolean: { input: boolean; output: boolean; }
  Int: { input: number; output: number; }
  Float: { input: number; output: number; }
  JSON: { input: any; output: any; }
};

export type Artifact = {
  __typename?: 'Artifact';
  mediaType: Scalars['String']['output'];
  name: Scalars['String']['output'];
  namespace: Scalars['String']['output'];
  url: Scalars['String']['output'];
};

export type AuthSuccess = {
  __typename?: 'AuthSuccess';
  token: Scalars['String']['output'];
  user: User;
};

export type Blob = {
  __typename?: 'Blob';
  content?: Maybe<Scalars['String']['output']>;
  hash: Scalars['String']['output'];
  name?: Maybe<Scalars['String']['output']>;
  size: Scalars['Int']['output'];
};

export type Commit = {
  __typename?: 'Commit';
  hash: Scalars['String']['output'];
  message: Scalars['String']['output'];
  tree: Tree;
  treeEntry?: Maybe<TreeEntry>;
};


export type CommitTreeEntryArgs = {
  path: Scalars['String']['input'];
};

export type Deployment = {
  __typename?: 'Deployment';
  artifacts: Array<Artifact>;
  commit: Commit;
  createdAt: Scalars['String']['output'];
  id: Scalars['String']['output'];
  lastLogs: Array<Log>;
  ref: Scalars['String']['output'];
  resources: Array<Resource>;
  state: DeploymentState;
};


export type DeploymentLastLogsArgs = {
  amount?: InputMaybe<Scalars['Int']['input']>;
};

export enum DeploymentState {
  Desired = 'DESIRED',
  Down = 'DOWN',
  Lingering = 'LINGERING',
  Undesired = 'UNDESIRED',
  Up = 'UP'
}

export type Environment = {
  __typename?: 'Environment';
  deployment: Deployment;
  deployments: Array<Deployment>;
  lastLogs: Array<Log>;
  name: Scalars['String']['output'];
  qid: Scalars['String']['output'];
  resource?: Maybe<Resource>;
  resources: Array<Resource>;
};


export type EnvironmentDeploymentArgs = {
  commit: Scalars['String']['input'];
};


export type EnvironmentLastLogsArgs = {
  amount?: InputMaybe<Scalars['Int']['input']>;
};


export type EnvironmentResourceArgs = {
  id: Scalars['String']['input'];
};

export type Log = {
  __typename?: 'Log';
  message: Scalars['String']['output'];
  severity: Severity;
  timestamp: Scalars['String']['output'];
};

export type Mutation = {
  __typename?: 'Mutation';
  createRepository: Repository;
  signin: AuthSuccess;
  signup: AuthSuccess;
};


export type MutationCreateRepositoryArgs = {
  organization: Scalars['String']['input'];
  repository: Scalars['String']['input'];
};


export type MutationSigninArgs = {
  pubkey: Scalars['String']['input'];
  signature: Scalars['String']['input'];
  username: Scalars['String']['input'];
};


export type MutationSignupArgs = {
  email: Scalars['String']['input'];
  pubkey: Scalars['String']['input'];
  signature: Scalars['String']['input'];
  username: Scalars['String']['input'];
};

export type Organization = {
  __typename?: 'Organization';
  name: Scalars['String']['output'];
  repositories: Array<Repository>;
  repository: Repository;
};


export type OrganizationRepositoryArgs = {
  name: Scalars['String']['input'];
};

export type Query = {
  __typename?: 'Query';
  authChallenge: Scalars['String']['output'];
  health: Scalars['Boolean']['output'];
  me: User;
  organization: Organization;
  organizations: Array<Organization>;
  refreshToken: AuthSuccess;
  repositories: Array<Repository>;
};


export type QueryAuthChallengeArgs = {
  username: Scalars['String']['input'];
};


export type QueryOrganizationArgs = {
  name: Scalars['String']['input'];
};

export type Repository = {
  __typename?: 'Repository';
  commit: Commit;
  environment: Environment;
  environments: Array<Environment>;
  name: Scalars['String']['output'];
  organization: Organization;
};


export type RepositoryCommitArgs = {
  hash: Scalars['String']['input'];
};


export type RepositoryEnvironmentArgs = {
  name: Scalars['String']['input'];
};

export type Resource = {
  __typename?: 'Resource';
  dependencies: Array<Resource>;
  inputs?: Maybe<Scalars['JSON']['output']>;
  lastLogs: Array<Log>;
  markers: Array<ResourceMarker>;
  name: Scalars['String']['output'];
  outputs?: Maybe<Scalars['JSON']['output']>;
  owner?: Maybe<Deployment>;
  sourceTrace: Array<SourceFrame>;
  type: Scalars['String']['output'];
};


export type ResourceLastLogsArgs = {
  amount?: InputMaybe<Scalars['Int']['input']>;
};

export enum ResourceMarker {
  Sticky = 'STICKY',
  Volatile = 'VOLATILE'
}

export enum Severity {
  Error = 'ERROR',
  Info = 'INFO',
  Warning = 'WARNING'
}

export type SourceFrame = {
  __typename?: 'SourceFrame';
  moduleId: Scalars['String']['output'];
  name: Scalars['String']['output'];
  span: Scalars['String']['output'];
};

export type Subscription = {
  __typename?: 'Subscription';
  deploymentLogs: Log;
  environmentLogs: Log;
  resourceLogs: Log;
};


export type SubscriptionDeploymentLogsArgs = {
  deploymentId: Scalars['String']['input'];
  initialAmount?: InputMaybe<Scalars['Int']['input']>;
};


export type SubscriptionEnvironmentLogsArgs = {
  environmentQid: Scalars['String']['input'];
  initialAmount?: InputMaybe<Scalars['Int']['input']>;
};


export type SubscriptionResourceLogsArgs = {
  initialAmount?: InputMaybe<Scalars['Int']['input']>;
  resourceQid: Scalars['String']['input'];
};

export type Tree = {
  __typename?: 'Tree';
  entries: Array<TreeEntry>;
  hash: Scalars['String']['output'];
  name?: Maybe<Scalars['String']['output']>;
};

export type TreeEntry = Blob | Tree;

export type User = {
  __typename?: 'User';
  email: Scalars['String']['output'];
  fullname?: Maybe<Scalars['String']['output']>;
  username: Scalars['String']['output'];
};

export type AuthChallengeQueryVariables = Exact<{
  username: Scalars['String']['input'];
}>;


export type AuthChallengeQuery = { __typename?: 'Query', authChallenge: string };

export type SignInMutationVariables = Exact<{
  username: Scalars['String']['input'];
  signature: Scalars['String']['input'];
  pubkey: Scalars['String']['input'];
}>;


export type SignInMutation = { __typename?: 'Mutation', signin: { __typename?: 'AuthSuccess', token: string, user: { __typename?: 'User', username: string, email: string, fullname?: string | null } } };

export type RefreshTokenQueryVariables = Exact<{ [key: string]: never; }>;


export type RefreshTokenQuery = { __typename?: 'Query', refreshToken: { __typename?: 'AuthSuccess', token: string, user: { __typename?: 'User', username: string, email: string, fullname?: string | null } } };

export type MeQueryVariables = Exact<{ [key: string]: never; }>;


export type MeQuery = { __typename?: 'Query', me: { __typename?: 'User', username: string, email: string, fullname?: string | null } };

export type EnvironmentDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
}>;


export type EnvironmentDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', name: string, qid: string, deployments: Array<{ __typename?: 'Deployment', id: string, ref: string, createdAt: string, state: DeploymentState, commit: { __typename?: 'Commit', hash: string, message: string }, resources: Array<{ __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }> }>, artifacts: Array<{ __typename?: 'Artifact', namespace: string, name: string, mediaType: string, url: string }>, lastLogs: Array<{ __typename?: 'Log', severity: Severity, timestamp: string, message: string }> }>, resources: Array<{ __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, owner?: { __typename?: 'Deployment', id: string } | null, dependencies: Array<{ __typename?: 'Resource', type: string, name: string }>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }> }> } } } };

export type ResourceDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
  resourceId: Scalars['String']['input'];
}>;


export type ResourceDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', qid: string, resource?: { __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, owner?: { __typename?: 'Deployment', id: string, ref: string, createdAt: string, state: DeploymentState, commit: { __typename?: 'Commit', hash: string, message: string } } | null, dependencies: Array<{ __typename?: 'Resource', type: string, name: string }>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }> } | null } } } };

export type DeploymentDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
  commit: Scalars['String']['input'];
}>;


export type DeploymentDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', deployment: { __typename?: 'Deployment', id: string, ref: string, createdAt: string, state: DeploymentState, commit: { __typename?: 'Commit', hash: string, message: string }, resources: Array<{ __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }> }>, artifacts: Array<{ __typename?: 'Artifact', namespace: string, name: string, mediaType: string, url: string }>, lastLogs: Array<{ __typename?: 'Log', severity: Severity, timestamp: string, message: string }> } } } } };

export type DeploymentLogsSubscriptionVariables = Exact<{
  deploymentId: Scalars['String']['input'];
  initialAmount?: InputMaybe<Scalars['Int']['input']>;
}>;


export type DeploymentLogsSubscription = { __typename?: 'Subscription', deploymentLogs: { __typename?: 'Log', severity: Severity, timestamp: string, message: string } };

export type EnvironmentLogsSubscriptionVariables = Exact<{
  environmentQid: Scalars['String']['input'];
  initialAmount?: InputMaybe<Scalars['Int']['input']>;
}>;


export type EnvironmentLogsSubscription = { __typename?: 'Subscription', environmentLogs: { __typename?: 'Log', severity: Severity, timestamp: string, message: string } };

export type ResourceLogsSubscriptionVariables = Exact<{
  resourceQid: Scalars['String']['input'];
  initialAmount?: InputMaybe<Scalars['Int']['input']>;
}>;


export type ResourceLogsSubscription = { __typename?: 'Subscription', resourceLogs: { __typename?: 'Log', severity: Severity, timestamp: string, message: string } };

export type OrganizationsQueryVariables = Exact<{ [key: string]: never; }>;


export type OrganizationsQuery = { __typename?: 'Query', organizations: Array<{ __typename?: 'Organization', name: string, repositories: Array<{ __typename?: 'Repository', name: string }> }> };

export type OrganizationDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
}>;


export type OrganizationDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', name: string, repositories: Array<{ __typename?: 'Repository', name: string, environments: Array<{ __typename?: 'Environment', name: string, qid: string, deployments: Array<{ __typename?: 'Deployment', id: string, ref: string, state: DeploymentState, commit: { __typename?: 'Commit', hash: string, message: string } }> }> }> } };

export type RepositoryDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
}>;


export type RepositoryDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', name: string, organization: { __typename?: 'Organization', name: string }, environments: Array<{ __typename?: 'Environment', name: string, qid: string, deployments: Array<{ __typename?: 'Deployment', id: string, ref: string, state: DeploymentState, commit: { __typename?: 'Commit', hash: string, message: string } }> }> } } };

export type CommitTreeEntryQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  commit: Scalars['String']['input'];
  path: Scalars['String']['input'];
}>;


export type CommitTreeEntryQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', commit: { __typename?: 'Commit', hash: string, message: string, treeEntry?: { __typename: 'Blob', hash: string, name?: string | null, size: number, content?: string | null } | { __typename: 'Tree', hash: string, name?: string | null, entries: Array<{ __typename: 'Blob', hash: string, name?: string | null, size: number } | { __typename: 'Tree', hash: string, name?: string | null }> } | null } } } };

export type CommitRootTreeQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  commit: Scalars['String']['input'];
}>;


export type CommitRootTreeQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', commit: { __typename?: 'Commit', hash: string, message: string, tree: { __typename?: 'Tree', hash: string, entries: Array<{ __typename: 'Blob', hash: string, name?: string | null, size: number } | { __typename: 'Tree', hash: string, name?: string | null }> } } } } };


export const AuthChallengeDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"AuthChallenge"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"username"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"authChallenge"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"username"},"value":{"kind":"Variable","name":{"kind":"Name","value":"username"}}}]}]}}]} as unknown as DocumentNode<AuthChallengeQuery, AuthChallengeQueryVariables>;
export const SignInDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"SignIn"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"username"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"signature"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"pubkey"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"signin"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"username"},"value":{"kind":"Variable","name":{"kind":"Name","value":"username"}}},{"kind":"Argument","name":{"kind":"Name","value":"signature"},"value":{"kind":"Variable","name":{"kind":"Name","value":"signature"}}},{"kind":"Argument","name":{"kind":"Name","value":"pubkey"},"value":{"kind":"Variable","name":{"kind":"Name","value":"pubkey"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"user"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}}]}},{"kind":"Field","name":{"kind":"Name","value":"token"}}]}}]}}]} as unknown as DocumentNode<SignInMutation, SignInMutationVariables>;
export const RefreshTokenDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"RefreshToken"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"refreshToken"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"user"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}}]}},{"kind":"Field","name":{"kind":"Name","value":"token"}}]}}]}}]} as unknown as DocumentNode<RefreshTokenQuery, RefreshTokenQueryVariables>;
export const MeDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"Me"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"me"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}}]}}]}}]} as unknown as DocumentNode<MeQuery, MeQueryVariables>;
export const EnvironmentDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"EnvironmentDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"deployments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}},{"kind":"Field","name":{"kind":"Name","value":"createdAt"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"artifacts"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"namespace"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"mediaType"}},{"kind":"Field","name":{"kind":"Name","value":"url"}}]}},{"kind":"Field","name":{"kind":"Name","value":"lastLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"amount"},"value":{"kind":"IntValue","value":"20"}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"owner"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"dependencies"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<EnvironmentDetailQuery, EnvironmentDetailQueryVariables>;
export const ResourceDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"ResourceDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"resourceId"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"resource"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"id"},"value":{"kind":"Variable","name":{"kind":"Name","value":"resourceId"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"owner"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}},{"kind":"Field","name":{"kind":"Name","value":"createdAt"}},{"kind":"Field","name":{"kind":"Name","value":"state"}}]}},{"kind":"Field","name":{"kind":"Name","value":"dependencies"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<ResourceDetailQuery, ResourceDetailQueryVariables>;
export const DeploymentDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"DeploymentDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"commit"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"deployment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"commit"},"value":{"kind":"Variable","name":{"kind":"Name","value":"commit"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}},{"kind":"Field","name":{"kind":"Name","value":"createdAt"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"artifacts"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"namespace"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"mediaType"}},{"kind":"Field","name":{"kind":"Name","value":"url"}}]}},{"kind":"Field","name":{"kind":"Name","value":"lastLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"amount"},"value":{"kind":"IntValue","value":"20"}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<DeploymentDetailQuery, DeploymentDetailQueryVariables>;
export const DeploymentLogsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"subscription","name":{"kind":"Name","value":"DeploymentLogs"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"deploymentId"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"deploymentLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"deploymentId"},"value":{"kind":"Variable","name":{"kind":"Name","value":"deploymentId"}}},{"kind":"Argument","name":{"kind":"Name","value":"initialAmount"},"value":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]} as unknown as DocumentNode<DeploymentLogsSubscription, DeploymentLogsSubscriptionVariables>;
export const EnvironmentLogsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"subscription","name":{"kind":"Name","value":"EnvironmentLogs"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"environmentQid"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environmentLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"environmentQid"},"value":{"kind":"Variable","name":{"kind":"Name","value":"environmentQid"}}},{"kind":"Argument","name":{"kind":"Name","value":"initialAmount"},"value":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]} as unknown as DocumentNode<EnvironmentLogsSubscription, EnvironmentLogsSubscriptionVariables>;
export const ResourceLogsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"subscription","name":{"kind":"Name","value":"ResourceLogs"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"resourceQid"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"resourceLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"resourceQid"},"value":{"kind":"Variable","name":{"kind":"Name","value":"resourceQid"}}},{"kind":"Argument","name":{"kind":"Name","value":"initialAmount"},"value":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]} as unknown as DocumentNode<ResourceLogsSubscription, ResourceLogsSubscriptionVariables>;
export const OrganizationsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"Organizations"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organizations"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"repositories"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]}}]} as unknown as DocumentNode<OrganizationsQuery, OrganizationsQueryVariables>;
export const OrganizationDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"OrganizationDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"repositories"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"environments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"deployments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<OrganizationDetailQuery, OrganizationDetailQueryVariables>;
export const RepositoryDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"RepositoryDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"organization"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"environments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"deployments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<RepositoryDetailQuery, RepositoryDetailQueryVariables>;
export const CommitTreeEntryDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"CommitTreeEntry"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"commit"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"path"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"commit"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"hash"},"value":{"kind":"Variable","name":{"kind":"Name","value":"commit"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}},{"kind":"Field","name":{"kind":"Name","value":"treeEntry"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"path"},"value":{"kind":"Variable","name":{"kind":"Name","value":"path"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Tree"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"entries"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Tree"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Blob"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"size"}}]}}]}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Blob"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"size"}},{"kind":"Field","name":{"kind":"Name","value":"content"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<CommitTreeEntryQuery, CommitTreeEntryQueryVariables>;
export const CommitRootTreeDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"CommitRootTree"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"commit"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"commit"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"hash"},"value":{"kind":"Variable","name":{"kind":"Name","value":"commit"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}},{"kind":"Field","name":{"kind":"Name","value":"tree"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"entries"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Tree"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Blob"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"size"}}]}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<CommitRootTreeQuery, CommitRootTreeQueryVariables>;