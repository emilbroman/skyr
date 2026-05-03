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

export type Deployment = IncidentEntity & {
  __typename?: 'Deployment';
  bootstrapped: Scalars['Boolean']['output'];
  commit: Commit;
  createdAt: Scalars['String']['output'];
  /**
   * Back-edge to the owning environment. The environment object is
   * constructed from the deployments currently in CDB.
   */
  environment: Environment;
  /**
   * Local identifier within the deployment's environment, in the form
   * `<commit-hash>.<nonce>`. This is what `Environment.deployment(id:)`
   * expects as its argument and what the web UI uses as the URL slug.
   */
  id: Scalars['String']['output'];
  lastLogs: Array<Log>;
  /** Currently-open incidents about this deployment. */
  openIncidents: Array<Incident>;
  /**
   * Globally-unique deployment identifier (the full QID, including org,
   * repo, and environment). Use this when you need a key that is unique
   * across the entire system — log subscriptions, status namespaces, etc.
   */
  qid: Scalars['ID']['output'];
  resources: Array<Resource>;
  /**
   * Compact, human-readable label of the form `<6-char commit>.<2-char
   * nonce>` — the first 6 characters of the commit hash followed by the
   * first 2 characters of the nonce (lowercase hex). Suitable for tight
   * UI surfaces where the full [`Self::id`] is too long.
   */
  shortId: Scalars['String']['output'];
  state: DeploymentState;
  /**
   * Per-deployment health rollup. **Self-only** — does not aggregate child
   * resource health. Resource statuses are reached via
   * `Deployment.resources -> Resource.status`.
   */
  status: StatusSummary;
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
  /**
   * Look up a deployment in this environment. If `id` is omitted, returns
   * the current deployment — the active deployment that has not been
   * superseded by another — or `null` if there is none. If `id` is
   * provided, returns the deployment with that `<commit-hash>.<nonce>`
   * identifier, or `null` if no such deployment exists.
   */
  deployment?: Maybe<Deployment>;
  deployments: Array<Deployment>;
  /**
   * Look up a single incident by id within this environment. Returns
   * `None` if no such incident exists in this environment.
   */
  incident?: Maybe<Incident>;
  /** Every incident in this environment, newest first. */
  incidents: Array<Incident>;
  lastLogs: Array<Log>;
  name: Scalars['String']['output'];
  qid: Scalars['String']['output'];
  resource?: Maybe<Resource>;
  resources: Array<Resource>;
};


export type EnvironmentDeploymentArgs = {
  id?: InputMaybe<Scalars['String']['input']>;
};


export type EnvironmentIncidentArgs = {
  id: Scalars['ID']['input'];
};


export type EnvironmentLastLogsArgs = {
  amount?: InputMaybe<Scalars['Int']['input']>;
};


export type EnvironmentResourceArgs = {
  id: Scalars['String']['input'];
};

/**
 * UI-friendly rolled-up health enum, computed from the underlying counters
 * in [`StatusSummary`]. `Down` if any open incident is at the `Crash`
 * severity tier, `Healthy` if there are no open incidents, `Degraded`
 * otherwise. The underlying severity tier is internal to the
 * status-reporting subsystem and not exposed in the API.
 */
export enum HealthStatus {
  Degraded = 'DEGRADED',
  Down = 'DOWN',
  Healthy = 'HEALTHY'
}

export type Incident = {
  __typename?: 'Incident';
  closedAt?: Maybe<Scalars['String']['output']>;
  /**
   * The deployment or resource this incident is about. `null` if the
   * underlying entity has since been destroyed.
   */
  entity?: Maybe<IncidentEntity>;
  /**
   * Back-edge to the owning environment. The environment object is
   * constructed from the deployments currently in CDB; if there are no
   * deployments left the returned `Environment` will have an empty
   * `deployments` list.
   */
  environment: Environment;
  id: Scalars['ID']['output'];
  lastReportAt: Scalars['String']['output'];
  openedAt: Scalars['String']['output'];
  /** Back-edge to the owning organization. */
  organization: Organization;
  reportCount: Scalars['Int']['output'];
  /** Back-edge to the owning repository. */
  repository: Repository;
  /**
   * The incident's projected summary: the union of distinct error
   * messages observed across all reports attributed to this incident, in
   * first-seen order, joined by `\n\n`. `null` if no error-bearing reports
   * have been recorded yet.
   */
  summary?: Maybe<Scalars['String']['output']>;
};

/**
 * Common shape of any entity an incident can be attached to. Currently
 * implemented by [`Resource`] and [`Deployment`]; the only field on the
 * interface is the entity's canonical QID, so type-specific data must be
 * reached through inline fragments. Each implementor exposes a `qid` field
 * of its own via its `graphql_object` impl, which juniper uses to satisfy
 * the interface contract — the trait below exists only to declare the
 * interface to the schema.
 */
export type IncidentEntity = {
  qid: Scalars['ID']['output'];
};

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
  region?: InputMaybe<Scalars['String']['input']>;
};


export type MutationCreateRepositoryArgs = {
  organization: Scalars['String']['input'];
  region?: InputMaybe<Scalars['String']['input']>;
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
  region: Scalars['String']['input'];
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
  members: Array<User>;
  name: Scalars['String']['output'];
  /** The Skyr region this organization belongs to — looked up in GDDB. */
  region: Region;
  repositories: Array<Repository>;
  repository: Repository;
};


export type OrganizationRepositoryArgs = {
  name: Scalars['String']['input'];
};

export type Query = {
  __typename?: 'Query';
  /**
   * Issue an authentication challenge for `username`.
   *
   * The challenge is owned by the user's home-region IAS (which holds
   * the salt). This edge resolves the home region in GDDB; for
   * already-registered users the GDDB entry tells us where to ask. For
   * brand-new signups (no GDDB entry yet), the caller must pass the
   * target signup region in `region`.
   */
  authChallenge: AuthChallenge;
  /**
   * Skyr regions this deployment makes available for new accounts and
   * orgs/repos. Configured by the operator via `--available-regions` /
   * `AVAILABLE_REGIONS`. Public so the web UI can render a region picker
   * before sign-in.
   */
  availableRegions: Array<Region>;
  health: Scalars['Boolean']['output'];
  me: SignedInUser;
  organization: Organization;
  organizations: Array<Organization>;
  refreshToken: AuthSuccess;
};


export type QueryAuthChallengeArgs = {
  region?: InputMaybe<Scalars['String']['input']>;
  username: Scalars['String']['input'];
};


export type QueryOrganizationArgs = {
  name: Scalars['String']['input'];
};

export type Region = {
  __typename?: 'Region';
  id: Scalars['String']['output'];
};

export type Repository = {
  __typename?: 'Repository';
  commit: Commit;
  environment: Environment;
  environments: Array<Environment>;
  name: Scalars['String']['output'];
  organization: Organization;
  /** The Skyr region this repository belongs to — looked up in GDDB. */
  region: Region;
};


export type RepositoryCommitArgs = {
  hash: Scalars['String']['input'];
};


export type RepositoryEnvironmentArgs = {
  name: Scalars['String']['input'];
};

export type Resource = IncidentEntity & {
  __typename?: 'Resource';
  dependencies: Array<Resource>;
  inputs?: Maybe<Scalars['JSON']['output']>;
  lastLogs: Array<Log>;
  markers: Array<ResourceMarker>;
  name: Scalars['String']['output'];
  /** Currently-open incidents about this resource. */
  openIncidents: Array<Incident>;
  outputs?: Maybe<Scalars['JSON']['output']>;
  owner?: Maybe<Deployment>;
  /**
   * Globally-unique resource identifier (the full QID, including
   * org/repo/env/type/name).
   */
  qid: Scalars['ID']['output'];
  /**
   * The Skyr region this resource lives in. Region is part of the
   * resource's structural identity (`<region>:<type>:<name>`).
   */
  region: Region;
  sourceTrace: Array<SourceFrame>;
  /** Per-resource health rollup. */
  status: StatusSummary;
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

export type SignedInUser = {
  __typename?: 'SignedInUser';
  email: Scalars['String']['output'];
  fullname?: Maybe<Scalars['String']['output']>;
  publicKeys: Array<Scalars['String']['output']>;
  /** The Skyr region this user belongs to. See [`User::region`]. */
  region: Region;
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
  /** The Skyr region this user belongs to — looked up in GDDB. */
  region: Region;
  username: Scalars['String']['output'];
};

export type AuthChallengeQueryVariables = Exact<{
  username: Scalars['String']['input'];
  region?: InputMaybe<Scalars['String']['input']>;
}>;


export type AuthChallengeQuery = { __typename?: 'Query', authChallenge: { __typename?: 'AuthChallenge', challenge: string, taken: boolean, passkeyRegistration: any, passkeySignin?: any | null } };

export type SignInMutationVariables = Exact<{
  username: Scalars['String']['input'];
  proof: Scalars['JSON']['input'];
}>;


export type SignInMutation = { __typename?: 'Mutation', signin: { __typename?: 'AuthSuccess', token: string, user: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string>, region: { __typename?: 'Region', id: string } } } };

export type SignupMutationVariables = Exact<{
  username: Scalars['String']['input'];
  email: Scalars['String']['input'];
  proof: Scalars['JSON']['input'];
  region: Scalars['String']['input'];
  fullname?: InputMaybe<Scalars['String']['input']>;
}>;


export type SignupMutation = { __typename?: 'Mutation', signup: { __typename?: 'AuthSuccess', token: string, user: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string>, region: { __typename?: 'Region', id: string } } } };

export type RefreshTokenQueryVariables = Exact<{ [key: string]: never; }>;


export type RefreshTokenQuery = { __typename?: 'Query', refreshToken: { __typename?: 'AuthSuccess', token: string, user: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string>, region: { __typename?: 'Region', id: string } } } };

export type MeQueryVariables = Exact<{ [key: string]: never; }>;


export type MeQuery = { __typename?: 'Query', me: { __typename?: 'SignedInUser', username: string, email: string, fullname?: string | null, publicKeys: Array<string>, region: { __typename?: 'Region', id: string } } };

export type EnvironmentDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
}>;


export type EnvironmentDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', name: string, qid: string, incidents: Array<{ __typename?: 'Incident', id: string, closedAt?: string | null }>, deployments: Array<{ __typename?: 'Deployment', id: string, shortId: string, createdAt: string, state: DeploymentState, bootstrapped: boolean, commit: { __typename?: 'Commit', hash: string, message: string }, status: { __typename?: 'StatusSummary', health: HealthStatus, lastReportAt: string, lastReportSucceeded: boolean, openIncidentCount: number, consecutiveFailureCount: number }, resources: Array<{ __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, region: { __typename?: 'Region', id: string }, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }> }>, lastLogs: Array<{ __typename?: 'Log', severity: Severity, timestamp: string, message: string }> }>, artifacts: Array<{ __typename?: 'Artifact', name: string, mediaType: string, url: string }>, resources: Array<{ __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, region: { __typename?: 'Region', id: string }, owner?: { __typename?: 'Deployment', id: string } | null, dependencies: Array<{ __typename?: 'Resource', type: string, name: string, region: { __typename?: 'Region', id: string } }>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }>, status: { __typename?: 'StatusSummary', health: HealthStatus, lastReportAt: string, lastReportSucceeded: boolean, openIncidentCount: number, consecutiveFailureCount: number } }> } } } };

export type ResourceDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
  resourceId: Scalars['String']['input'];
}>;


export type ResourceDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', qid: string, resource?: { __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, region: { __typename?: 'Region', id: string }, owner?: { __typename?: 'Deployment', id: string, shortId: string, createdAt: string, state: DeploymentState, bootstrapped: boolean, commit: { __typename?: 'Commit', hash: string, message: string } } | null, dependencies: Array<{ __typename?: 'Resource', type: string, name: string, region: { __typename?: 'Region', id: string } }>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }>, status: { __typename?: 'StatusSummary', health: HealthStatus, lastReportAt: string, lastReportSucceeded: boolean, openIncidentCount: number, consecutiveFailureCount: number }, openIncidents: Array<{ __typename?: 'Incident', id: string, openedAt: string, lastReportAt: string, reportCount: number, summary?: string | null }> } | null } } } };

export type CreateDeploymentMutationVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
  commitHash: Scalars['String']['input'];
}>;


export type CreateDeploymentMutation = { __typename?: 'Mutation', createDeployment: { __typename?: 'Deployment', id: string, state: DeploymentState } };

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


export type DeploymentDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', deployment?: { __typename?: 'Deployment', id: string, shortId: string, qid: string, createdAt: string, state: DeploymentState, bootstrapped: boolean, commit: { __typename?: 'Commit', hash: string, message: string }, status: { __typename?: 'StatusSummary', health: HealthStatus, lastReportAt: string, lastReportSucceeded: boolean, openIncidentCount: number, consecutiveFailureCount: number }, openIncidents: Array<{ __typename?: 'Incident', id: string, openedAt: string, lastReportAt: string, reportCount: number, summary?: string | null }>, resources: Array<{ __typename?: 'Resource', type: string, name: string, inputs?: any | null, outputs?: any | null, markers: Array<ResourceMarker>, region: { __typename?: 'Region', id: string }, owner?: { __typename?: 'Deployment', id: string } | null, dependencies: Array<{ __typename?: 'Resource', type: string, name: string, region: { __typename?: 'Region', id: string } }>, sourceTrace: Array<{ __typename?: 'SourceFrame', moduleId: string, span: string, name: string }>, status: { __typename?: 'StatusSummary', health: HealthStatus, openIncidentCount: number } }>, lastLogs: Array<{ __typename?: 'Log', severity: Severity, timestamp: string, message: string }> } | null } } } };

export type EnvironmentIncidentsQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
}>;


export type EnvironmentIncidentsQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', name: string, qid: string, incidents: Array<{ __typename?: 'Incident', id: string, openedAt: string, closedAt?: string | null, lastReportAt: string, reportCount: number, summary?: string | null, entity?: { __typename: 'Deployment', id: string, shortId: string, qid: string } | { __typename: 'Resource', type: string, name: string, qid: string } | null }> } } } };

export type EnvironmentIncidentDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
  env: Scalars['String']['input'];
  id: Scalars['ID']['input'];
}>;


export type EnvironmentIncidentDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', environment: { __typename?: 'Environment', name: string, qid: string, incident?: { __typename?: 'Incident', id: string, openedAt: string, closedAt?: string | null, lastReportAt: string, reportCount: number, summary?: string | null, entity?: { __typename: 'Deployment', id: string, shortId: string, qid: string } | { __typename: 'Resource', type: string, name: string, qid: string } | null } | null } } } };

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


export type OrganizationsQuery = { __typename?: 'Query', organizations: Array<{ __typename?: 'Organization', name: string, members: Array<{ __typename?: 'User', username: string, fullname?: string | null }>, repositories: Array<{ __typename?: 'Repository', name: string }> }> };

export type OrganizationDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
}>;


export type OrganizationDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', name: string, members: Array<{ __typename?: 'User', username: string, fullname?: string | null }>, repositories: Array<{ __typename?: 'Repository', name: string, environments: Array<{ __typename?: 'Environment', name: string, qid: string, currentDeployment?: { __typename?: 'Deployment', id: string, state: DeploymentState, status: { __typename?: 'StatusSummary', health: HealthStatus, openIncidentCount: number }, openIncidents: Array<{ __typename?: 'Incident', id: string, openedAt: string, lastReportAt: string, reportCount: number, summary?: string | null }> } | null, deployments: Array<{ __typename?: 'Deployment', id: string, shortId: string, state: DeploymentState, commit: { __typename?: 'Commit', hash: string, message: string } }> }> }> } };

export type CreateOrganizationMutationVariables = Exact<{
  name: Scalars['String']['input'];
  region?: InputMaybe<Scalars['String']['input']>;
}>;


export type CreateOrganizationMutation = { __typename?: 'Mutation', createOrganization: { __typename?: 'Organization', name: string } };

export type AddOrganizationMemberMutationVariables = Exact<{
  organization: Scalars['String']['input'];
  username: Scalars['String']['input'];
}>;


export type AddOrganizationMemberMutation = { __typename?: 'Mutation', addOrganizationMember: { __typename?: 'Organization', name: string, members: Array<{ __typename?: 'User', username: string, fullname?: string | null }> } };

export type AvailableRegionsQueryVariables = Exact<{ [key: string]: never; }>;


export type AvailableRegionsQuery = { __typename?: 'Query', availableRegions: Array<{ __typename?: 'Region', id: string }> };

export type CreateRepositoryMutationVariables = Exact<{
  organization: Scalars['String']['input'];
  repository: Scalars['String']['input'];
  region?: InputMaybe<Scalars['String']['input']>;
}>;


export type CreateRepositoryMutation = { __typename?: 'Mutation', createRepository: { __typename?: 'Repository', name: string } };

export type RepositoryDetailQueryVariables = Exact<{
  org: Scalars['String']['input'];
  repo: Scalars['String']['input'];
}>;


export type RepositoryDetailQuery = { __typename?: 'Query', organization: { __typename?: 'Organization', repository: { __typename?: 'Repository', name: string, organization: { __typename?: 'Organization', name: string }, environments: Array<{ __typename?: 'Environment', name: string, qid: string, resources: Array<{ __typename?: 'Resource', name: string }>, deployments: Array<{ __typename?: 'Deployment', id: string, shortId: string, state: DeploymentState, commit: { __typename?: 'Commit', hash: string, message: string } }> }> } } };

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


export const AuthChallengeDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"AuthChallenge"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"username"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"region"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"authChallenge"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"username"},"value":{"kind":"Variable","name":{"kind":"Name","value":"username"}}},{"kind":"Argument","name":{"kind":"Name","value":"region"},"value":{"kind":"Variable","name":{"kind":"Name","value":"region"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"challenge"}},{"kind":"Field","name":{"kind":"Name","value":"taken"}},{"kind":"Field","name":{"kind":"Name","value":"passkeyRegistration"}},{"kind":"Field","name":{"kind":"Name","value":"passkeySignin"}}]}}]}}]} as unknown as DocumentNode<AuthChallengeQuery, AuthChallengeQueryVariables>;
export const SignInDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"SignIn"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"username"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"proof"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"JSON"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"signin"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"username"},"value":{"kind":"Variable","name":{"kind":"Name","value":"username"}}},{"kind":"Argument","name":{"kind":"Name","value":"proof"},"value":{"kind":"Variable","name":{"kind":"Name","value":"proof"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"user"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}},{"kind":"Field","name":{"kind":"Name","value":"token"}}]}}]}}]} as unknown as DocumentNode<SignInMutation, SignInMutationVariables>;
export const SignupDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Signup"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"username"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"email"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"proof"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"JSON"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"region"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"fullname"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"signup"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"username"},"value":{"kind":"Variable","name":{"kind":"Name","value":"username"}}},{"kind":"Argument","name":{"kind":"Name","value":"email"},"value":{"kind":"Variable","name":{"kind":"Name","value":"email"}}},{"kind":"Argument","name":{"kind":"Name","value":"proof"},"value":{"kind":"Variable","name":{"kind":"Name","value":"proof"}}},{"kind":"Argument","name":{"kind":"Name","value":"region"},"value":{"kind":"Variable","name":{"kind":"Name","value":"region"}}},{"kind":"Argument","name":{"kind":"Name","value":"fullname"},"value":{"kind":"Variable","name":{"kind":"Name","value":"fullname"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"user"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}},{"kind":"Field","name":{"kind":"Name","value":"token"}}]}}]}}]} as unknown as DocumentNode<SignupMutation, SignupMutationVariables>;
export const RefreshTokenDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"RefreshToken"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"refreshToken"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"user"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}},{"kind":"Field","name":{"kind":"Name","value":"token"}}]}}]}}]} as unknown as DocumentNode<RefreshTokenQuery, RefreshTokenQueryVariables>;
export const MeDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"Me"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"me"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}}]}}]} as unknown as DocumentNode<MeQuery, MeQueryVariables>;
export const EnvironmentDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"EnvironmentDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"incidents"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"closedAt"}}]}},{"kind":"Field","name":{"kind":"Name","value":"deployments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"shortId"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}},{"kind":"Field","name":{"kind":"Name","value":"createdAt"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"bootstrapped"}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportSucceeded"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}},{"kind":"Field","name":{"kind":"Name","value":"consecutiveFailureCount"}}]}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"lastLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"amount"},"value":{"kind":"IntValue","value":"20"}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"artifacts"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"mediaType"}},{"kind":"Field","name":{"kind":"Name","value":"url"}}]}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"owner"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"dependencies"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportSucceeded"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}},{"kind":"Field","name":{"kind":"Name","value":"consecutiveFailureCount"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<EnvironmentDetailQuery, EnvironmentDetailQueryVariables>;
export const ResourceDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"ResourceDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"resourceId"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"resource"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"id"},"value":{"kind":"Variable","name":{"kind":"Name","value":"resourceId"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"owner"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"shortId"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}},{"kind":"Field","name":{"kind":"Name","value":"createdAt"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"bootstrapped"}}]}},{"kind":"Field","name":{"kind":"Name","value":"dependencies"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportSucceeded"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}},{"kind":"Field","name":{"kind":"Name","value":"consecutiveFailureCount"}}]}},{"kind":"Field","name":{"kind":"Name","value":"openIncidents"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"openedAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"reportCount"}},{"kind":"Field","name":{"kind":"Name","value":"summary"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<ResourceDetailQuery, ResourceDetailQueryVariables>;
export const CreateDeploymentDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"CreateDeployment"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"commitHash"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"createDeployment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"organization"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}},{"kind":"Argument","name":{"kind":"Name","value":"repository"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}},{"kind":"Argument","name":{"kind":"Name","value":"environment"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}},{"kind":"Argument","name":{"kind":"Name","value":"commitHash"},"value":{"kind":"Variable","name":{"kind":"Name","value":"commitHash"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"state"}}]}}]}}]} as unknown as DocumentNode<CreateDeploymentMutation, CreateDeploymentMutationVariables>;
export const TearDownEnvironmentDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"TearDownEnvironment"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"tearDownEnvironment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"organization"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}},{"kind":"Argument","name":{"kind":"Name","value":"repository"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}},{"kind":"Argument","name":{"kind":"Name","value":"environment"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}]}]}}]} as unknown as DocumentNode<TearDownEnvironmentMutation, TearDownEnvironmentMutationVariables>;
export const DeleteResourceDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"DeleteResource"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"resource"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"deleteResource"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"organization"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}},{"kind":"Argument","name":{"kind":"Name","value":"repository"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}},{"kind":"Argument","name":{"kind":"Name","value":"environment"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}},{"kind":"Argument","name":{"kind":"Name","value":"resource"},"value":{"kind":"Variable","name":{"kind":"Name","value":"resource"}}}]}]}}]} as unknown as DocumentNode<DeleteResourceMutation, DeleteResourceMutationVariables>;
export const DeploymentDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"DeploymentDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"id"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"deployment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"id"},"value":{"kind":"Variable","name":{"kind":"Name","value":"id"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"shortId"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}},{"kind":"Field","name":{"kind":"Name","value":"createdAt"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"bootstrapped"}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportSucceeded"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}},{"kind":"Field","name":{"kind":"Name","value":"consecutiveFailureCount"}}]}},{"kind":"Field","name":{"kind":"Name","value":"openIncidents"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"openedAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"reportCount"}},{"kind":"Field","name":{"kind":"Name","value":"summary"}}]}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"inputs"}},{"kind":"Field","name":{"kind":"Name","value":"outputs"}},{"kind":"Field","name":{"kind":"Name","value":"owner"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"dependencies"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"region"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"markers"}},{"kind":"Field","name":{"kind":"Name","value":"sourceTrace"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"moduleId"}},{"kind":"Field","name":{"kind":"Name","value":"span"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"lastLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"amount"},"value":{"kind":"IntValue","value":"20"}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<DeploymentDetailQuery, DeploymentDetailQueryVariables>;
export const EnvironmentIncidentsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"EnvironmentIncidents"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"incidents"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"openedAt"}},{"kind":"Field","name":{"kind":"Name","value":"closedAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"reportCount"}},{"kind":"Field","name":{"kind":"Name","value":"summary"}},{"kind":"Field","name":{"kind":"Name","value":"entity"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Resource"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Deployment"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"shortId"}}]}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<EnvironmentIncidentsQuery, EnvironmentIncidentsQueryVariables>;
export const EnvironmentIncidentDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"EnvironmentIncidentDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"env"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"id"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"ID"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environment"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"env"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"incident"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"id"},"value":{"kind":"Variable","name":{"kind":"Name","value":"id"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"openedAt"}},{"kind":"Field","name":{"kind":"Name","value":"closedAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"reportCount"}},{"kind":"Field","name":{"kind":"Name","value":"summary"}},{"kind":"Field","name":{"kind":"Name","value":"entity"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Resource"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"type"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Deployment"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"shortId"}}]}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<EnvironmentIncidentDetailQuery, EnvironmentIncidentDetailQueryVariables>;
export const DeploymentLogsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"subscription","name":{"kind":"Name","value":"DeploymentLogs"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"deploymentId"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"deploymentLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"deploymentId"},"value":{"kind":"Variable","name":{"kind":"Name","value":"deploymentId"}}},{"kind":"Argument","name":{"kind":"Name","value":"initialAmount"},"value":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]} as unknown as DocumentNode<DeploymentLogsSubscription, DeploymentLogsSubscriptionVariables>;
export const EnvironmentLogsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"subscription","name":{"kind":"Name","value":"EnvironmentLogs"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"environmentQid"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environmentLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"environmentQid"},"value":{"kind":"Variable","name":{"kind":"Name","value":"environmentQid"}}},{"kind":"Argument","name":{"kind":"Name","value":"initialAmount"},"value":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]} as unknown as DocumentNode<EnvironmentLogsSubscription, EnvironmentLogsSubscriptionVariables>;
export const ResourceLogsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"subscription","name":{"kind":"Name","value":"ResourceLogs"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"resourceQid"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"Int"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"resourceLogs"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"resourceQid"},"value":{"kind":"Variable","name":{"kind":"Name","value":"resourceQid"}}},{"kind":"Argument","name":{"kind":"Name","value":"initialAmount"},"value":{"kind":"Variable","name":{"kind":"Name","value":"initialAmount"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"severity"}},{"kind":"Field","name":{"kind":"Name","value":"timestamp"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]} as unknown as DocumentNode<ResourceLogsSubscription, ResourceLogsSubscriptionVariables>;
export const OrganizationsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"Organizations"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organizations"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"members"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}}]}},{"kind":"Field","name":{"kind":"Name","value":"repositories"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]}}]} as unknown as DocumentNode<OrganizationsQuery, OrganizationsQueryVariables>;
export const OrganizationDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"OrganizationDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"members"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}}]}},{"kind":"Field","name":{"kind":"Name","value":"repositories"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"environments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","alias":{"kind":"Name","value":"currentDeployment"},"name":{"kind":"Name","value":"deployment"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"status"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"health"}},{"kind":"Field","name":{"kind":"Name","value":"openIncidentCount"}}]}},{"kind":"Field","name":{"kind":"Name","value":"openIncidents"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"openedAt"}},{"kind":"Field","name":{"kind":"Name","value":"lastReportAt"}},{"kind":"Field","name":{"kind":"Name","value":"reportCount"}},{"kind":"Field","name":{"kind":"Name","value":"summary"}}]}}]}},{"kind":"Field","name":{"kind":"Name","value":"deployments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"shortId"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<OrganizationDetailQuery, OrganizationDetailQueryVariables>;
export const CreateOrganizationDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"CreateOrganization"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"name"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"region"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"createOrganization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"name"}}},{"kind":"Argument","name":{"kind":"Name","value":"region"},"value":{"kind":"Variable","name":{"kind":"Name","value":"region"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]} as unknown as DocumentNode<CreateOrganizationMutation, CreateOrganizationMutationVariables>;
export const AddOrganizationMemberDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"AddOrganizationMember"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"organization"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"username"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"addOrganizationMember"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"organization"},"value":{"kind":"Variable","name":{"kind":"Name","value":"organization"}}},{"kind":"Argument","name":{"kind":"Name","value":"username"},"value":{"kind":"Variable","name":{"kind":"Name","value":"username"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"members"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}}]}}]}}]}}]} as unknown as DocumentNode<AddOrganizationMemberMutation, AddOrganizationMemberMutationVariables>;
export const AvailableRegionsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"AvailableRegions"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"availableRegions"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}}]}}]}}]} as unknown as DocumentNode<AvailableRegionsQuery, AvailableRegionsQueryVariables>;
export const CreateRepositoryDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"CreateRepository"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"organization"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repository"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"region"}},"type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"createRepository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"organization"},"value":{"kind":"Variable","name":{"kind":"Name","value":"organization"}}},{"kind":"Argument","name":{"kind":"Name","value":"repository"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repository"}}},{"kind":"Argument","name":{"kind":"Name","value":"region"},"value":{"kind":"Variable","name":{"kind":"Name","value":"region"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]} as unknown as DocumentNode<CreateRepositoryMutation, CreateRepositoryMutationVariables>;
export const RepositoryDetailDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"RepositoryDetail"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"organization"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"environments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"qid"}},{"kind":"Field","name":{"kind":"Name","value":"resources"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"Field","name":{"kind":"Name","value":"deployments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"id"}},{"kind":"Field","name":{"kind":"Name","value":"shortId"}},{"kind":"Field","name":{"kind":"Name","value":"state"}},{"kind":"Field","name":{"kind":"Name","value":"commit"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<RepositoryDetailQuery, RepositoryDetailQueryVariables>;
export const UserSettingsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"UserSettings"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"me"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}}]}}]} as unknown as DocumentNode<UserSettingsQuery, UserSettingsQueryVariables>;
export const UpdateFullnameDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"UpdateFullname"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"fullname"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"updateFullname"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"fullname"},"value":{"kind":"Variable","name":{"kind":"Name","value":"fullname"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}}]}}]}}]} as unknown as DocumentNode<UpdateFullnameMutation, UpdateFullnameMutationVariables>;
export const AddPublicKeyDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"AddPublicKey"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"proof"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"JSON"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"addPublicKey"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"proof"},"value":{"kind":"Variable","name":{"kind":"Name","value":"proof"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}}]}}]} as unknown as DocumentNode<AddPublicKeyMutation, AddPublicKeyMutationVariables>;
export const RemovePublicKeyDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"RemovePublicKey"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"fingerprint"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"removePublicKey"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"fingerprint"},"value":{"kind":"Variable","name":{"kind":"Name","value":"fingerprint"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"username"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"fullname"}},{"kind":"Field","name":{"kind":"Name","value":"publicKeys"}}]}}]}}]} as unknown as DocumentNode<RemovePublicKeyMutation, RemovePublicKeyMutationVariables>;
export const CommitTreeEntryDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"CommitTreeEntry"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"commit"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"path"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"commit"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"hash"},"value":{"kind":"Variable","name":{"kind":"Name","value":"commit"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}},{"kind":"Field","name":{"kind":"Name","value":"parents"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}}]}},{"kind":"Field","name":{"kind":"Name","value":"treeEntry"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"path"},"value":{"kind":"Variable","name":{"kind":"Name","value":"path"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Tree"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"entries"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Tree"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Blob"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"size"}}]}}]}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Blob"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"size"}},{"kind":"Field","name":{"kind":"Name","value":"content"}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<CommitTreeEntryQuery, CommitTreeEntryQueryVariables>;
export const CommitRootTreeDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"CommitRootTree"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"commit"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"commit"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"hash"},"value":{"kind":"Variable","name":{"kind":"Name","value":"commit"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"message"}},{"kind":"Field","name":{"kind":"Name","value":"parents"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}}]}},{"kind":"Field","name":{"kind":"Name","value":"tree"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"entries"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Tree"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}}]}},{"kind":"InlineFragment","typeCondition":{"kind":"NamedType","name":{"kind":"Name","value":"Blob"}},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"__typename"}},{"kind":"Field","name":{"kind":"Name","value":"hash"}},{"kind":"Field","name":{"kind":"Name","value":"name"}},{"kind":"Field","name":{"kind":"Name","value":"size"}}]}}]}}]}}]}}]}}]}}]}}]} as unknown as DocumentNode<CommitRootTreeQuery, CommitRootTreeQueryVariables>;
export const CommitPageEnvironmentsDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"CommitPageEnvironments"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"org"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}},{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"repo"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"organization"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"org"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"repository"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"name"},"value":{"kind":"Variable","name":{"kind":"Name","value":"repo"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"environments"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"name"}}]}}]}}]}}]}}]} as unknown as DocumentNode<CommitPageEnvironmentsQuery, CommitPageEnvironmentsQueryVariables>;