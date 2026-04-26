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
  url: Scalars['String']['output'];
};

export type AuthChallenge = {
  __typename?: 'AuthChallenge';
  challenge: Scalars['String']['output'];
  passkeyRegistration: Scalars['JSON']['output'];
  passkeySignin?: Maybe<Scalars['JSON']['output']>;
  taken: Scalars['Boolean']['output'];
};

export type AuthSuccess = {
  __typename?: 'AuthSuccess';
  token: Scalars['String']['output'];
  user: SignedInUser;
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
  parents: Array<Commit>;
  tree: Tree;
  treeEntry?: Maybe<TreeEntry>;
};


export type CommitTreeEntryArgs = {
  path: Scalars['String']['input'];
};

export type Deployment = {
  __typename?: 'Deployment';
  bootstrapped: Scalars['Boolean']['output'];
  commit: Commit;
  createdAt: Scalars['String']['output'];
  id: Scalars['String']['output'];
  /** Incidents scoped to this deployment (newest first). */
  incidents: Array<Incident>;
  lastLogs: Array<Log>;
  nonce: Scalars['String']['output'];
  ref: Scalars['String']['output'];
  resources: Array<Resource>;
  state: DeploymentState;
  /**
   * Per-deployment health rollup. **Self-only** — does not aggregate child
   * resource health. Resource statuses are reached via
   * `Deployment.resources -> Resource.status`.
   */
  status: StatusSummary;
};


export type DeploymentIncidentsArgs = {
  category?: InputMaybe<IncidentCategory>;
  entityQid?: InputMaybe<Scalars['String']['input']>;
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  openOnly?: InputMaybe<Scalars['Boolean']['input']>;
  since?: InputMaybe<Scalars['String']['input']>;
  until?: InputMaybe<Scalars['String']['input']>;
};


export type DeploymentLastLogsArgs = {
  amount?: InputMaybe<Scalars['Int']['input']>;
};

export enum DeploymentState {
  Desired = 'DESIRED',
  Down = 'DOWN',
  Lingering = 'LINGERING',
  Undesired = 'UNDESIRED'
}

export type Environment = {
  __typename?: 'Environment';
  artifacts: Array<Artifact>;
  deployment: Deployment;
  deployments: Array<Deployment>;
  /** Incidents scoped to this environment (newest first). */
  incidents: Array<Incident>;
  lastLogs: Array<Log>;
  name: Scalars['String']['output'];
  qid: Scalars['String']['output'];
  resource?: Maybe<Resource>;
  resources: Array<Resource>;
};


export type EnvironmentDeploymentArgs = {
  id: Scalars['String']['input'];
};


export type EnvironmentIncidentsArgs = {
  category?: InputMaybe<IncidentCategory>;
  entityQid?: InputMaybe<Scalars['String']['input']>;
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  openOnly?: InputMaybe<Scalars['Boolean']['input']>;
  since?: InputMaybe<Scalars['String']['input']>;
  until?: InputMaybe<Scalars['String']['input']>;
};


export type EnvironmentLastLogsArgs = {
  amount?: InputMaybe<Scalars['Int']['input']>;
};


export type EnvironmentResourceArgs = {
  id: Scalars['String']['input'];
};

/**
 * UI-friendly rolled-up health enum, computed from the underlying counters
 * in [`StatusSummary`]:
 *
 * - `Healthy` iff `openIncidentCount == 0`
 * - `Down` iff `worstOpenCategory == Crash`
 * - `Degraded` otherwise
 */
export enum HealthStatus {
  Degraded = 'DEGRADED',
  Down = 'DOWN',
  Healthy = 'HEALTHY'
}

export type Incident = {
  __typename?: 'Incident';
  category: IncidentCategory;
  closedAt?: Maybe<Scalars['String']['output']>;
  /**
   * Back-edge to the owning deployment. Populated only for incidents about
   * deployments. `null` for resource-scoped incidents.
   */
  deployment?: Maybe<Deployment>;
  entityQid: Scalars['String']['output'];
  /**
   * Back-edge to the owning environment. Always populated when the entity
   * QID is parseable. Note that the environment object is constructed from
   * the deployments currently in CDB; if there are no deployments left the
   * returned `Environment` will have an empty `deployments` list.
   */
  environment?: Maybe<Environment>;
  id: Scalars['ID']['output'];
  lastErrorMessage?: Maybe<Scalars['String']['output']>;
  lastReportAt: Scalars['String']['output'];
  openedAt: Scalars['String']['output'];
  /** Back-edge to the owning organization. Always populated. */
  organization?: Maybe<Organization>;
  reportCount: Scalars['Int']['output'];
  /**
   * Back-edge to the owning repository. Always populated when the entity
   * QID is parseable.
   */
  repository?: Maybe<Repository>;
  /**
   * Back-edge to the owning resource. Populated only for incidents about
   * resources. `null` for deployment-scoped incidents. The resource may
   * have been destroyed since the incident was opened, in which case this
   * resolves to `null`.
   */
  resource?: Maybe<Resource>;
  triggeringReportSummary?: Maybe<Scalars['String']['output']>;
};

/**
 * Producer-classified failure category. Mirrors [`sdb::Category`] /
 * [`rq::IncidentCategory`].
 */
export enum IncidentCategory {
  BadConfiguration = 'BAD_CONFIGURATION',
  CannotProgress = 'CANNOT_PROGRESS',
  Crash = 'CRASH',
  InconsistentState = 'INCONSISTENT_STATE',
  SystemError = 'SYSTEM_ERROR'
}

export type Log = {
  __typename?: 'Log';
  message: Scalars['String']['output'];
  severity: Severity;
  timestamp: Scalars['String']['output'];
};

export type Mutation = {
  __typename?: 'Mutation';
  addOrganizationMember: Organization;
  addPublicKey: SignedInUser;
  /**
   * Create a new deployment for the given commit hash and make it
   * `Desired`, superseding whichever deployment is currently active
   * in the same environment.
   *
   * Requires the caller to be a member of the owning organisation (the
   * same access level as other repository-scoped operations).
   */
  createDeployment: Deployment;
  createOrganization: Organization;
  createRepository: Repository;
  /**
   * Manually request the deletion of a single resource.  Publishes a
   * `Destroy` message to RTQ that the RTE worker pool will consume and
   * forward to the resource's plugin.  The RDB row is cleared on the
   * plugin's success; on failure the row remains and the failure is
   * visible in the resource's log stream.
   *
   * This is an imperative action intended as an escape hatch.  The
   * declarative model still applies: if the owning deployment is still
   * `Desired` and the resource is part of its current evaluation, the
   * deployment engine will recreate the resource on its next tick.
   *
   * Requires the caller to be a member of the owning organisation (the
   * same access level as other repository-scoped mutations).
   */
  deleteResource: Scalars['Boolean']['output'];
  leaveOrganization: Scalars['Boolean']['output'];
  removePublicKey: SignedInUser;
  signin: AuthSuccess;
  signup: AuthSuccess;
  /**
   * Tear down an environment by transitioning all currently-`Desired`
   * deployments to `Undesired` without superseding them.  This mirrors
   * the behaviour of deleting a Git ref via SCS.
   *
   * Requires the caller to be a member of the owning organisation.
   */
  tearDownEnvironment: Scalars['Boolean']['output'];
  updateFullname: SignedInUser;
};


export type MutationAddOrganizationMemberArgs = {
  organization: Scalars['String']['input'];
  username: Scalars['String']['input'];
};


export type MutationAddPublicKeyArgs = {
  proof: Scalars['JSON']['input'];
};


export type MutationCreateDeploymentArgs = {
  commitHash: Scalars['String']['input'];
  environment: Scalars['String']['input'];
  organization: Scalars['String']['input'];
  repository: Scalars['String']['input'];
};


export type MutationCreateOrganizationArgs = {
  name: Scalars['String']['input'];
};


export type MutationCreateRepositoryArgs = {
  organization: Scalars['String']['input'];
  repository: Scalars['String']['input'];
};


export type MutationDeleteResourceArgs = {
  environment: Scalars['String']['input'];
  organization: Scalars['String']['input'];
  repository: Scalars['String']['input'];
  resource: Scalars['String']['input'];
};


export type MutationLeaveOrganizationArgs = {
  organization: Scalars['String']['input'];
};


export type MutationRemovePublicKeyArgs = {
  fingerprint: Scalars['String']['input'];
};


export type MutationSigninArgs = {
  proof: Scalars['JSON']['input'];
  username: Scalars['String']['input'];
};


export type MutationSignupArgs = {
  email: Scalars['String']['input'];
  fullname?: InputMaybe<Scalars['String']['input']>;
  proof: Scalars['JSON']['input'];
  username: Scalars['String']['input'];
};


export type MutationTearDownEnvironmentArgs = {
  environment: Scalars['String']['input'];
  organization: Scalars['String']['input'];
  repository: Scalars['String']['input'];
};


export type MutationUpdateFullnameArgs = {
  fullname: Scalars['String']['input'];
};

export type Organization = {
  __typename?: 'Organization';
  /**
   * Look up a single incident by id, scoped to this organization. Returns
   * `None` if no such incident exists, or if it exists but does not belong
   * to this organization (ACL via scope traversal).
   */
  incident?: Maybe<Incident>;
  /** Incidents scoped to this organization (newest first). */
  incidents: Array<Incident>;
  members: Array<User>;
  name: Scalars['String']['output'];
  repositories: Array<Repository>;
  repository: Repository;
};


export type OrganizationIncidentArgs = {
  id: Scalars['ID']['input'];
};


export type OrganizationIncidentsArgs = {
  category?: InputMaybe<IncidentCategory>;
  entityQid?: InputMaybe<Scalars['String']['input']>;
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  openOnly?: InputMaybe<Scalars['Boolean']['input']>;
  since?: InputMaybe<Scalars['String']['input']>;
  until?: InputMaybe<Scalars['String']['input']>;
};


export type OrganizationRepositoryArgs = {
  name: Scalars['String']['input'];
};

export type Query = {
  __typename?: 'Query';
  authChallenge: AuthChallenge;
  health: Scalars['Boolean']['output'];
  me: SignedInUser;
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
  /** Incidents scoped to this repository (newest first). */
  incidents: Array<Incident>;
  name: Scalars['String']['output'];
  organization: Organization;
};


export type RepositoryCommitArgs = {
  hash: Scalars['String']['input'];
};


export type RepositoryEnvironmentArgs = {
  name: Scalars['String']['input'];
};


export type RepositoryIncidentsArgs = {
  category?: InputMaybe<IncidentCategory>;
  entityQid?: InputMaybe<Scalars['String']['input']>;
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  openOnly?: InputMaybe<Scalars['Boolean']['input']>;
  since?: InputMaybe<Scalars['String']['input']>;
  until?: InputMaybe<Scalars['String']['input']>;
};

export type Resource = {
  __typename?: 'Resource';
  dependencies: Array<Resource>;
  /** Incidents scoped to this resource (newest first). */
  incidents: Array<Incident>;
  inputs?: Maybe<Scalars['JSON']['output']>;
  lastLogs: Array<Log>;
  markers: Array<ResourceMarker>;
  name: Scalars['String']['output'];
  outputs?: Maybe<Scalars['JSON']['output']>;
  owner?: Maybe<Deployment>;
  sourceTrace: Array<SourceFrame>;
  /** Per-resource health rollup. */
  status: StatusSummary;
  type: Scalars['String']['output'];
};


export type ResourceIncidentsArgs = {
  category?: InputMaybe<IncidentCategory>;
  entityQid?: InputMaybe<Scalars['String']['input']>;
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  openOnly?: InputMaybe<Scalars['Boolean']['input']>;
  since?: InputMaybe<Scalars['String']['input']>;
  until?: InputMaybe<Scalars['String']['input']>;
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

export type SignedInUser = {
  __typename?: 'SignedInUser';
  email: Scalars['String']['output'];
  fullname?: Maybe<Scalars['String']['output']>;
  publicKeys: Array<Scalars['String']['output']>;
  username: Scalars['String']['output'];
};

export type SourceFrame = {
  __typename?: 'SourceFrame';
  moduleId: Scalars['String']['output'];
  name: Scalars['String']['output'];
  span: Scalars['String']['output'];
};

export type StatusSummary = {
  __typename?: 'StatusSummary';
  consecutiveFailureCount: Scalars['Int']['output'];
  /** UI-friendly rolled-up health enum. */
  health: HealthStatus;
  lastReportAt: Scalars['String']['output'];
  lastReportSucceeded: Scalars['Boolean']['output'];
  openIncidentCount: Scalars['Int']['output'];
  worstOpenCategory?: Maybe<IncidentCategory>;
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


export type AuthChallengeQuery = { __typename?: 'Query', authChallenge: { __typename?: 'AuthChallenge', challenge: string, taken: boolean, passkeyRegistration: any, passkeySignin?: any | null } };

export type SignInMutationVariables = Exact<{
  username: Scalars['String']['input'];
  proof: Scalars['JSON']['input'];
}>;


export type SignInMutation = { __typename?: 'Mutation', signin: { __typename?: 'AuthSuccess', token: string, user: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string> } } };

export type SignupMutationVariables = Exact<{
  username: Scalars['String']['input'];
  email: Scalars['String']['input'];
  proof: Scalars['JSON']['input'];
  fullname?: InputMaybe<Scalars['String']['input']>;
}>;


export type SignupMutation = { __typename?: 'Mutation', signup: { __typename?: 'AuthSuccess', token: string, user: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string> } } };

export type RefreshTokenQueryVariables = Exact<{ [key: string]: never; }>;


export type RefreshTokenQuery = { __typename?: 'Query', refreshToken: { __typename?: 'AuthSuccess', token: string, user: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string> } } };

export type MeQueryVariables = Exact<{ [key: string]: never; }>;


export type MeQuery = { __typename?: 'Query', me: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string> } };

export type EnvironmentDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
}>;


export type EnvironmentDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', name: string, qid: string, deployments: Array<{ __typename?: 'Deployment', id: string, nonce: string, ref: string, createdAt: string, state: DeploymentState, bootstrapped: boolean, commit: { __typename?: 'Commit', hash: string, message: string }, status: { __typename?: 'StatusSummary', health: HealthStatus, lastReportAt: string, lastReportSucceeded: boolean, openIncidentCount: number, worstOpenCategory?: IncidentCategory | null, consecutiveFailureCount: number }, resources: Array<{ __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }> }>, lastLogs: Array<{ __typename?: 'Log', severity: Severity, timestamp: string, message: string }> }>, artifacts: Array<{ __typename?: 'Artifact', name: string, mediaType: string, url: string }>, resources: Array<{ __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, owner?: { __typename?: 'Deployment', id: string } | null, dependencies: Array<{ __typename?: 'Resource', type: string, name: string }>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }>, status: { __typename?: 'StatusSummary', health: HealthStatus, lastReportAt: string, lastReportSucceeded: boolean, openIncidentCount: number, worstOpenCategory?: IncidentCategory | null, consecutiveFailureCount: number } }> } } } };

export type ResourceDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
  resourceId: Scalars['String']['input'];
}>;


export type ResourceDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', qid: string, resource?: { __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, owner?: { __typename?: 'Deployment', id: string, nonce: string, ref: string, createdAt: string, state: DeploymentState, bootstrapped: boolean, commit: { __typename?: 'Commit', hash: string, message: string } } | null, dependencies: Array<{ __typename?: 'Resource', type: string, name: string }>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }>, status: { __typename?: 'StatusSummary', health: HealthStatus, lastReportAt: string, lastReportSucceeded: boolean, openIncidentCount: number, worstOpenCategory?: IncidentCategory | null, consecutiveFailureCount: number }, incidents: Array<{ __typename?: 'Incident', id: string, category: IncidentCategory, openedAt: string, closedAt?: string | null, lastReportAt: string, reportCount: number, lastErrorMessage?: string | null }> } | null } } } };

export type CreateDeploymentMutationVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
  commitHash: Scalars['String']['input'];
}>;


export type CreateDeploymentMutation = { __typename?: 'Mutation', createDeployment: { __typename?: 'Deployment', id: string, nonce: string, state: DeploymentState } };

export type TearDownEnvironmentMutationVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
}>;


export type TearDownEnvironmentMutation = { __typename?: 'Mutation', tearDownEnvironment: boolean };

export type DeleteResourceMutationVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
  resource: Scalars['String']['input'];
}>;


export type DeleteResourceMutation = { __typename?: 'Mutation', deleteResource: boolean };

export type DeploymentDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
  id: Scalars['String']['input'];
}>;


export type DeploymentDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', deployment: { __typename?: 'Deployment', id: string, ref: string, createdAt: string, state: DeploymentState, bootstrapped: boolean, commit: { __typename?: 'Commit', hash: string, message: string }, status: { __typename?: 'StatusSummary', health: HealthStatus, lastReportAt: string, lastReportSucceeded: boolean, openIncidentCount: number, worstOpenCategory?: IncidentCategory | null, consecutiveFailureCount: number }, incidents: Array<{ __typename?: 'Incident', id: string, category: IncidentCategory, openedAt: string, closedAt?: string | null, lastReportAt: string, reportCount: number, lastErrorMessage?: string | null }>, resources: Array<{ __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, owner?: { __typename?: 'Deployment', id: string } | null, dependencies: Array<{ __typename?: 'Resource', type: string, name: string }>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }>, status: { __typename?: 'StatusSummary', health: HealthStatus, openIncidentCount: number, worstOpenCategory?: IncidentCategory | null } }>, lastLogs: Array<{ __typename?: 'Log', severity: Severity, timestamp: string, message: string }> } } } } };

export type OrganizationIncidentsQueryVariables = Exact<{
  org: Scalars['String']['input'];
  category?: InputMaybe<IncidentCategory>;
  openOnly?: InputMaybe<Scalars['Boolean']['input']>;
  since?: InputMaybe<Scalars['String']['input']>;
  until?: InputMaybe<Scalars['String']['input']>;
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
}>;


export type OrganizationIncidentsQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', name: string, incidents: Array<{ __typename?: 'Incident', id: string, entityQid: string, category: IncidentCategory, openedAt: string, closedAt?: string | null, lastReportAt: string, reportCount: number, lastErrorMessage?: string | null, repository?: { __typename?: 'Repository', name: string } | null, environment?: { __typename?: 'Environment', name: string } | null, deployment?: { __typename?: 'Deployment', id: string, ref: string, commit: { __typename?: 'Commit', hash: string } } | null, resource?: { __typename?: 'Resource', type: string, name: string } | null }> } };

export type OrganizationIncidentDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  id: Scalars['ID']['input'];
}>;


export type OrganizationIncidentDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', name: string, incident?: { __typename?: 'Incident', id: string, entityQid: string, category: IncidentCategory, openedAt: string, closedAt?: string | null, lastReportAt: string, reportCount: number, lastErrorMessage?: string | null, triggeringReportSummary?: string | null, repository?: { __typename?: 'Repository', name: string } | null, environment?: { __typename?: 'Environment', name: string } | null, deployment?: { __typename?: 'Deployment', id: string, ref: string, commit: { __typename?: 'Commit', hash: string, message: string } } | null, resource?: { __typename?: 'Resource', type: string, name: string } | null } | null } };

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


export type OrganizationsQuery = { __typename?: 'Query', organizations: Array<{ __typename?: 'Organization', name: string, members: Array<{ __typename?: 'User', username: string }>, repositories: Array<{ __typename?: 'Repository', name: string }> }> };

export type OrganizationDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
}>;


export type OrganizationDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', name: string, members: Array<{ __typename?: 'User', username: string }>, repositories: Array<{ __typename?: 'Repository', name: string, environments: Array<{ __typename?: 'Environment', name: string, qid: string, deployments: Array<{ __typename?: 'Deployment', id: string, ref: string, state: DeploymentState, commit: { __typename?: 'Commit', hash: string, message: string } }> }> }> } };

export type CreateOrganizationMutationVariables = Exact<{
  name: Scalars['String']['input'];
}>;


export type CreateOrganizationMutation = { __typename?: 'Mutation', createOrganization: { __typename?: 'Organization', name: string } };

export type AddOrganizationMemberMutationVariables = Exact<{
  organization: Scalars['String']['input'];
  username: Scalars['String']['input'];
}>;


export type AddOrganizationMemberMutation = { __typename?: 'Mutation', addOrganizationMember: { __typename?: 'Organization', name: string, members: Array<{ __typename?: 'User', username: string }> } };

export type CreateRepositoryMutationVariables = Exact<{
  organization: Scalars['String']['input'];
  repository: Scalars['String']['input'];
}>;


export type CreateRepositoryMutation = { __typename?: 'Mutation', createRepository: { __typename?: 'Repository', name: string } };

export type RepositoryDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
}>;


export type RepositoryDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', name: string, organization: { __typename?: 'Organization', name: string }, environments: Array<{ __typename?: 'Environment', name: string, qid: string, resources: Array<{ __typename?: 'Resource', name: string }>, deployments: Array<{ __typename?: 'Deployment', id: string, ref: string, state: DeploymentState, commit: { __typename?: 'Commit', hash: string, message: string } }> }> } } };

export type UserSettingsQueryVariables = Exact<{ [key: string]: never; }>;


export type UserSettingsQuery = { __typename?: 'Query', me: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string> } };

export type UpdateFullnameMutationVariables = Exact<{
  fullname: Scalars['String']['input'];
}>;


export type UpdateFullnameMutation = { __typename?: 'Mutation', updateFullname: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null } };

export type AddPublicKeyMutationVariables = Exact<{
  proof: Scalars['JSON']['input'];
}>;


export type AddPublicKeyMutation = { __typename?: 'Mutation', addPublicKey: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string> } };

export type RemovePublicKeyMutationVariables = Exact<{
  fingerprint: Scalars['String']['input'];
}>;


export type RemovePublicKeyMutation = { __typename?: 'Mutation', removePublicKey: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string> } };

export type CommitTreeEntryQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  commit: Scalars['String']['input'];
  path: Scalars['String']['input'];
}>;


export type CommitTreeEntryQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', commit: { __typename?: 'Commit', hash: string, message: string, parents: Array<{ __typename?: 'Commit', hash: string }>, treeEntry?: { __typename: 'Blob', hash: string, name?: string | null, size: number, content?: string | null } | { __typename: 'Tree', hash: string, name?: string | null, entries: Array<{ __typename: 'Blob', hash: string, name?: string | null, size: number } | { __typename: 'Tree', hash: string, name?: string | null }> } | null } } } };

export type CommitRootTreeQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  commit: Scalars['String']['input'];
}>;


export type CommitRootTreeQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', commit: { __typename?: 'Commit', hash: string, message: string, parents: Array<{ __typename?: 'Commit', hash: string }>, tree: { __typename?: 'Tree', hash: string, entries: Array<{ __typename: 'Blob', hash: string, name?: string | null, size: number } | { __typename: 'Tree', hash: string, name?: string | null }> } } } } };

export type CommitPageEnvironmentsQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
}>;


export type CommitPageEnvironmentsQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environments: Array<{ __typename?: 'Environment', name: string }> } } };


export const AuthChallengeDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"AuthChallenge"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"username"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"authChallenge"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"username"},"value":{"kind":"Variable","name":{"kind":"Name","value":"username"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"challenge"}},{"kind":"Field","name":{"kind":"Name","value":"taken"}},{"kind":"Field","name":{"kind":"Name","value":"passkeyRegistration"}},{"kind":"Field","name":{"kind":"Name","value":"passkeySignin"}}]}}]}}]} as unknown as DocumentNode<AuthChallengeQuery, AuthChallengeQueryVariables>;
export const SignInDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"SignIn"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"username"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"proof"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"JSON"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"signin"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"username"},"value":{"kind":"Variable","name":{"kind":"Name","value":"username"}}},{"kind":"Argument","name":{"kind":"Name","value":"proof"},"value":{"kind":"Variable","name":{"kind":"Name","value":"proof"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"user"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}},{"kind":"Field","name":{"kind":"Name","value":"token"}}]}}]}}]} as unknown as DocumentNode<SignInMutation, SignInMutationVariables>;
export const SignupDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Signup"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"username"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"email"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"proof"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"JSON"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"fullname"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"signup"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"username"},"value":{"kind":"Variable","name":{"kind":"Name","value":"username"}}},{"kind":"Argument","name":{"kind":"Name","value":"email"},"value":{"kind":"Variable","name":{"kind":"Name","value":"email"}}},{"kind":"Argument","name":{"kind":"Name","value":"proof"},"value":{"kind":"Variable","name":{"kind":"Name","value":"proof"}}},{"kind":"Argument","name":{"kind":"Name","value":"fullname"},"value":{"kind":"Variable","name":{"kind":"Name","value":"fullname"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"user"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}},{"kind":"Field","name":{"kind":"Name","value":"token"}}]}}]}}]} as unknown as DocumentNode<SignupMutation, SignupMutationVariables>;
export const RefreshTokenDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"RefreshToken"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"refreshToken"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"user"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}},{"kind":"Field","name":{"kind":"Name","value":"token"}}]}}]}}]} as unknown as DocumentNode<RefreshTokenQuery, RefreshTokenQueryVariables>;
export const MeDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"Me"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"me"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}}]}}]} as unknown as DocumentNode<MeQuery, MeQueryVariables>;
export const EnvironmentDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"EnvironmentDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"deployments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"nonce"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}},{"kind":"Field","name":{"kind":"Name","value":"createdAt"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"bootstrapped"}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportSucceeded"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}},{"kind":"Field","name":{"kind":"Name","value":"worstOpenCategory"}},{"kind":"Field","name":{"kind":"Name","value":"consecutiveFailureCount"}}]}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"lastLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"amount"},"value":{"kind":"IntValue","value":"20"}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"artifacts"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"mediaType"}},{"kind":"Field","name":{"kind":"Name","value":"url"}}]}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"owner"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"dependencies"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportSucceeded"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}},{"kind":"Field","name":{"kind":"Name","value":"worstOpenCategory"}},{"kind":"Field","name":{"kind":"Name","value":"consecutiveFailureCount"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<EnvironmentDetailQuery, EnvironmentDetailQueryVariables>;
export const ResourceDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"ResourceDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"resourceId"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"resource"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"id"},"value":{"kind":"Variable","name":{"kind":"Name","value":"resourceId"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"owner"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"nonce"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}},{"kind":"Field","name":{"kind":"Name","value":"createdAt"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"bootstrapped"}}]}},{"kind":"Field","name":{"kind":"Name","value":"dependencies"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportSucceeded"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}},{"kind":"Field","name":{"kind":"Name","value":"worstOpenCategory"}},{"kind":"Field","name":{"kind":"Name","value":"consecutiveFailureCount"}}]}},{"kind":"Field","name":{"kind":"Name","value":"incidents"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"limit"},"value":{"kind":"IntValue","value":"50"}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"category"}},{"kind":"Field","name":{"kind":"Name","value":"openedAt"}},{"kind":"Field","name":{"kind":"Name","value":"closedAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"reportCount"}},{"kind":"Field","name":{"kind":"Name","value":"lastErrorMessage"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<ResourceDetailQuery, ResourceDetailQueryVariables>;
export const CreateDeploymentDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"CreateDeployment"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"commitHash"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"createDeployment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"organization"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}},{"kind":"Argument","name":{"kind":"Name","value":"repository"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}},{"kind":"Argument","name":{"kind":"Name","value":"environment"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}},{"kind":"Argument","name":{"kind":"Name","value":"commitHash"},"value":{"kind":"Variable","name":{"kind":"Name","value":"commitHash"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"nonce"}},{"kind":"Field","name":{"kind":"Name","value":"state"}}]}}]}}]} as unknown as DocumentNode<CreateDeploymentMutation, CreateDeploymentMutationVariables>;
export const TearDownEnvironmentDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"TearDownEnvironment"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"tearDownEnvironment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"organization"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}},{"kind":"Argument","name":{"kind":"Name","value":"repository"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}},{"kind":"Argument","name":{"kind":"Name","value":"environment"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}]}]}}]} as unknown as DocumentNode<TearDownEnvironmentMutation, TearDownEnvironmentMutationVariables>;
export const DeleteResourceDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"DeleteResource"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"resource"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"deleteResource"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"organization"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}},{"kind":"Argument","name":{"kind":"Name","value":"repository"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}},{"kind":"Argument","name":{"kind":"Name","value":"environment"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}},{"kind":"Argument","name":{"kind":"Name","value":"resource"},"value":{"kind":"Variable","name":{"kind":"Name","value":"resource"}}}]}]}}]} as unknown as DocumentNode<DeleteResourceMutation, DeleteResourceMutationVariables>;
export const DeploymentDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"DeploymentDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"id"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"deployment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"id"},"value":{"kind":"Variable","name":{"kind":"Name","value":"id"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}},{"kind":"Field","name":{"kind":"Name","value":"createdAt"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"bootstrapped"}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportSucceeded"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}},{"kind":"Field","name":{"kind":"Name","value":"worstOpenCategory"}},{"kind":"Field","name":{"kind":"Name","value":"consecutiveFailureCount"}}]}},{"kind":"Field","name":{"kind":"Name","value":"incidents"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"limit"},"value":{"kind":"IntValue","value":"50"}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"category"}},{"kind":"Field","name":{"kind":"Name","value":"openedAt"}},{"kind":"Field","name":{"kind":"Name","value":"closedAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"reportCount"}},{"kind":"Field","name":{"kind":"Name","value":"lastErrorMessage"}}]}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"owner"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"dependencies"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}},{"kind":"Field","name":{"kind":"Name","value":"worstOpenCategory"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"lastLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"amount"},"value":{"kind":"IntValue","value":"20"}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<DeploymentDetailQuery, DeploymentDetailQueryVariables>;
export const OrganizationIncidentsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"OrganizationIncidents"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"category"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"IncidentCategory"}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"openOnly"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Boolean"}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"since"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"until"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"limit"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"offset"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"incidents"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"category"},"value":{"kind":"Variable","name":{"kind":"Name","value":"category"}}},{"kind":"Argument","name":{"kind":"Name","value":"openOnly"},"value":{"kind":"Variable","name":{"kind":"Name","value":"openOnly"}}},{"kind":"Argument","name":{"kind":"Name","value":"since"},"value":{"kind":"Variable","name":{"kind":"Name","value":"since"}}},{"kind":"Argument","name":{"kind":"Name","value":"until"},"value":{"kind":"Variable","name":{"kind":"Name","value":"until"}}},{"kind":"Argument","name":{"kind":"Name","value":"limit"},"value":{"kind":"Variable","name":{"kind":"Name","value":"limit"}}},{"kind":"Argument","name":{"kind":"Name","value":"offset"},"value":{"kind":"Variable","name":{"kind":"Name","value":"offset"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"entityQid"}},{"kind":"Field","name":{"kind":"Name","value":"category"}},{"kind":"Field","name":{"kind":"Name","value":"openedAt"}},{"kind":"Field","name":{"kind":"Name","value":"closedAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"reportCount"}},{"kind":"Field","name":{"kind":"Name","value":"lastErrorMessage"}},{"kind":"Field","name":{"kind":"Name","value":"repository"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"environment"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"deployment"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"resource"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]}}]}}]} as unknown as DocumentNode<OrganizationIncidentsQuery, OrganizationIncidentsQueryVariables>;
export const OrganizationIncidentDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"OrganizationIncidentDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"id"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"ID"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"incident"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"id"},"value":{"kind":"Variable","name":{"kind":"Name","value":"id"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"entityQid"}},{"kind":"Field","name":{"kind":"Name","value":"category"}},{"kind":"Field","name":{"kind":"Name","value":"openedAt"}},{"kind":"Field","name":{"kind":"Name","value":"closedAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"reportCount"}},{"kind":"Field","name":{"kind":"Name","value":"lastErrorMessage"}},{"kind":"Field","name":{"kind":"Name","value":"triggeringReportSummary"}},{"kind":"Field","name":{"kind":"Name","value":"repository"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"environment"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"deployment"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"resource"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]}}]}}]} as unknown as DocumentNode<OrganizationIncidentDetailQuery, OrganizationIncidentDetailQueryVariables>;
export const DeploymentLogsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"subscription","name":{"kind":"Name","value":"DeploymentLogs"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"deploymentId"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"deploymentLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"deploymentId"},"value":{"kind":"Variable","name":{"kind":"Name","value":"deploymentId"}}},{"kind":"Argument","name":{"kind":"Name","value":"initialAmount"},"value":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]} as unknown as DocumentNode<DeploymentLogsSubscription, DeploymentLogsSubscriptionVariables>;
export const EnvironmentLogsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"subscription","name":{"kind":"Name","value":"EnvironmentLogs"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"environmentQid"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environmentLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"environmentQid"},"value":{"kind":"Variable","name":{"kind":"Name","value":"environmentQid"}}},{"kind":"Argument","name":{"kind":"Name","value":"initialAmount"},"value":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]} as unknown as DocumentNode<EnvironmentLogsSubscription, EnvironmentLogsSubscriptionVariables>;
export const ResourceLogsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"subscription","name":{"kind":"Name","value":"ResourceLogs"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"resourceQid"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"resourceLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"resourceQid"},"value":{"kind":"Variable","name":{"kind":"Name","value":"resourceQid"}}},{"kind":"Argument","name":{"kind":"Name","value":"initialAmount"},"value":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]} as unknown as DocumentNode<ResourceLogsSubscription, ResourceLogsSubscriptionVariables>;
export const OrganizationsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"Organizations"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organizations"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"members"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}}]}},{"kind":"Field","name":{"kind":"Name","value":"repositories"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]}}]} as unknown as DocumentNode<OrganizationsQuery, OrganizationsQueryVariables>;
export const OrganizationDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"OrganizationDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"members"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}}]}},{"kind":"Field","name":{"kind":"Name","value":"repositories"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"environments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"deployments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<OrganizationDetailQuery, OrganizationDetailQueryVariables>;
export const CreateOrganizationDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"CreateOrganization"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"name"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"createOrganization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"name"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]} as unknown as DocumentNode<CreateOrganizationMutation, CreateOrganizationMutationVariables>;
export const AddOrganizationMemberDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"AddOrganizationMember"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"organization"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"username"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"addOrganizationMember"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"organization"},"value":{"kind":"Variable","name":{"kind":"Name","value":"organization"}}},{"kind":"Argument","name":{"kind":"Name","value":"username"},"value":{"kind":"Variable","name":{"kind":"Name","value":"username"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"members"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}}]}}]}}]}}]} as unknown as DocumentNode<AddOrganizationMemberMutation, AddOrganizationMemberMutationVariables>;
export const CreateRepositoryDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"CreateRepository"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"organization"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repository"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"createRepository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"organization"},"value":{"kind":"Variable","name":{"kind":"Name","value":"organization"}}},{"kind":"Argument","name":{"kind":"Name","value":"repository"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repository"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]} as unknown as DocumentNode<CreateRepositoryMutation, CreateRepositoryMutationVariables>;
export const RepositoryDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"RepositoryDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"organization"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"environments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"deployments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"ref"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<RepositoryDetailQuery, RepositoryDetailQueryVariables>;
export const UserSettingsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"UserSettings"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"me"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}}]}}]} as unknown as DocumentNode<UserSettingsQuery, UserSettingsQueryVariables>;
export const UpdateFullnameDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"UpdateFullname"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"fullname"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"updateFullname"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"fullname"},"value":{"kind":"Variable","name":{"kind":"Name","value":"fullname"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}}]}}]}}]} as unknown as DocumentNode<UpdateFullnameMutation, UpdateFullnameMutationVariables>;
export const AddPublicKeyDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"AddPublicKey"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"proof"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"JSON"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"addPublicKey"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"proof"},"value":{"kind":"Variable","name":{"kind":"Name","value":"proof"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}}]}}]} as unknown as DocumentNode<AddPublicKeyMutation, AddPublicKeyMutationVariables>;
export const RemovePublicKeyDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"RemovePublicKey"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"fingerprint"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"removePublicKey"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"fingerprint"},"value":{"kind":"Variable","name":{"kind":"Name","value":"fingerprint"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}}]}}]} as unknown as DocumentNode<RemovePublicKeyMutation, RemovePublicKeyMutationVariables>;
export const CommitTreeEntryDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"CommitTreeEntry"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"commit"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"path"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"commit"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"hash"},"value":{"kind":"Variable","name":{"kind":"Name","value":"commit"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}},{"kind":"Field","name":{"kind":"Name","value":"parents"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}}]}},{"kind":"Field","name":{"kind":"Name","value":"treeEntry"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"path"},"value":{"kind":"Variable","name":{"kind":"Name","value":"path"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Tree"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"entries"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Tree"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Blob"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"size"}}]}}]}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Blob"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"size"}},{"kind":"Field","name":{"kind":"Name","value":"content"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<CommitTreeEntryQuery, CommitTreeEntryQueryVariables>;
export const CommitRootTreeDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"CommitRootTree"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"commit"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"commit"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"hash"},"value":{"kind":"Variable","name":{"kind":"Name","value":"commit"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}},{"kind":"Field","name":{"kind":"Name","value":"parents"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}}]}},{"kind":"Field","name":{"kind":"Name","value":"tree"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"entries"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Tree"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Blob"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"size"}}]}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<CommitRootTreeQuery, CommitRootTreeQueryVariables>;
export const CommitPageEnvironmentsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"CommitPageEnvironments"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]}}]}}]} as unknown as DocumentNode<CommitPageEnvironmentsQuery, CommitPageEnvironmentsQueryVariables>;