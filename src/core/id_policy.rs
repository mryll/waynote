use std::collections::{HashMap, HashSet};

/// A loaded-but-unresolved note file.
pub struct RawId<'a> {
    pub path_key: &'a str,
    pub fm_id: &'a str,
    pub filename_id: Option<&'a str>,
}

/// Outcome per file: keep the existing id, or assign a freshly generated one.
pub struct Resolved {
    // read by tests / future callers to map results to input files
    #[allow(dead_code)]
    pub path_key: String,
    pub id: String,
    pub assigned: bool,
}

/// Injectable id source so tests are deterministic.
pub trait IdGen {
    fn new_id(&mut self) -> String;
}

/// Production id generator using `ulid::Ulid::new`.
// constructed when the binary wires persistence (later plan)
#[allow(dead_code)]
pub struct UlidGen;

impl IdGen for UlidGen {
    fn new_id(&mut self) -> String {
        ulid::Ulid::new().to_string()
    }
}

/// Resolve ids across the whole set.
///
/// Resolution rules (in priority order):
/// 1. A non-empty, well-formed (valid ULID), unique fm_id wins → kept.
/// 2. A valid ULID fm_id that is duplicated across files:
///    - The file whose filename_id == fm_id keeps it.
///    - If no filename matches, first in input order keeps it.
///    - All other claimants get a new id.
/// 3. Empty or invalid (non-ULID) fm_id → assign new id.
///
/// Tie-break for duplicate fm_id where no filename matches: first file in input
/// order keeps the id; all subsequent claimants are assigned new ids.
pub fn resolve(raws: &[RawId], gen: &mut impl IdGen) -> Vec<Resolved> {
    let winner_by_id = find_winners(raws);
    build_resolved(raws, &winner_by_id, gen)
}

/// For each valid fm_id, pick the single winner index.
fn find_winners(raws: &[RawId]) -> HashMap<String, usize> {
    let mut seen: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, raw) in raws.iter().enumerate() {
        if is_valid_ulid(raw.fm_id) {
            seen.entry(raw.fm_id).or_default().push(i);
        }
    }
    seen.into_iter().map(|(id, indices)| {
        let winner = pick_winner(raws, id, &indices);
        (id.to_string(), winner)
    }).collect()
}

/// Pick which index wins when multiple files share fm_id.
fn pick_winner(raws: &[RawId], id: &str, indices: &[usize]) -> usize {
    // Prefer the file whose filename_id matches.
    for &i in indices {
        if raws[i].filename_id == Some(id) {
            return i;
        }
    }
    // Tie-break: first in input order.
    indices[0]
}

fn build_resolved(
    raws: &[RawId],
    winner_by_id: &HashMap<String, usize>,
    gen: &mut impl IdGen,
) -> Vec<Resolved> {
    let winner_indices: HashSet<usize> = winner_by_id.values().copied().collect();
    raws.iter().enumerate().map(|(i, raw)| {
        if winner_indices.contains(&i) {
            Resolved { path_key: raw.path_key.to_string(), id: raw.fm_id.to_string(), assigned: false }
        } else {
            Resolved { path_key: raw.path_key.to_string(), id: gen.new_id(), assigned: true }
        }
    }).collect()
}

/// ULID validity check (26 chars, Crockford base32 uppercase canonical).
///
/// Crockford alphabet (uppercase): 0123456789ABCDEFGHJKMNPQRSTVWXYZ
/// Excluded: I, L, O, U (to avoid visual confusion).
/// The spec allows case-insensitive, but canonical form is uppercase.
pub fn is_valid_ulid(s: &str) -> bool {
    s.len() == 26 && s.chars().all(is_crockford_char)
}

fn is_crockford_char(c: char) -> bool {
    // Crockford base32 excludes I, L, O, U. The ranges below stop just before
    // each excluded letter: A-H (no I), J-K (no L), M-N (no O), P-T (no U), V-Z.
    matches!(c,
        '0'..='9' | 'A'..='H' | 'J'..='K' | 'M'..='N' | 'P'..='T' | 'V'..='Z'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real 26-char Crockford ULID for use in tests.
    const ID_A: &str = "01JZ9P6S0R8ZX0G8N3Z4V7Y8QK";
    const ID_B: &str = "01JZ9P6S0R8ZX0G8N3Z4V7Y8QM";

    /// Test double: returns "GEN1", "GEN2", ... on successive calls.
    struct SeqIdGen {
        counter: usize,
    }

    impl SeqIdGen {
        fn new() -> Self { SeqIdGen { counter: 0 } }
    }

    impl IdGen for SeqIdGen {
        fn new_id(&mut self) -> String {
            self.counter += 1;
            format!("GEN{}", self.counter)
        }
    }

    #[test]
    fn valid_unique_fm_id_is_kept() {
        let raws = [RawId { path_key: "a.md", fm_id: ID_A, filename_id: Some(ID_A) }];
        let mut gen = SeqIdGen::new();
        let resolved = resolve(&raws, &mut gen);
        assert_eq!(resolved[0].id, ID_A);
        assert!(!resolved[0].assigned);
    }

    #[test]
    fn empty_fm_id_gets_new_assigned_id() {
        let raws = [RawId { path_key: "a.md", fm_id: "", filename_id: None }];
        let mut gen = SeqIdGen::new();
        let resolved = resolve(&raws, &mut gen);
        assert_eq!(resolved[0].id, "GEN1");
        assert!(resolved[0].assigned);
    }

    #[test]
    fn invalid_fm_id_gets_new_assigned_id() {
        // "nope" is not a valid ULID (wrong length, wrong charset) → assign new.
        let raws = [RawId { path_key: "a.md", fm_id: "nope", filename_id: None }];
        let mut gen = SeqIdGen::new();
        let resolved = resolve(&raws, &mut gen);
        assert!(resolved[0].assigned);
    }

    #[test]
    fn duplicate_fm_id_filename_match_wins() {
        let raws = [
            RawId { path_key: "a.md", fm_id: ID_A, filename_id: Some(ID_A) },
            RawId { path_key: "b.md", fm_id: ID_A, filename_id: Some(ID_B) },
        ];
        let mut gen = SeqIdGen::new();
        let resolved = resolve(&raws, &mut gen);
        let a = resolved.iter().find(|r| r.path_key == "a.md").unwrap();
        let b = resolved.iter().find(|r| r.path_key == "b.md").unwrap();
        assert_eq!(a.id, ID_A);
        assert!(!a.assigned);
        assert_eq!(b.id, "GEN1");
        assert!(b.assigned);
    }

    #[test]
    fn duplicate_fm_id_no_filename_match_first_in_order_wins() {
        // Neither filename matches the id; first in input order keeps it.
        let raws = [
            RawId { path_key: "a.md", fm_id: ID_A, filename_id: Some(ID_B) },
            RawId { path_key: "b.md", fm_id: ID_A, filename_id: None },
        ];
        let mut gen = SeqIdGen::new();
        let resolved = resolve(&raws, &mut gen);
        let a = resolved.iter().find(|r| r.path_key == "a.md").unwrap();
        let b = resolved.iter().find(|r| r.path_key == "b.md").unwrap();
        assert_eq!(a.id, ID_A);
        assert!(!a.assigned);
        assert_eq!(b.id, "GEN1");
        assert!(b.assigned);
    }

    #[test]
    fn is_valid_ulid_accepts_real_ulid() {
        assert!(is_valid_ulid("01JZ9P6S0R8ZX0G8N3Z4V7Y8QK"));
    }

    #[test]
    fn is_valid_ulid_rejects_empty() {
        assert!(!is_valid_ulid(""));
    }

    #[test]
    fn is_valid_ulid_rejects_short_string() {
        assert!(!is_valid_ulid("abc"));
    }

    #[test]
    fn is_valid_ulid_rejects_string_with_crockford_excluded_char_u() {
        // 'U' is excluded from Crockford base32 alphabet.
        let s = "01JZ9P6S0R8ZX0G8N3Z4V7U8QK";
        assert_eq!(s.len(), 26);
        assert!(!is_valid_ulid(s));
    }

    #[test]
    fn is_valid_ulid_rejects_string_with_crockford_excluded_char_l() {
        // 'L' is excluded from Crockford base32 alphabet.
        let s = "01JZ9P6S0R8ZX0G8N3Z4V7L8QK";
        assert_eq!(s.len(), 26);
        assert!(!is_valid_ulid(s));
    }

    #[test]
    fn is_valid_ulid_rejects_string_with_crockford_excluded_char_i() {
        // 'I' is excluded from Crockford base32 alphabet.
        let s = "01JZ9P6S0R8ZX0G8N3Z4V7I8QK";
        assert_eq!(s.len(), 26);
        assert!(!is_valid_ulid(s));
    }

    #[test]
    fn is_valid_ulid_rejects_string_with_crockford_excluded_char_o() {
        // 'O' is excluded from Crockford base32 alphabet.
        let s = "01JZ9P6S0R8ZX0G8N3Z4V7O8QK";
        assert_eq!(s.len(), 26);
        assert!(!is_valid_ulid(s));
    }
}
