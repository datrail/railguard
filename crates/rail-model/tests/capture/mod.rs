use serde_json::Value;

mod call;
mod compile;
mod plan;
mod window;

// RFC 6902's add, remove and replace, as the vectors use them.
pub fn patch(document: &mut Value, operation: &Value) {
    let pointer = operation["path"].as_str().unwrap();
    let (parent, last) = pointer.rsplit_once('/').unwrap();
    let parent = document.pointer_mut(parent).unwrap();
    let value = operation.get("value").cloned();
    match (operation["op"].as_str().unwrap(), parent) {
        ("remove", Value::Object(fields)) => drop(fields.remove(last).unwrap()),
        ("remove", Value::Array(items)) => drop(items.remove(last.parse().unwrap())),
        ("add" | "replace", Value::Object(fields)) => {
            drop(fields.insert(last.into(), value.unwrap()))
        }
        ("add", Value::Array(items)) if last == "-" => items.push(value.unwrap()),
        ("add", Value::Array(items)) => items.insert(last.parse().unwrap(), value.unwrap()),
        ("replace", Value::Array(items)) => items[last.parse::<usize>().unwrap()] = value.unwrap(),
        (op, _) => panic!("{op} at {pointer}"),
    }
}
