//! gRPC service implementation. Dispatches to the auth helpers and the
//! UDB client; conversion between proto and udb types lives here.

use std::time::Duration;

use chrono::Utc;
use tonic::{Request, Response, Status};

use crate::auth::{RegistrationOutcome, verify_registration_proof, verify_signin};
use crate::challenge::Challenger;
use crate::proto;

/// How long a freshly-minted identity token is valid. Matches the
/// pre-IAS `IDENTITY_TOKEN_TTL` constant the API used; shortening this
/// requires shipping a refresh flow that doesn't depend on every edge
/// being able to reach the home-region IAS on every refresh.
const IDENTITY_TOKEN_TTL: Duration = Duration::from_secs(86400);

pub struct IasService {
    udb: udb::Client,
    challenger: Challenger,
}

impl IasService {
    pub fn new(udb: udb::Client, challenger: Challenger) -> Self {
        Self { udb, challenger }
    }
}

fn to_proto_user(u: udb::User) -> proto::User {
    proto::User {
        username: u.username,
        email: u.email,
        fullname: u.fullname,
    }
}

fn to_proto_org(o: udb::Org) -> proto::Org {
    proto::Org {
        name: o.name,
        creator: o.creator,
    }
}

fn to_proto_credential(c: udb::Credential) -> proto::Credential {
    proto::Credential {
        fingerprint: c.fingerprint,
        public_key: c.public_key,
        credential_id: c.credential_id,
        sign_count: c.sign_count,
    }
}

#[tonic::async_trait]
impl crate::IdentityAndAccess for IasService {
    async fn get_verifying_key(
        &self,
        _request: Request<()>,
    ) -> Result<Response<proto::VerifyingKey>, Status> {
        let key = self
            .udb
            .signing_public_key()
            .ok_or_else(|| Status::failed_precondition("IAS has no signing identity configured"))?;
        Ok(Response::new(proto::VerifyingKey {
            public_key: key.to_vec(),
        }))
    }

    async fn issue_challenge(
        &self,
        request: Request<proto::IssueChallengeRequest>,
    ) -> Result<Response<proto::IssueChallengeResponse>, Status> {
        let req = request.into_inner();
        let now = Utc::now();
        let challenge = self.challenger.challenge(now, &req.username);

        let user_client = self.udb.user(&req.username);
        let (user_taken, credentials) = match user_client.get().await {
            Ok(_) => (
                true,
                user_client
                    .pubkeys()
                    .list_credentials()
                    .await
                    .unwrap_or_default(),
            ),
            Err(udb::UserQueryError::NotFound) => (false, Vec::new()),
            Err(e) => {
                tracing::error!("Failed to look up user: {e}");
                return Err(Status::internal("Internal server error"));
            }
        };

        let webauthn_credential_ids = credentials
            .into_iter()
            .filter_map(|c| c.credential_id)
            .collect();

        Ok(Response::new(proto::IssueChallengeResponse {
            challenge,
            user_taken,
            webauthn_credential_ids,
        }))
    }

    async fn signup(
        &self,
        request: Request<proto::SignupRequest>,
    ) -> Result<Response<proto::TokenResponse>, Status> {
        let req = request.into_inner();
        let proof = req
            .proof
            .ok_or_else(|| Status::invalid_argument("missing proof"))?;
        let now = Utc::now();

        let RegistrationOutcome {
            openssh_key,
            credential_id,
            sign_count,
        } = verify_registration_proof(&self.challenger, &proof, &req.username, now)?;

        let user_client = self.udb.user(&req.username);
        let user = match user_client.register(req.email, req.fullname).await {
            Ok(u) => u,
            Err(udb::RegisterUserError::UsernameTaken) => {
                return Err(Status::already_exists("Username already taken"));
            }
            Err(udb::RegisterUserError::InvalidUsername(msg)) => {
                return Err(Status::invalid_argument(format!("Invalid username: {msg}")));
            }
            Err(udb::RegisterUserError::InvalidEmail(msg)) => {
                return Err(Status::invalid_argument(format!("Invalid email: {msg}")));
            }
            Err(e) => {
                tracing::error!("Failed to register user: {e}");
                return Err(Status::internal("Internal server error"));
            }
        };

        user_client
            .pubkeys()
            .add_credential(&openssh_key, credential_id.as_deref(), sign_count)
            .await
            .map_err(|e| {
                tracing::error!("Failed to add credential: {e}");
                Status::internal("Internal server error")
            })?;

        let token = user_client
            .issue_identity_token(IDENTITY_TOKEN_TTL)
            .map_err(|e| {
                tracing::error!("Failed to issue identity token: {e}");
                Status::internal("Internal server error")
            })?;

        Ok(Response::new(proto::TokenResponse {
            token,
            user: Some(to_proto_user(user)),
        }))
    }

    async fn signin(
        &self,
        request: Request<proto::SigninRequest>,
    ) -> Result<Response<proto::TokenResponse>, Status> {
        let req = request.into_inner();
        let proof = req
            .proof
            .ok_or_else(|| Status::invalid_argument("missing proof"))?;
        let now = Utc::now();

        let user_client = self.udb.user(&req.username);
        // Read the user record first so we have something to return on
        // success — and so that "user not found" looks the same as a bad
        // proof to the caller.
        let user = match user_client.get().await {
            Ok(u) => u,
            Err(udb::UserQueryError::NotFound) => {
                return Err(Status::unauthenticated("Invalid credentials"));
            }
            Err(e) => {
                tracing::error!("Failed to look up user: {e}");
                return Err(Status::internal("Internal server error"));
            }
        };

        verify_signin(&self.challenger, &user_client, &proof, &req.username, now).await?;

        let token = user_client
            .issue_identity_token(IDENTITY_TOKEN_TTL)
            .map_err(|e| {
                tracing::error!("Failed to issue identity token: {e}");
                Status::internal("Internal server error")
            })?;

        Ok(Response::new(proto::TokenResponse {
            token,
            user: Some(to_proto_user(user)),
        }))
    }

    async fn refresh_token(
        &self,
        request: Request<proto::RefreshTokenRequest>,
    ) -> Result<Response<proto::TokenResponse>, Status> {
        let req = request.into_inner();
        let user_client = self.udb.user(&req.username);
        let user = match user_client.get().await {
            Ok(u) => u,
            Err(udb::UserQueryError::NotFound) => return Err(Status::not_found("User not found")),
            Err(e) => {
                tracing::error!("Failed to look up user: {e}");
                return Err(Status::internal("Internal server error"));
            }
        };

        let token = user_client
            .issue_identity_token(IDENTITY_TOKEN_TTL)
            .map_err(|e| {
                tracing::error!("Failed to issue identity token: {e}");
                Status::internal("Internal server error")
            })?;

        Ok(Response::new(proto::TokenResponse {
            token,
            user: Some(to_proto_user(user)),
        }))
    }

    async fn add_credential(
        &self,
        request: Request<proto::AddCredentialRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let proof = req
            .proof
            .ok_or_else(|| Status::invalid_argument("missing proof"))?;
        let now = Utc::now();

        let RegistrationOutcome {
            openssh_key,
            credential_id,
            sign_count,
        } = verify_registration_proof(&self.challenger, &proof, &req.username, now)?;

        self.udb
            .user(&req.username)
            .pubkeys()
            .add_credential(&openssh_key, credential_id.as_deref(), sign_count)
            .await
            .map_err(|e| {
                tracing::error!("Failed to add credential: {e}");
                Status::internal("Internal server error")
            })?;

        Ok(Response::new(()))
    }

    async fn remove_credential(
        &self,
        request: Request<proto::RemoveCredentialRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        self.udb
            .user(&req.username)
            .pubkeys()
            .remove(&req.fingerprint)
            .await
            .map_err(|e| {
                tracing::error!("Failed to remove credential: {e}");
                Status::internal("Internal server error")
            })?;
        Ok(Response::new(()))
    }

    async fn list_credentials(
        &self,
        request: Request<proto::ListCredentialsRequest>,
    ) -> Result<Response<proto::ListCredentialsResponse>, Status> {
        let req = request.into_inner();
        let credentials = self
            .udb
            .user(&req.username)
            .pubkeys()
            .list_credentials()
            .await
            .map_err(|e| {
                tracing::error!("Failed to list credentials: {e}");
                Status::internal("Internal server error")
            })?;
        Ok(Response::new(proto::ListCredentialsResponse {
            credentials: credentials.into_iter().map(to_proto_credential).collect(),
        }))
    }

    async fn get_user(
        &self,
        request: Request<proto::GetUserRequest>,
    ) -> Result<Response<proto::User>, Status> {
        let req = request.into_inner();
        let user = match self.udb.user(&req.username).get().await {
            Ok(u) => u,
            Err(udb::UserQueryError::NotFound) => return Err(Status::not_found("User not found")),
            Err(e) => {
                tracing::error!("Failed to fetch user: {e}");
                return Err(Status::internal("Internal server error"));
            }
        };
        Ok(Response::new(to_proto_user(user)))
    }

    async fn update_fullname(
        &self,
        request: Request<proto::UpdateFullnameRequest>,
    ) -> Result<Response<proto::User>, Status> {
        let req = request.into_inner();
        let user_client = self.udb.user(&req.username);
        user_client.set_fullname(&req.fullname).await.map_err(|e| {
            tracing::error!("Failed to update fullname: {e}");
            Status::internal("Internal server error")
        })?;
        let user = user_client.get().await.map_err(|e| {
            tracing::error!("Failed to fetch user after fullname update: {e}");
            Status::internal("Internal server error")
        })?;
        Ok(Response::new(to_proto_user(user)))
    }

    async fn list_user_orgs(
        &self,
        request: Request<proto::ListUserOrgsRequest>,
    ) -> Result<Response<proto::ListOrgsResponse>, Status> {
        let req = request.into_inner();
        let org_names = self
            .udb
            .user(&req.username)
            .list_orgs()
            .await
            .map_err(|e| {
                tracing::error!("Failed to list orgs: {e}");
                Status::internal("Internal server error")
            })?;
        Ok(Response::new(proto::ListOrgsResponse { org_names }))
    }

    async fn create_org(
        &self,
        request: Request<proto::CreateOrgRequest>,
    ) -> Result<Response<proto::Org>, Status> {
        let req = request.into_inner();
        let org = match self.udb.org(&req.name).create(&req.creator).await {
            Ok(o) => o,
            Err(udb::CreateOrgError::NameTaken) => {
                return Err(Status::already_exists("Name already taken"));
            }
            Err(udb::CreateOrgError::InvalidName(msg)) => {
                return Err(Status::invalid_argument(format!("Invalid name: {msg}")));
            }
            Err(udb::CreateOrgError::CreatorNotFound) => {
                return Err(Status::not_found("User not found"));
            }
            Err(e) => {
                tracing::error!("Failed to create organization: {e}");
                return Err(Status::internal("Internal server error"));
            }
        };
        Ok(Response::new(to_proto_org(org)))
    }

    async fn get_org(
        &self,
        request: Request<proto::GetOrgRequest>,
    ) -> Result<Response<proto::Org>, Status> {
        let req = request.into_inner();
        let org = match self.udb.org(&req.name).get().await {
            Ok(o) => o,
            Err(udb::OrgQueryError::NotFound) => {
                return Err(Status::not_found("Organization not found"));
            }
            Err(e) => {
                tracing::error!("Failed to fetch org: {e}");
                return Err(Status::internal("Internal server error"));
            }
        };
        Ok(Response::new(to_proto_org(org)))
    }

    async fn list_org_members(
        &self,
        request: Request<proto::ListOrgMembersRequest>,
    ) -> Result<Response<proto::ListOrgMembersResponse>, Status> {
        let req = request.into_inner();
        let usernames = self
            .udb
            .org(&req.name)
            .members()
            .list()
            .await
            .map_err(|e| {
                tracing::error!("Failed to list org members: {e}");
                Status::internal("Internal server error")
            })?;
        Ok(Response::new(proto::ListOrgMembersResponse { usernames }))
    }

    async fn org_contains_member(
        &self,
        request: Request<proto::OrgContainsMemberRequest>,
    ) -> Result<Response<proto::BoolValue>, Status> {
        let req = request.into_inner();
        let value = self
            .udb
            .org(&req.name)
            .members()
            .contains(&req.username)
            .await
            .map_err(|e| {
                tracing::error!("Failed to check org membership: {e}");
                Status::internal("Internal server error")
            })?;
        Ok(Response::new(proto::BoolValue { value }))
    }

    async fn add_org_member(
        &self,
        request: Request<proto::AddOrgMemberRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        match self.udb.org(&req.name).members().add(&req.username).await {
            Ok(()) => Ok(Response::new(())),
            Err(udb::OrgQueryError::UserNotFound) => Err(Status::not_found("User not found")),
            Err(udb::OrgQueryError::AlreadyMember) => {
                Err(Status::already_exists("User is already a member"))
            }
            Err(udb::OrgQueryError::NotFound) => Err(Status::not_found("Organization not found")),
            Err(e) => {
                tracing::error!("Failed to add org member: {e}");
                Err(Status::internal("Internal server error"))
            }
        }
    }

    async fn remove_org_member(
        &self,
        request: Request<proto::RemoveOrgMemberRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        self.udb
            .org(&req.name)
            .members()
            .remove(&req.username)
            .await
            .map_err(|e| {
                tracing::error!("Failed to remove org member: {e}");
                Status::internal("Internal server error")
            })?;
        Ok(Response::new(()))
    }
}
