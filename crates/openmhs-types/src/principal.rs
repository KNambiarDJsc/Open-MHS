//! Principal names (spec/authorization.md §2, `common.schema.json#/$defs/principal`).
//!
//! An optional lowercase kind, a colon, then an identifier: `operator:local`,
//! `agent:lab-assistant`, or a bare identifier such as `agent.lab-assistant`.

const MAX_KIND: usize = 32;
const MAX_NAME: usize = 128;

/// True if `s` is a valid principal name.
pub fn is_principal(s: &str) -> bool {
    match s.split_once(':') {
        Some((kind, name)) => is_kind(kind) && is_identifier(name),
        None => is_identifier(s),
    }
}

fn is_kind(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && s.len() <= MAX_KIND
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// `^[a-z0-9](?:[a-z0-9._-]{0,126}[a-z0-9])?$`, the schema's `identifier`.
fn is_identifier(s: &str) -> bool {
    let edge = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let inner = |c: char| edge(c) || matches!(c, '.' | '_' | '-');
    !s.is_empty()
        && s.len() <= MAX_NAME
        && s.starts_with(edge)
        && s.ends_with(edge)
        && s.chars().all(inner)
}

#[cfg(test)]
mod tests {
    use super::is_principal;

    #[test]
    fn accepts_kinded_and_bare_names() {
        for ok in [
            "operator:local",
            "agent:lab-assistant",
            "agent.lab-assistant",
            "svc_2:x",
        ] {
            assert!(is_principal(ok), "{ok}");
        }
    }

    #[test]
    fn rejects_patterns_and_malformed_names() {
        for bad in [
            "", "agent:", ":x", "agent:*", "Agent:x", "agent:X", "a:b:c", "agent:-x", "agent:x.",
            "1kind:x", "agent x", "agent:ü",
        ] {
            assert!(!is_principal(bad), "{bad:?}");
        }
    }
}
