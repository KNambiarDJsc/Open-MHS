//! `*` wildcard matching for grant patterns. `*` matches any sequence, including dots.

pub fn matches(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            backtrack = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some((star, matched)) = backtrack {
            pi = star + 1;
            ti = matched + 1;
            backtrack = Some((star, matched + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn cases() {
        assert!(matches("power.output.*", "power.output.voltage.setpoint"));
        assert!(matches("sim.psu-*", "sim.psu-01"));
        assert!(matches("*", "anything.at.all"));
        assert!(matches("a*c", "abbbc"));
        assert!(matches("exact", "exact"));
        assert!(!matches("exact", "exactly"));
        assert!(!matches("power.output.*", "power.input.voltage"));
        assert!(!matches("sim.psu-*", "sim.axis-01"));
        assert!(matches("*.setpoint", "power.output.voltage.setpoint"));
        assert!(!matches("", "x"));
    }
}
