//! Command templates: `VOLT {{value}}`, or JSON such as `{"on": "{{value}}"}`.
//!
//! A placeholder names the written `value`, an action parameter, or `command_id`. In
//! JSON, a string that is exactly one placeholder is replaced by the typed JSON value;
//! placeholders inside longer strings are replaced by text. Simulators use the same
//! templates in reverse ([`match_text`], [`json_slots`]) to understand requests.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Slot(String),
}

fn parse(template: &str) -> Result<Vec<Segment>, String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        if start > 0 {
            out.push(Segment::Literal(rest[..start].to_owned()));
        }
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| format!("unclosed '{{{{' in '{template}'"))?;
        let name = after[..end].trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!(
                "'{{{{{name}}}}}' in '{template}' is not a placeholder name"
            ));
        }
        if matches!(out.last(), Some(Segment::Slot(_))) {
            return Err(format!(
                "two placeholders in a row in '{template}' cannot be told apart"
            ));
        }
        out.push(Segment::Slot(name.to_owned()));
        rest = &after[end + 2..];
    }
    if !rest.is_empty() {
        out.push(Segment::Literal(rest.to_owned()));
    }
    Ok(out)
}

/// The placeholder names in a text template, in order.
pub fn names(template: &str) -> Result<Vec<String>, String> {
    Ok(parse(template)?
        .into_iter()
        .filter_map(|s| match s {
            Segment::Slot(n) => Some(n),
            Segment::Literal(_) => None,
        })
        .collect())
}

/// Fill a text template. Every placeholder must have a value.
pub fn render_text(template: &str, vars: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::new();
    for seg in parse(template)? {
        match seg {
            Segment::Literal(l) => out.push_str(&l),
            Segment::Slot(n) => out.push_str(
                vars.get(&n)
                    .ok_or_else(|| format!("no value for '{{{{{n}}}}}'"))?,
            ),
        }
    }
    Ok(out)
}

/// Match `input` against a text template, ignoring ASCII case in the literal parts (as
/// SCPI does). Returns the text each placeholder matched.
pub fn match_text(template: &str, input: &str) -> Option<BTreeMap<String, String>> {
    let segs = parse(template).ok()?;
    let lower = input.to_ascii_lowercase();
    let mut pos = 0;
    let mut out = BTreeMap::new();
    for (i, seg) in segs.iter().enumerate() {
        match seg {
            Segment::Literal(l) => {
                let l = l.to_ascii_lowercase();
                if !lower[pos..].starts_with(&l) {
                    return None;
                }
                pos += l.len();
            }
            Segment::Slot(n) => {
                let end = match segs.get(i + 1) {
                    Some(Segment::Literal(next)) => {
                        pos + lower[pos..].find(&next.to_ascii_lowercase())?
                    }
                    _ => input.len(),
                };
                out.insert(n.clone(), input[pos..end].to_owned());
                pos = end;
            }
        }
    }
    (pos == input.len()).then_some(out)
}

/// Fill a JSON template. A string that is exactly `{{name}}` becomes the JSON value;
/// other strings have their placeholders replaced by text.
pub fn render_json(
    template: &serde_json::Value,
    vars: &BTreeMap<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    Ok(match template {
        serde_json::Value::String(s) => match exact_slot(s) {
            Some(n) => vars
                .get(&n)
                .cloned()
                .ok_or_else(|| format!("no value for '{{{{{n}}}}}'"))?,
            None => {
                let text: BTreeMap<String, String> = vars
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_str().map_or_else(|| v.to_string(), str::to_owned),
                        )
                    })
                    .collect();
                serde_json::Value::String(render_text(s, &text)?)
            }
        },
        serde_json::Value::Array(a) => serde_json::Value::Array(
            a.iter()
                .map(|v| render_json(v, vars))
                .collect::<Result<_, _>>()?,
        ),
        serde_json::Value::Object(o) => serde_json::Value::Object(
            o.iter()
                .map(|(k, v)| Ok((k.clone(), render_json(v, vars)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

/// Every placeholder in a JSON template, as (name, JSON pointer) for the strings that
/// are exactly one placeholder. Checks the other strings parse.
pub fn json_slots(template: &serde_json::Value) -> Result<Vec<(String, String)>, String> {
    fn walk(
        v: &serde_json::Value,
        at: &str,
        out: &mut Vec<(String, String)>,
    ) -> Result<(), String> {
        match v {
            serde_json::Value::String(s) => match exact_slot(s) {
                Some(n) => out.push((n, at.to_owned())),
                None => {
                    parse(s)?;
                }
            },
            serde_json::Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    walk(x, &format!("{at}/{i}"), out)?;
                }
            }
            serde_json::Value::Object(o) => {
                for (k, x) in o {
                    let k = k.replace('~', "~0").replace('/', "~1");
                    walk(x, &format!("{at}/{k}"), out)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(template, "", &mut out)?;
    Ok(out)
}

/// Every placeholder name in a JSON template, whether whole or inside text.
pub fn json_names(template: &serde_json::Value) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) -> Result<(), String> {
        match v {
            serde_json::Value::String(s) => out.extend(names(s)?),
            serde_json::Value::Array(a) => {
                for x in a {
                    walk(x, out)?;
                }
            }
            serde_json::Value::Object(o) => {
                for x in o.values() {
                    walk(x, out)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk(template, &mut out)?;
    Ok(out)
}

/// Set `pointer` in `doc`, creating objects on the way. Simulators use it to build the
/// documents a profile's pointers describe.
pub fn put(doc: &mut serde_json::Value, pointer: &str, value: serde_json::Value) {
    let mut at = doc;
    for raw in pointer.split('/').skip(1) {
        let key = raw.replace("~1", "/").replace("~0", "~");
        if !at.is_object() {
            *at = serde_json::json!({});
        }
        let Some(obj) = at.as_object_mut() else {
            return;
        };
        at = obj.entry(key).or_insert(serde_json::Value::Null);
    }
    *at = value;
}

/// Replace `${env:NAME}` with the environment variable, so secrets stay out of profiles.
pub fn expand_env(text: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${env:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 6..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("unclosed '${{env:' in '{text}'"))?;
        let name = &after[..end];
        out.push_str(
            &std::env::var(name).map_err(|_| format!("environment variable {name} is not set"))?,
        );
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn exact_slot(s: &str) -> Option<String> {
    match parse(s).ok()?.as_slice() {
        [Segment::Slot(n)] => Some(n.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn secrets_come_from_the_environment() {
        assert_eq!(expand_env("plain").unwrap(), "plain");
        assert!(
            expand_env("Bearer ${env:OPENMHS_TEST_SURELY_UNSET}")
                .unwrap_err()
                .contains("not set")
        );
        let path = std::env::var("PATH").unwrap_or_default();
        assert_eq!(expand_env("x${env:PATH}y").unwrap(), format!("x{path}y"));
    }

    #[test]
    fn put_builds_the_document_a_pointer_describes() {
        let mut doc = json!({});
        put(&mut doc, "/led/on", json!(true));
        put(&mut doc, "/a~1b", json!(1));
        assert_eq!(doc, json!({"led": {"on": true}, "a/b": 1}));
    }

    #[test]
    fn text_templates_render_and_match_back() {
        let t = "SOUR:VOLT {{value}}";
        assert_eq!(
            render_text(t, &vars(&[("value", "5")])).unwrap(),
            "SOUR:VOLT 5"
        );
        assert_eq!(match_text(t, "sour:volt 5.5").unwrap()["value"], "5.5");
        assert!(match_text(t, "SOUR:CURR 5").is_none());
        let m = match_text("MOVE {{pos}},{{speed}}", "MOVE 10,2").unwrap();
        assert_eq!((m["pos"].as_str(), m["speed"].as_str()), ("10", "2"));
        assert!(render_text("X {{a}}", &vars(&[])).is_err());
        assert!(parse("X {{a}}{{b}}").is_err());
        assert!(parse("X {{a").is_err());
    }

    #[test]
    fn json_templates_keep_types() {
        let t = json!({"on": "{{value}}", "id": "cmd-{{command_id}}", "n": [1, "{{count}}"]});
        let v: BTreeMap<String, serde_json::Value> = [
            ("value".to_owned(), json!(true)),
            ("command_id".to_owned(), json!("42")),
            ("count".to_owned(), json!(3)),
        ]
        .into();
        assert_eq!(
            render_json(&t, &v).unwrap(),
            json!({"on": true, "id": "cmd-42", "n": [1, 3]})
        );
        let mut slots = json_slots(&t).unwrap();
        slots.sort();
        assert_eq!(
            slots,
            [
                ("count".to_owned(), "/n/1".to_owned()),
                ("value".to_owned(), "/on".to_owned())
            ]
        );
        let mut all = json_names(&t).unwrap();
        all.sort();
        assert_eq!(all, ["command_id", "count", "value"]);
    }
}
