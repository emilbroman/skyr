use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, Months, Utc};
use clap::Parser;
use sclc::ValueAssertions;

const CLOCK_RESOURCE_TYPE: &str = "Std/Time.Clock";

#[derive(Parser)]
struct Args {
    #[arg(long)]
    bind: String,
}

struct TimePlugin;

impl TimePlugin {
    fn new() -> Self {
        Self
    }

    fn clock_resource(&self, inputs: sclc::Record) -> anyhow::Result<sclc::Resource> {
        let months = *inputs.get("months").assert_int_ref()?;
        let milliseconds = *inputs.get("milliseconds").assert_int_ref()?;

        if months <= 0 && milliseconds <= 0 {
            anyhow::bail!(
                "Clock duration must be positive: \
                 at least one of months or milliseconds must be greater than zero"
            );
        }
        if months < 0 {
            anyhow::bail!("Clock months must be non-negative, got {months}");
        }
        if milliseconds < 0 {
            anyhow::bail!("Clock milliseconds must be non-negative, got {milliseconds}");
        }

        let now = Utc::now();
        let boundary = truncate_to_boundary(now, months, milliseconds)?;

        let mut outputs = sclc::Record::default();
        outputs.insert(
            String::from("epochMillis"),
            sclc::Value::Int(boundary.timestamp_millis()),
        );

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: BTreeSet::from([sclc::Marker::Volatile]),
        })
    }
}

/// Compute months since the Unix epoch for a given datetime.
fn months_since_epoch(dt: &DateTime<Utc>) -> i64 {
    (dt.year() - 1970) as i64 * 12 + (dt.month() as i64 - 1)
}

/// Truncate `now` to the closest past boundary of a duration defined by
/// `months` and `milliseconds`, aligned with the Unix epoch.
///
/// The month component is aligned to calendar months from epoch (1970-01-01).
/// The millisecond component is then aligned from that month boundary.
fn truncate_to_boundary(
    now: DateTime<Utc>,
    months: i64,
    milliseconds: i64,
) -> anyhow::Result<DateTime<Utc>> {
    // Start from the epoch
    let epoch =
        DateTime::from_timestamp_millis(0).expect("Unix epoch (0ms) is always a valid timestamp");

    // Step 1: Find the month boundary
    let month_boundary = if months > 0 {
        let now_months_since_epoch = months_since_epoch(&now);
        // How many complete month-intervals have passed
        let intervals = now_months_since_epoch / months;
        // The month boundary is intervals * months from epoch
        let boundary_months = intervals
            .checked_mul(months)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| anyhow::anyhow!("month overflow computing clock boundary"))?;
        epoch
            .checked_add_months(Months::new(boundary_months))
            .ok_or_else(|| anyhow::anyhow!("month overflow computing clock boundary"))?
    } else {
        epoch
    };

    // Step 2: Find the millisecond boundary within the current month interval
    if milliseconds > 0 {
        let ms_since_month_boundary = now.signed_duration_since(month_boundary).num_milliseconds();
        if ms_since_month_boundary < 0 {
            // now is before the computed month boundary — can happen with
            // day-of-month truncation; fall back to the previous month interval
            let prev_months = if months > 0 {
                let now_months_since_epoch = months_since_epoch(&now);
                let intervals = now_months_since_epoch / months;
                let prev_boundary_months = (intervals - 1)
                    .checked_mul(months)
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| anyhow::anyhow!("month overflow computing clock boundary"))?;
                epoch
                    .checked_add_months(Months::new(prev_boundary_months))
                    .ok_or_else(|| anyhow::anyhow!("month overflow computing clock boundary"))?
            } else {
                epoch
            };
            let ms_since = now.signed_duration_since(prev_months).num_milliseconds();
            let ms_intervals = ms_since / milliseconds;
            let offset_ms = ms_intervals
                .checked_mul(milliseconds)
                .ok_or_else(|| anyhow::anyhow!("millisecond overflow computing clock boundary"))?;
            Ok(prev_months + chrono::Duration::milliseconds(offset_ms))
        } else {
            let ms_intervals = ms_since_month_boundary / milliseconds;
            let offset_ms = ms_intervals
                .checked_mul(milliseconds)
                .ok_or_else(|| anyhow::anyhow!("millisecond overflow computing clock boundary"))?;
            Ok(month_boundary + chrono::Duration::milliseconds(offset_ms))
        }
    } else {
        Ok(month_boundary)
    }
}

#[async_trait::async_trait]
impl rtp::Plugin for TimePlugin {
    async fn create_resource(
        &mut self,
        _environment_qid: &str,
        _deployment_id: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        match id.typ.as_str() {
            CLOCK_RESOURCE_TYPE => self.clock_resource(inputs),
            _ => anyhow::bail!("unsupported resource type: {}", id.typ),
        }
    }

    async fn update_resource(
        &mut self,
        _environment_qid: &str,
        _deployment_id: &str,
        id: ids::ResourceId,
        _prev_inputs: sclc::Record,
        _prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        match id.typ.as_str() {
            CLOCK_RESOURCE_TYPE => self.clock_resource(inputs),
            _ => anyhow::bail!("unsupported resource type: {}", id.typ),
        }
    }

    async fn check(
        &self,
        _environment_qid: &str,
        _deployment_id: &str,
        id: ids::ResourceId,
        resource: sclc::Resource,
    ) -> anyhow::Result<sclc::Resource> {
        match id.typ.as_str() {
            CLOCK_RESOURCE_TYPE => self.clock_resource(resource.inputs),
            _ => Ok(resource),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    rtp::serve(&args.bind, TimePlugin::new).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_1_month_boundary() {
        // 2024-03-15T12:00:00Z should truncate to 2024-03-01T00:00:00Z with 1-month duration
        let now = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let boundary = truncate_to_boundary(now, 1, 0).unwrap();
        assert_eq!(boundary.to_rfc3339(), "2024-03-01T00:00:00+00:00");
    }

    #[test]
    fn truncate_1_month_1_ms_boundary() {
        // Example from the spec: 1 month + 1 millisecond
        // Second window starts at 1970-02-01T00:00:00.001Z
        // Third window starts at 1970-03-01T00:00:00.002Z
        let now = DateTime::parse_from_rfc3339("1970-02-01T00:00:00.001Z")
            .unwrap()
            .with_timezone(&Utc);
        let boundary = truncate_to_boundary(now, 1, 1).unwrap();
        // At exactly the second window start, we should get the second window
        assert_eq!(boundary.timestamp_millis(), now.timestamp_millis());

        // Just before the second window
        let now = DateTime::parse_from_rfc3339("1970-02-01T00:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc);
        let boundary = truncate_to_boundary(now, 1, 1).unwrap();
        // Should fall back to 1970-02-01T00:00:00.000Z (month boundary, 0 ms intervals)
        assert_eq!(
            boundary,
            DateTime::parse_from_rfc3339("1970-02-01T00:00:00.000Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn truncate_pure_milliseconds() {
        // 5000ms (5s) duration, at time 12345ms from epoch
        let now = DateTime::from_timestamp_millis(12345).unwrap();
        let boundary = truncate_to_boundary(now, 0, 5000).unwrap();
        // 12345 / 5000 = 2, so boundary = 10000ms
        assert_eq!(boundary.timestamp_millis(), 10000);
    }

    #[test]
    fn truncate_3_month_boundary() {
        // 2024-05-15 with 3-month intervals: 0, 3, 6, 9, 12, ...
        // months since epoch for 2024-05 = (2024-1970)*12 + 4 = 652
        // 652 / 3 = 217 intervals -> 651 months from epoch = 2024-04-01
        let now = DateTime::parse_from_rfc3339("2024-05-15T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let boundary = truncate_to_boundary(now, 3, 0).unwrap();
        assert_eq!(boundary.to_rfc3339(), "2024-04-01T00:00:00+00:00");
    }

    #[test]
    fn truncate_epoch() {
        let now = DateTime::from_timestamp_millis(0).unwrap();
        let boundary = truncate_to_boundary(now, 1, 1000).unwrap();
        assert_eq!(boundary.timestamp_millis(), 0);
    }

    // Edge case tests for negative/zero durations

    #[test]
    fn reject_zero_duration() {
        let plugin = TimePlugin::new();
        let mut inputs = sclc::Record::default();
        inputs.insert(String::from("months"), sclc::Value::Int(0));
        inputs.insert(String::from("milliseconds"), sclc::Value::Int(0));
        let result = plugin.clock_resource(inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be positive"));
    }

    #[test]
    fn reject_negative_months() {
        let plugin = TimePlugin::new();
        let mut inputs = sclc::Record::default();
        inputs.insert(String::from("months"), sclc::Value::Int(-1));
        inputs.insert(String::from("milliseconds"), sclc::Value::Int(1000));
        let result = plugin.clock_resource(inputs);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must be non-negative")
        );
    }

    #[test]
    fn reject_negative_milliseconds() {
        let plugin = TimePlugin::new();
        let mut inputs = sclc::Record::default();
        inputs.insert(String::from("months"), sclc::Value::Int(1));
        inputs.insert(String::from("milliseconds"), sclc::Value::Int(-500));
        let result = plugin.clock_resource(inputs);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must be non-negative")
        );
    }

    #[test]
    fn reject_both_negative() {
        let plugin = TimePlugin::new();
        let mut inputs = sclc::Record::default();
        inputs.insert(String::from("months"), sclc::Value::Int(-1));
        inputs.insert(String::from("milliseconds"), sclc::Value::Int(-1));
        let result = plugin.clock_resource(inputs);
        assert!(result.is_err());
    }

    #[test]
    fn months_since_epoch_helper() {
        let dt = DateTime::parse_from_rfc3339("2024-03-15T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // (2024-1970)*12 + (3-1) = 54*12 + 2 = 650
        assert_eq!(months_since_epoch(&dt), 650);

        let epoch = DateTime::from_timestamp_millis(0).unwrap();
        assert_eq!(months_since_epoch(&epoch), 0);
    }
}
