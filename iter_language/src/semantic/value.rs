//! Generic value lowering used by external triggers.

use std::collections::BTreeMap;

use crate::ast::Value;
use crate::parser::CstValue;

pub(super) fn value_from_raw_pure(raw: CstValue) -> Value {
    match raw {
        CstValue::String(s, _) => Value::String(s),
        CstValue::Integer(n, _) => Value::Integer(n),
        CstValue::Bool(b, _) => Value::Bool(b),
        CstValue::Null(_) => Value::Null,
        CstValue::Duration(secs, _) => Value::DurationSecs(secs),
        CstValue::Ident(name, _) => Value::Ident(name),
        CstValue::List(items, _) => {
            Value::List(items.into_iter().map(value_from_raw_pure).collect())
        }
        CstValue::Block(block) => {
            let mut map = BTreeMap::new();
            for field in block.fields {
                map.insert(field.name.name, value_from_raw_pure(field.value));
            }
            Value::Block(map)
        }
        CstValue::Call { name, args, .. } => Value::Call {
            name,
            args: args.into_iter().map(value_from_raw_pure).collect(),
        },
    }
}
