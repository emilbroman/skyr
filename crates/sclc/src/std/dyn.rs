pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn("Std/Dyn.cast", |args, _ctx| {
        let mut args = args.into_iter();
        let arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));

        if arg.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(arg.dependencies));
        }

        let mut record = crate::Record::default();

        let type_tag = match arg.value {
            crate::Value::Int(i) => {
                record.insert("int".into(), crate::Value::Int(i));
                "Int"
            }
            crate::Value::Float(f) => {
                record.insert("float".into(), crate::Value::Float(f));
                "Float"
            }
            crate::Value::Bool(b) => {
                record.insert("bool".into(), crate::Value::Bool(b));
                "Bool"
            }
            crate::Value::Str(s) => {
                record.insert("str".into(), crate::Value::Str(s));
                "Str"
            }
            crate::Value::Path(p) => {
                record.insert("path".into(), crate::Value::Path(p));
                "Path"
            }
            crate::Value::List(l) => {
                record.insert("list".into(), crate::Value::List(l));
                "List"
            }
            crate::Value::Dict(d) => {
                record.insert("dict".into(), crate::Value::Dict(d));
                "Dict"
            }
            crate::Value::Nil => "Nil",
            crate::Value::Record(_) => "Record",
            crate::Value::Fn(_) | crate::Value::ExternFn(_) => "Fn",
            crate::Value::Exception(_) => "Exception",
            crate::Value::Pending(_) => unreachable!("pending handled above"),
        };

        record.insert("typeTag".into(), crate::Value::Str(type_tag.into()));

        Ok(crate::TrackedValue::new(crate::Value::Record(record))
            .with_dependencies(arg.dependencies))
    });
}
