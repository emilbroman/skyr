use redis::{AsyncCommands, Client as RedisClient};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ARecordData {
    pub addresses: Vec<String>,
    pub ttl_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AAAARecordData {
    pub addresses: Vec<String>,
    pub ttl_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CNAMERecordData {
    pub target: String,
    pub ttl_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TXTRecordData {
    pub values: Vec<String>,
    pub ttl_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MXExchange {
    pub priority: u16,
    pub host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MXRecordData {
    pub exchanges: Vec<MXExchange>,
    pub ttl_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SRVTarget {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SRVRecordData {
    pub records: Vec<SRVTarget>,
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

    fn a_key(fqdn: &str) -> String {
        format!("dns:a:{}", fqdn.to_lowercase())
    }

    fn aaaa_key(fqdn: &str) -> String {
        format!("dns:aaaa:{}", fqdn.to_lowercase())
    }

    fn cname_key(fqdn: &str) -> String {
        format!("dns:cname:{}", fqdn.to_lowercase())
    }

    fn txt_key(fqdn: &str) -> String {
        format!("dns:txt:{}", fqdn.to_lowercase())
    }

    fn mx_key(fqdn: &str) -> String {
        format!("dns:mx:{}", fqdn.to_lowercase())
    }

    fn srv_key(fqdn: &str) -> String {
        format!("dns:srv:{}", fqdn.to_lowercase())
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
        let _: () = self.conn.clone().set(Self::a_key(fqdn), json).await?;
        Ok(())
    }

    pub async fn delete_a_record(&self, fqdn: &str) -> anyhow::Result<()> {
        let _: () = self.conn.clone().del(Self::a_key(fqdn)).await?;
        Ok(())
    }

    pub async fn get_a_record(&self, fqdn: &str) -> anyhow::Result<Option<ARecordData>> {
        let data: Option<String> = self.conn.clone().get(Self::a_key(fqdn)).await?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    pub async fn set_aaaa_record(
        &self,
        fqdn: &str,
        addresses: &[String],
        ttl_seconds: u32,
    ) -> anyhow::Result<()> {
        let data = AAAARecordData {
            addresses: addresses.to_vec(),
            ttl_seconds,
        };
        let json = serde_json::to_string(&data)?;
        let _: () = self.conn.clone().set(Self::aaaa_key(fqdn), json).await?;
        Ok(())
    }

    pub async fn delete_aaaa_record(&self, fqdn: &str) -> anyhow::Result<()> {
        let _: () = self.conn.clone().del(Self::aaaa_key(fqdn)).await?;
        Ok(())
    }

    pub async fn get_aaaa_record(&self, fqdn: &str) -> anyhow::Result<Option<AAAARecordData>> {
        let data: Option<String> = self.conn.clone().get(Self::aaaa_key(fqdn)).await?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    pub async fn set_cname_record(
        &self,
        fqdn: &str,
        target: &str,
        ttl_seconds: u32,
    ) -> anyhow::Result<()> {
        let data = CNAMERecordData {
            target: target.to_string(),
            ttl_seconds,
        };
        let json = serde_json::to_string(&data)?;
        let _: () = self.conn.clone().set(Self::cname_key(fqdn), json).await?;
        Ok(())
    }

    pub async fn delete_cname_record(&self, fqdn: &str) -> anyhow::Result<()> {
        let _: () = self.conn.clone().del(Self::cname_key(fqdn)).await?;
        Ok(())
    }

    pub async fn get_cname_record(&self, fqdn: &str) -> anyhow::Result<Option<CNAMERecordData>> {
        let data: Option<String> = self.conn.clone().get(Self::cname_key(fqdn)).await?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    pub async fn set_txt_record(
        &self,
        fqdn: &str,
        values: &[String],
        ttl_seconds: u32,
    ) -> anyhow::Result<()> {
        let data = TXTRecordData {
            values: values.to_vec(),
            ttl_seconds,
        };
        let json = serde_json::to_string(&data)?;
        let _: () = self.conn.clone().set(Self::txt_key(fqdn), json).await?;
        Ok(())
    }

    pub async fn delete_txt_record(&self, fqdn: &str) -> anyhow::Result<()> {
        let _: () = self.conn.clone().del(Self::txt_key(fqdn)).await?;
        Ok(())
    }

    pub async fn get_txt_record(&self, fqdn: &str) -> anyhow::Result<Option<TXTRecordData>> {
        let data: Option<String> = self.conn.clone().get(Self::txt_key(fqdn)).await?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    pub async fn set_mx_record(
        &self,
        fqdn: &str,
        exchanges: &[MXExchange],
        ttl_seconds: u32,
    ) -> anyhow::Result<()> {
        let data = MXRecordData {
            exchanges: exchanges.to_vec(),
            ttl_seconds,
        };
        let json = serde_json::to_string(&data)?;
        let _: () = self.conn.clone().set(Self::mx_key(fqdn), json).await?;
        Ok(())
    }

    pub async fn delete_mx_record(&self, fqdn: &str) -> anyhow::Result<()> {
        let _: () = self.conn.clone().del(Self::mx_key(fqdn)).await?;
        Ok(())
    }

    pub async fn get_mx_record(&self, fqdn: &str) -> anyhow::Result<Option<MXRecordData>> {
        let data: Option<String> = self.conn.clone().get(Self::mx_key(fqdn)).await?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    pub async fn set_srv_record(
        &self,
        fqdn: &str,
        records: &[SRVTarget],
        ttl_seconds: u32,
    ) -> anyhow::Result<()> {
        let data = SRVRecordData {
            records: records.to_vec(),
            ttl_seconds,
        };
        let json = serde_json::to_string(&data)?;
        let _: () = self.conn.clone().set(Self::srv_key(fqdn), json).await?;
        Ok(())
    }

    pub async fn delete_srv_record(&self, fqdn: &str) -> anyhow::Result<()> {
        let _: () = self.conn.clone().del(Self::srv_key(fqdn)).await?;
        Ok(())
    }

    pub async fn get_srv_record(&self, fqdn: &str) -> anyhow::Result<Option<SRVRecordData>> {
        let data: Option<String> = self.conn.clone().get(Self::srv_key(fqdn)).await?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }
}
