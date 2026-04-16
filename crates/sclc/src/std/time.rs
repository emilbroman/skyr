use chrono::{DateTime, Datelike, Months, Timelike};

use crate::{EvalErrorKind, Record, Value, ValueAssertions};

const SCHEDULE_RESOURCE_TYPE: &str = "Std/Time.Schedule";

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn("Std/Time.toISO", |args, _ctx| {
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(Value::Nil));

        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }

        first.try_map(|value| {
            let record = value.assert_record()?;
            let epoch_millis = *record.get("epochMillis").assert_int_ref()?;
            let dt = parse_instant(epoch_millis)?;
            Ok(Value::Str(dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()))
        })
    });

    eval.add_extern_fn("Std/Time.utc", |args, _ctx| {
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(Value::Nil));

        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }

        first.try_map(|value| {
            let record = value.assert_record()?;
            let epoch_millis = *record.get("epochMillis").assert_int_ref()?;
            let dt = parse_instant(epoch_millis)?;

            let mut date = Record::default();
            date.insert("year".into(), Value::Int(dt.year() as i64));
            date.insert("month".into(), Value::Int(dt.month() as i64));
            date.insert("day".into(), Value::Int(dt.day() as i64));

            let mut time = Record::default();
            time.insert("hour".into(), Value::Int(dt.hour() as i64));
            time.insert("minute".into(), Value::Int(dt.minute() as i64));
            time.insert("second".into(), Value::Int(dt.second() as i64));

            let mut datetime = Record::default();
            datetime.insert("date".into(), Value::Record(date));
            datetime.insert("time".into(), Value::Record(time));

            Ok(Value::Record(datetime))
        })
    });

    eval.add_extern_fn("Std/Time.add", |args, _ctx| {
        let mut args = args.into_iter();
        let instant_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(Value::Nil));
        let duration_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(Value::Nil));

        let deps = instant_arg
            .dependencies
            .union(&duration_arg.dependencies)
            .cloned()
            .collect();

        if instant_arg.value.has_pending() || duration_arg.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(deps));
        }

        let instant = instant_arg.value.assert_record()?;
        let duration = duration_arg.value.assert_record()?;
        let epoch_millis = *instant.get("epochMillis").assert_int_ref()?;
        let dt = parse_instant(epoch_millis)?;

        let dt = apply_duration(dt, &duration, 1)?;

        let mut result = Record::default();
        result.insert("epochMillis".into(), Value::Int(dt.timestamp_millis()));
        Ok(crate::TrackedValue::new(Value::Record(result)).with_dependencies(deps))
    });

    eval.add_extern_fn(SCHEDULE_RESOURCE_TYPE, |args, eval_ctx| {
        let (duration, argument_dependencies) = match super::extract_config(args)? {
            super::ExtractedConfig::Pending(pending) => return Ok(pending),
            super::ExtractedConfig::Ready {
                config,
                dependencies,
            } => (config, dependencies),
        };

        let months = match duration.get("months") {
            Value::Nil => 0,
            other => *other.assert_int_ref()?,
        };
        let milliseconds = match duration.get("milliseconds") {
            Value::Nil => 0,
            other => *other.assert_int_ref()?,
        };

        let id_str = format!("{months}/{milliseconds}");
        let resource_id = ids::ResourceId {
            typ: SCHEDULE_RESOURCE_TYPE.to_string(),
            name: id_str.clone(),
        };

        let mut inputs = Record::default();
        inputs.insert(String::from("months"), Value::Int(months));
        inputs.insert(String::from("milliseconds"), Value::Int(milliseconds));

        let Some(outputs) = eval_ctx.resource(
            SCHEDULE_RESOURCE_TYPE,
            &id_str,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            return Ok(crate::TrackedValue::pending().with_dependency(resource_id));
        };

        let epoch_millis = outputs.get("epochMillis").assert_int_ref()?;
        let mut result = Record::default();
        result.insert("epochMillis".into(), Value::Int(*epoch_millis));
        Ok(crate::TrackedValue::new(Value::Record(result)).with_dependency(resource_id))
    });

    eval.add_extern_fn("Std/Time.subtract", |args, _ctx| {
        let mut args = args.into_iter();
        let instant_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(Value::Nil));
        let duration_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(Value::Nil));

        let deps = instant_arg
            .dependencies
            .union(&duration_arg.dependencies)
            .cloned()
            .collect();

        if instant_arg.value.has_pending() || duration_arg.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(deps));
        }

        let instant = instant_arg.value.assert_record()?;
        let duration = duration_arg.value.assert_record()?;
        let epoch_millis = *instant.get("epochMillis").assert_int_ref()?;
        let dt = parse_instant(epoch_millis)?;

        let dt = apply_duration(dt, &duration, -1)?;

        let mut result = Record::default();
        result.insert("epochMillis".into(), Value::Int(dt.timestamp_millis()));
        Ok(crate::TrackedValue::new(Value::Record(result)).with_dependencies(deps))
    });
}

fn parse_instant(epoch_millis: i64) -> Result<DateTime<chrono::Utc>, crate::EvalError> {
    DateTime::from_timestamp_millis(epoch_millis).ok_or_else(|| {
        EvalErrorKind::Custom(format!("epoch millis {epoch_millis} is out of range")).into()
    })
}

/// Apply a Duration to a DateTime. `sign` is 1 for add, -1 for subtract.
fn apply_duration(
    mut dt: DateTime<chrono::Utc>,
    duration: &Record,
    sign: i64,
) -> Result<DateTime<chrono::Utc>, crate::EvalError> {
    // Months (big-to-small: apply months first)
    let months_val = duration.get("months");
    if !matches!(months_val, Value::Nil) {
        let months = *months_val.assert_int_ref()?;
        let effective = months * sign;
        if effective >= 0 {
            dt = dt
                .checked_add_months(Months::new(effective as u32))
                .ok_or_else(|| {
                    EvalErrorKind::Custom(format!(
                        "adding {effective} months would overflow the date"
                    ))
                })?;
        } else {
            dt = dt
                .checked_sub_months(Months::new(effective.unsigned_abs() as u32))
                .ok_or_else(|| {
                    EvalErrorKind::Custom(format!(
                        "subtracting {} months would overflow the date",
                        effective.unsigned_abs()
                    ))
                })?;
        }
    }

    // Milliseconds
    let ms_val = duration.get("milliseconds");
    if !matches!(ms_val, Value::Nil) {
        let ms = *ms_val.assert_int_ref()?;
        let effective = ms * sign;
        dt = dt
            .checked_add_signed(chrono::Duration::milliseconds(effective))
            .ok_or_else(|| {
                EvalErrorKind::Custom(format!(
                    "adding {effective} milliseconds would overflow the date"
                ))
            })?;
    }

    Ok(dt)
}
