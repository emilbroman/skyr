mod receive_pack;
mod upload_pack;

use futures::{AsyncWriteExt, StreamExt};
use gix_ref::Reference;
use russh::{Channel, server};
use tokio_util::compat::TokioAsyncWriteCompatExt;

use cdb::DeploymentState;

pub(crate) struct CommandHandler<'a> {
    pub(crate) channel: &'a mut Channel<server::Msg>,
    pub(crate) _user: &'a udb::User,
    pub(crate) client: cdb::RepositoryClient,
}

impl<'a> CommandHandler<'a> {
    async fn advertise_refs(
        &self,
        server_caps: &[u8],
        mut refs: impl futures::Stream<Item = Reference> + Unpin,
    ) -> anyhow::Result<()> {
        let mut server_caps = Some(server_caps);

        let mut pkt =
            gix_packetline::async_io::Writer::new(self.channel.make_writer().compat_write());

        while let Some(reference) = refs.next().await {
            if let gix_ref::Target::Object(oid) = reference.target {
                let mut line = vec![];
                oid.write_hex_to(&mut line)?;
                line.push(b' ');
                line.extend_from_slice(reference.name.as_bstr().as_ref());
                if let Some(caps) = server_caps.take() {
                    line.push(b'\0');
                    line.extend_from_slice(caps);
                }
                line.push(b'\n');
                pkt.write_all(&line).await?;
            }
        }

        if let Some(caps) = server_caps.take() {
            let mut line = vec![];
            gix_hash::ObjectId::Sha1(Default::default()).write_hex_to(&mut line)?;
            line.extend_from_slice(b" capabilities^{}\0");
            line.extend_from_slice(caps);
            line.push(b'\n');
            pkt.write_all(&line).await?;
        }

        gix_packetline::async_io::encode::write_packet_line(
            &gix_packetline::PacketLineRef::Flush,
            pkt.inner_mut(),
        )
        .await?;
        pkt.flush().await?;

        Ok(())
    }

    async fn collect_refs(&self) -> anyhow::Result<Vec<Reference>> {
        let mut refs = vec![];

        let mut deployments = self.client.active_deployments().await?;
        while let Some(deployment) = deployments.next().await {
            let deployment = deployment?;

            // Filter out deployments that are not the current head of
            // their environment: Undesired/Lingering/Down are older
            // deployments being torn down or already terminated.
            // Only Desired deployments represent the environment's HEAD.
            if deployment.state != DeploymentState::Desired {
                continue;
            }

            let git_ref = deployment.environment.to_git_ref();
            let commit_oid =
                gix_hash::ObjectId::from_hex(deployment.deployment.commit.as_str().as_bytes())?;
            refs.push(Reference {
                name: gix_ref::FullName::try_from(git_ref.as_str())?,
                target: gix_ref::Target::Object(commit_oid),
                peeled: None,
            });
        }

        Ok(refs)
    }
}
