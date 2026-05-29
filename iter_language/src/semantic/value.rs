//! Generic value lowering used by external triggers.

use std::collections::BTreeMap;

use crate::ast::Value;
use crate::parser::RawValue;

pub(super) fn value_from_raw_pure(raw: RawValue) -> Value {
    match raw {
        RawValue::String(s, _) => Value::String(s),
        RawValue::Integer(n, _) => Value::Integer(n),
        RawValue::Bool(b, _) => Value::Bool(b),
        RawValue::Null(_) => Value::Null,
        RawValue::Duration(secs, _) => Value::DurationSecs(secs),
        RawValue::Ident(name, _) => Value::Ident(name),
        RawValue::List(items, _) => {
            Value::List(items.into_iter().map(value_from_raw_pure).collect())
        }
        RawValue::Block(block) => {
            let mut map = BTreeMap::new();
            for field in block.fields {
                map.insert(field.name.name, value_from_raw_pure(field.value));
            }
            Value::Block(map)
        }
        RawValue::Call { name, args, .. } => Value::Call {
            name,
            args: args.into_iter().map(value_from_raw_pure).collect(),
        },
    }
}
