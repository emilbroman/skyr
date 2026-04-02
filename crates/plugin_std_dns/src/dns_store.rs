use redis::{AsyncCommands, Client as RedisClient};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ARecordData {
    pub addresses: Vec<String>,
    pub ttl_seconds: u32,
}

#[derive(Clone)]
pub struct DnsStore {
    conn: redis::aio::MultiplexedConnection,
}

impl DnsStore {
    pub async fn connect(hostname: &str) -> anyhow::Result<Self> {
        let url = format!("redis://{hostname}/");
        let client = RedisClient::open(url)?;
        let conn = client.get_multiplexed_async_connection().await?;
        Ok(Self { conn })
    }

    fn key(fqdn: &str) -> String {
        format!("dns:a:{}", fqdn.to_lowercase())
    }

    pub async fn set_a_record(
        &self,
        fqdn: &str,
        addresses: &[String],
        ttl_seconds: u32,
    ) -> anyhow::Result<()> {
        let data = ARecordData {
            addresses: addresses.to_vec(),
            ttl_seconds,
        };
        let json = serde_json::to_string(&data)?;
        let _: () = self.conn.clone().set(Self::key(fqdn), json).await?;
        Ok(())
    }

    pub async fn delete_a_record(&self, fqdn: &str) -> anyhow::Result<()> {
        let _: () = self.conn.clone().del(Self::key(fqdn)).await?;
        Ok(())
    }

    pub async fn get_a_record(&self, fqdn: &str) -> anyhow::Result<Option<ARecordData>> {
        let data: Option<String> = self.conn.clone().get(Self::key(fqdn)).await?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }
}
